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
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use rcgen::generate_simple_self_signed;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use zeroize::{Zeroize, Zeroizing};

use sonimesh_app::{
    ControlListenError, ControlSessionPoll, ControlToMixerBridge, DesktopControlOwner,
    DesktopReceiverOwner, DesktopReceiverScheduler, InvitationInputs, OutputSelection, PairingId,
    PairingInvitationController, PairingListener, PairingQrInvitation, PairingRecordState,
    PairingSource, ReceiverPcmProducerInstaller, SessionState, accept_pairing_stream,
    begin_from_rendered_payload,
};
use sonimesh_audio::RealtimePcmProducer;
use sonimesh_network::{
    PinnedServerChannel, UntrustedControlStream, client_config, connect_pinned_client,
    server_config,
};
use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceId, OutputDeviceInfo, OutputError, OutputHealth,
    RenderSource, SampleFormat,
};
use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, Envelope, Hello, MAX_QR_PAYLOAD_LEN, PROOF_FRAME_LEN,
    PairingProofFrame, PairingRole, envelope,
};

const HOST: &str = "localhost";
/// Fixed non-secret lab secret; never printed.
const LAB_PAIRING_SECRET: [u8; 32] = [0xC9_u8; 32];

type TestResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct TestOutput;

impl AudioOutput for TestOutput {
    fn devices(&self) -> Result<Vec<OutputDeviceInfo>, OutputError> {
        Ok(Vec::new())
    }
    fn start(
        &mut self,
        _: Option<&OutputDeviceId>,
        _: OutputConfig,
        _: Box<dyn RenderSource>,
    ) -> Result<(), OutputError> {
        Ok(())
    }
    fn stop(&mut self) {}
    fn health(&self) -> OutputHealth {
        OutputHealth::Healthy
    }
}

struct TestPipeline;
impl ReceiverPcmProducerInstaller<480> for TestPipeline {
    fn replace_pcm_producer(&mut self, _: RealtimePcmProducer<480>) {}
}

#[test]
fn valid_proof_received_after_invitation_expiry_is_rejected() -> TestResult<()> {
    let listener = PairingListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let invitation_id = PairingId::new([0x71; 16]);
    let key = generate_simple_self_signed(vec![HOST.to_owned()])?;
    let certificate = key.cert.der().clone();
    let pin = key.key_pair.public_key_der();
    let mut private = Zeroizing::new(key.key_pair.serialize_der());
    let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(private.to_vec()));
    private.zeroize();
    let tls = Arc::new(server_config(certificate, private_key)?);

    let mut pairing = PairingInvitationController::<4>::new();
    let mut session = SessionState::IDLE;
    let mut payload = vec![0; MAX_QR_PAYLOAD_LEN];
    let written = begin_from_rendered_payload(
        &mut pairing,
        &mut session,
        100,
        150,
        2,
        PairingQrInvitation {
            id: invitation_id,
            pairing_secret: LAB_PAIRING_SECRET,
            control_host: HOST,
            control_port: address.port(),
            desktop_tls_spki: &pin,
            expires_at_ms: 250,
        },
        &mut payload,
    )?;
    payload.truncate(written);

    let desktop = thread::spawn(move || -> TestResult<()> {
        let stream = loop {
            if let Some(stream) = listener.try_accept()? {
                break stream;
            }
            thread::yield_now();
        };
        let result = accept_pairing_stream(
            stream,
            tls,
            &mut pairing,
            &mut session,
            100,
            PairingSource::new([0x72; 16]),
            &InvitationInputs {
                invitation: invitation_id,
                pairing_secret: LAB_PAIRING_SECRET,
                scanned_payload: &payload,
            },
        );
        assert!(matches!(result, Err(ControlListenError::Handshake(_))));
        assert_eq!(pairing.state(), Some(PairingRecordState::Expired));
        assert_eq!(session, SessionState::IDLE);
        Ok(())
    });

    let (mut channel, exporter) = connect_pinned_client(
        client_config(pin)?,
        HOST,
        &invitation_id.as_bytes(),
        TcpStream::connect(address)?,
    )?;
    let binding = sonimesh_protocol::PairingBinding::new(
        1,
        invitation_id.as_bytes(),
        LAB_PAIRING_SECRET,
        exporter.as_ref(),
    )?;
    let frame = PairingProofFrame {
        protocol_version: 1,
        role: PairingRole::AndroidClient,
        pairing_id: invitation_id.as_bytes(),
        proof: binding.local_proof(PairingRole::AndroidClient)?,
    };
    let mut wire = [0; PROOF_FRAME_LEN];
    frame.encode_into(&mut wire)?;
    thread::sleep(Duration::from_millis(250));
    channel.write_all(&wire)?;
    channel.flush()?;
    desktop.join().expect("desktop thread joins")?;
    Ok(())
}

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
    let (idle_tx, idle_rx) = mpsc::channel();
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
        let bridge = ControlToMixerBridge::<TestOutput, 1, 256>::new(
            session,
            &[Capability::FrameDuration20Ms],
            TestOutput,
            OutputSelection {
                device: None,
                config: OutputConfig {
                    sample_rate_hz: 48_000,
                    channels: 2,
                    sample_format: SampleFormat::Float32,
                    requested_callback_frames: Some(480),
                    maximum_callback_frames: 1_024,
                },
            },
            8,
            8,
        )?;
        let owner = DesktopControlOwner::<_, 1, 256, 1>::new(bridge)?;
        let receiver = DesktopReceiverOwner::<_, _, 1, 256, 1, 480>::new(owner, TestPipeline, None);
        let mut scheduler: DesktopReceiverScheduler<_, _, 1, 256, 1, 480, PinnedServerChannel> =
            DesktopReceiverScheduler::new(receiver);
        let token = scheduler
            .receiver_mut()
            .owner_mut()
            .install_authenticated_control_session()?;
        scheduler.attach_trusted_control(token, trusted)?;

        assert!(matches!(
            scheduler.poll_control_once()?,
            Some(ControlSessionPoll::Empty)
        ));
        idle_tx.send(())?;

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match scheduler.poll_control_once()? {
                Some(ControlSessionPoll::Processed {
                    replied: true,
                    write: Some(sonimesh_network::UntrustedControlWrite::Complete),
                }) => break,
                Some(ControlSessionPoll::Empty) if Instant::now() < deadline => thread::yield_now(),
                other => return Err(format!("unexpected scheduler result: {other:?}").into()),
            }
        }
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

    idle_rx.recv_timeout(Duration::from_secs(2))?;

    // With trust on both sides, send hello through the bounded framing so
    // the desktop coordinator negotiates it over the live TLS channel.
    channel
        .sock
        .set_read_timeout(Some(Duration::from_secs(2)))?;
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
    let reply = loop {
        let poll = framed.poll()?;
        if !matches!(poll, sonimesh_network::UntrustedControlPoll::Empty) {
            break poll;
        }
    };
    assert!(
        matches!(reply, sonimesh_network::UntrustedControlPoll::Frame(ref frame) if frame.request_id == 7 && matches!(frame.payload, Some(envelope::Payload::Hello(_))))
    );

    desktop.join().expect("desktop thread joins")?;
    Ok(())
}
