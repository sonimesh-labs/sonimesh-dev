//! Desktop invitation listener for the Android pairing-client lab.
//! The fixed ID and secret are public test data, never product credentials.

use std::fmt::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rcgen::generate_simple_self_signed;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use sonimesh_app::{
    ControlStreamAction, ControlStreamCoordinator, InvitationInputs, PairingId,
    PairingInvitationController, PairingListener, PairingQrInvitation, PairingSource, SessionState,
    accept_pairing_stream, begin_from_rendered_payload,
};
use sonimesh_network::{
    UntrustedControlPoll, UntrustedControlStream, UntrustedControlWrite, server_config,
};
use sonimesh_protocol::{Capability, Envelope, Hello, MAX_QR_PAYLOAD_LEN, envelope};
use zeroize::{Zeroize, Zeroizing};

const LAB_ID: [u8; 16] = [0x71; 16];
const LAB_SECRET: [u8; 32] = [0x42; 32];
type LabResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn main() {
    if let Err(error) = run() {
        eprintln!("pairing-lab: {error}");
        std::process::exit(1);
    }
}

fn run() -> LabResult<()> {
    let listener = PairingListener::bind("127.0.0.1:47610")?;
    let address = listener.local_addr()?;
    let key = generate_simple_self_signed(vec!["localhost".to_owned()])?;
    let pin = key.key_pair.public_key_der();
    let mut private = Zeroizing::new(key.key_pair.serialize_der());
    let tls = Arc::new(server_config(
        key.cert.der().clone(),
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(private.to_vec())),
    )?);
    private.zeroize();
    let id = PairingId::new(LAB_ID);
    let mut pairing = PairingInvitationController::<4>::new();
    let mut session = SessionState::IDLE;
    let mut payload = vec![0; MAX_QR_PAYLOAD_LEN];
    let length = begin_from_rendered_payload(
        &mut pairing,
        &mut session,
        100,
        120_000,
        2,
        PairingQrInvitation {
            id,
            pairing_secret: LAB_SECRET,
            control_host: "127.0.0.1",
            control_port: address.port(),
            desktop_tls_spki: &pin,
            expires_at_ms: 120_100,
        },
        &mut payload,
    )?;
    payload.truncate(length);
    let mut pin_hex = String::with_capacity(pin.len() * 2);
    for byte in &pin {
        write!(pin_hex, "{byte:02x}")?;
    }
    println!("SPKI_HEX {pin_hex}");
    println!("LISTENING {address}");
    println!("READY");
    let started = Instant::now();
    let stream = loop {
        if let Some(stream) = listener.try_accept()? {
            break stream;
        }
        if started.elapsed() >= Duration::from_secs(90) {
            return Err("client connection timed out".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let elapsed = u64::try_from(started.elapsed().as_millis())?;
    let (channel, _root) = accept_pairing_stream(
        stream,
        tls,
        &mut pairing,
        &mut session,
        100 + elapsed,
        PairingSource::new([0x72; 16]),
        &InvitationInputs {
            invitation: id,
            pairing_secret: LAB_SECRET,
            scanned_payload: &payload,
        },
    )?;
    println!("RESULT pairing verified");
    negotiate_hello(channel, session)
}

fn negotiate_hello(
    channel: sonimesh_app::TrustedControlChannel,
    session: SessionState,
) -> LabResult<()> {
    let (channel, _exporter) = channel.into_parts();
    channel
        .sock
        .set_read_timeout(Some(Duration::from_secs(30)))?;
    channel
        .sock
        .set_write_timeout(Some(Duration::from_secs(30)))?;
    let mut framed = UntrustedControlStream::from_io(channel);
    let mut coordinator = ControlStreamCoordinator::new(session, &[Capability::FrameDuration20Ms])?;
    let token = coordinator.install_authenticated_control_session()?;
    let request = loop {
        match framed.poll()? {
            UntrustedControlPoll::Frame(envelope) => break envelope,
            UntrustedControlPoll::Empty => continue,
            UntrustedControlPoll::Closed => {
                return Err("control channel closed before hello".into());
            }
        }
    };
    if !matches!(
        coordinator.accept(token, &request)?,
        ControlStreamAction::Negotiated(_)
    ) {
        return Err("expected a valid hello".into());
    }
    let reply = Envelope {
        protocol_version: 1,
        request_id: request.request_id,
        payload: Some(envelope::Payload::Hello(Hello {
            supported_versions: vec![1],
            capabilities: vec![Capability::FrameDuration20Ms as i32],
            display_name: "desktop lab".to_owned(),
        })),
    };
    let mut state = framed.send(&reply)?;
    while state == UntrustedControlWrite::Pending {
        state = framed.flush_write()?;
    }
    if state != UntrustedControlWrite::Complete {
        return Err("control channel closed during hello reply".into());
    }
    println!("RESULT hello negotiated");
    Ok(())
}
