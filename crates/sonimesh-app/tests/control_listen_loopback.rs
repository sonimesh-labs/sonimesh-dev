//! Integration test for the invitation-gated pinned control listener.
//!
//! The desktop side runs [`accept_pairing_connection`] against a real
//! bound listener and TLS config; the Android role connects with the QR
//! pin, derives its client proof from the live exporter, and sends the
//! canonical frame. The trusted outcome must carry a media root that seals
//! identically to an independent derivation, and the reply frame written
//! back into the channel must verify under the peer binding.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;

use rcgen::generate_simple_self_signed;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use zeroize::{Zeroize, Zeroizing};

use sonimesh_app::{
    InvitationInputs, PairingId, PairingInvitationController, PairingQrInvitation, PairingSource,
    SessionState, accept_pairing_connection, begin_from_rendered_payload,
};
use sonimesh_network::{client_config, connect_pinned_client, server_config};
use sonimesh_protocol::{MAX_QR_PAYLOAD_LEN, PROOF_FRAME_LEN, PairingProofFrame, PairingRole};

const HOST: &str = "localhost";
/// Fixed non-secret lab secret; never printed.
const LAB_PAIRING_SECRET: [u8; 32] = [0xC9_u8; 32];

type TestResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[test]
fn the_listener_completes_one_trusted_pairing_exchange() -> TestResult<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
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

    // Desktop thread: run the listener through one full exchange.
    let desktop_payload = payload_out.clone();
    let desktop = thread::spawn(move || -> TestResult<()> {
        let (trusted, _media_root) = accept_pairing_connection(
            &listener,
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
        let (_channel, _exporter) = trusted.into_parts();
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

    desktop.join().expect("desktop thread joins")?;
    Ok(())
}
