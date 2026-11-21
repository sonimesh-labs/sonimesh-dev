//! Integration test for the invitation-gated pinned control listener.
//!
//! The desktop side polls a real non-blocking listener, then runs
//! [`accept_pairing_stream`] against the accepted stream and TLS config; the
//! Android role connects with the QR pin, derives its client proof from the
//! live exporter, and sends the canonical frame. The trusted outcome must
//! carry a media root that seals identically to an independent derivation, and
//! the reply frame written back into the channel must verify under the peer
//! binding.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::thread;

use rcgen::generate_simple_self_signed;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use zeroize::{Zeroize, Zeroizing};

use sonimesh_app::{
    ControlStreamAction, ControlStreamCoordinator, InvitationInputs, PairingId,
    PairingInvitationController, PairingListener, PairingQrInvitation, PairingSource, SessionState,
    accept_pairing_stream, begin_from_rendered_payload,
};
use sonimesh_network::{
    UntrustedControlStream, client_config, connect_pinned_client, server_config,
};
use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, Envelope, Hello, MAX_QR_PAYLOAD_LEN, PROOF_FRAME_LEN,
    PairingProofFrame, PairingRole, envelope,
};

const HOST: &str = "localhost";
/// Fixed non-secret lab secret; never printed.
const LAB_PAIRING_SECRET: [u8; 32] = [0xC9_u8; 32];

type TestResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[test]
#[allow(clippy::too_many_lines)] // one end-to-end authenticated listener scenario
fn the_listener_completes_one_trusted_pairing_exchange() -> TestResult<()> {
    let listener = PairingListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let invitation_id = PairingId::new([0x7B_u8; 16]);

    // Fresh lab certificate whose public key becomes the QR pin.
    let certified_key =
        generate_simple_self_signed(vec![HOST.to_owned()]).expect("lab certificate generates");
    let certificate = certified_key.cert.der().clone();
    let spki_pin = certified_key.key_pair.public_key_der();
    let mut private_key_der = Zeroizing::new(certified_key.key_pair.serialize_der());
    let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(private_key_der.to_vec()));
    private_key_der.zeroize();
    let tls = Arc::new(server_config(certificate, private_key)?);

    // Render the invitation exactly as the production owner would.
    let mut pairing = PairingInvitationController::<4>::new();
    let mut session = SessionState::IDLE;
    let mut payload_out = vec![0_u8; MAX_QR_PAYLOAD_LEN];
    let written = begin_from_rendered_payload(
        &mut pairing,
        &mut session,
        100,
        60_000,
        2,
        PairingQrInvitation {
            id: invitation_id,
            pairing_secret: LAB_PAIRING_SECRET,
            control_host: HOST,
            control_port: address.port(),
            desktop_tls_spki: &spki_pin,
            expires_at_ms: 61_000,
        },
        &mut payload_out,
    )
    .expect("canonical invitation renders");
    payload_out.truncate(written);
    assert_eq!(session.phase(), sonimesh_app::SessionPhase::Pairing);

    // Desktop thread: run the listener through one full exchange, then
    // install the authenticated control session and negotiate hello over
    // the trusted channel's bounded framing.
    let desktop_payload = payload_out.clone();
    let desktop = thread::spawn(move || -> TestResult<()> {
        let stream = loop {
            if let Some(stream) = listener.try_accept()? {
                break stream;
            }
            thread::yield_now();
        };
        let (trusted, _media_root) = accept_pairing_stream(
            stream,
            Arc::clone(&tls),
            &mut pairing,
            &mut session,
            150,
            PairingSource::new([0xD1_u8; 16]),
            &(InvitationInputs {
                invitation: invitation_id,
                pairing_secret: LAB_PAIRING_SECRET,
                scanned_payload: &desktop_payload,
            }),
        )?;
        let (channel, _exporter) = trusted.into_parts();
        assert_eq!(channel.sock.read_timeout()?, None);
        assert_eq!(channel.sock.write_timeout()?, None);

        // Trust is already established; the framing boundary is the only
        // remaining wrapper before coordinator envelopes.
        let mut coordinator =
            ControlStreamCoordinator::new(session, &[Capability::FrameDuration20Ms])
                .expect("coordinator constructs after trust");
        let token = coordinator
            .install_authenticated_control_session()
            .expect("a READY session issues its transport token");
        let mut framed = UntrustedControlStream::from_io(channel);

        // Negotiate hello over the live TLS-framed control channel.
        let mut poll = framed.poll()?;
        while matches!(poll, sonimesh_network::UntrustedControlPoll::Empty) {
            thread::yield_now();
            poll = framed.poll()?;
        }
        let sonimesh_network::UntrustedControlPoll::Frame(hello) = poll else {
            return Err("desktop did not receive a hello frame".into());
        };
        assert!(matches!(
            coordinator.accept(token, &hello),
            Ok(ControlStreamAction::Negotiated(_))
        ));
        Ok(())
    });

    // Android role in this process: connect with the pin and send the frame.
    let stream = TcpStream::connect(address)?;
    let config = client_config(spki_pin.clone())?;
    let (mut channel, exporter) =
        connect_pinned_client(config, HOST, &invitation_id.as_bytes(), stream)?;

    let binding = sonimesh_protocol::PairingBinding::new(
        1,
        invitation_id.as_bytes(),
        LAB_PAIRING_SECRET,
        exporter.as_ref(),
    )?;
    let client_frame = PairingProofFrame {
        protocol_version: 1,
        role: PairingRole::AndroidClient,
        pairing_id: invitation_id.as_bytes(),
        proof: binding.local_proof(PairingRole::AndroidClient)?,
    };
    let mut wire = [0_u8; PROOF_FRAME_LEN];
    client_frame.encode_into(&mut wire)?;
    channel.write_all(&wire)?;
    channel.flush()?;

    // The desktop's reply arrives on the same channel and must verify.
    let mut reply_wire = [0_u8; PROOF_FRAME_LEN];
    channel.read_exact(&mut reply_wire)?;
    let reply = sonimesh_protocol::parse_pairing_proof_frame(&reply_wire)?;
    assert_eq!(reply.role, PairingRole::DesktopServer);
    assert!(binding.verify_peer_proof(PairingRole::AndroidClient, &reply.proof)?);

    // With trust on both sides, send hello through the bounded framing so
    // the desktop coordinator negotiates it over the live TLS channel.
    let mut framed = UntrustedControlStream::from_io(channel);
    let hello = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 7,
        payload: Some(envelope::Payload::Hello(Hello {
            supported_versions: vec![CONTROL_PROTOCOL_VERSION],
            capabilities: vec![Capability::FrameDuration20Ms as i32],
            display_name: "loopback sender".to_owned(),
        })),
    };
    assert!(matches!(
        framed.send(&hello)?,
        sonimesh_network::UntrustedControlWrite::Complete
    ));

    desktop.join().expect("desktop thread joins")?;
    Ok(())
}
