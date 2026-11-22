//! Desktop invitation listener for the Android pairing-client lab.
//! The fixed ID and secret are public test data, never product credentials.

use std::fmt::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rcgen::generate_simple_self_signed;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use sonimesh_app::{
    InvitationInputs, PairingId, PairingInvitationController, PairingListener, PairingQrInvitation,
    PairingSource, SessionState, accept_pairing_stream, begin_from_rendered_payload,
};
use sonimesh_network::server_config;
use sonimesh_protocol::MAX_QR_PAYLOAD_LEN;
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
    let (_channel, _root) = accept_pairing_stream(
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
    Ok(())
}
