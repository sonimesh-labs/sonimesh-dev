//! Cross-platform TLS interoperability measurement server.
//!
//! One side of the ADR 0021 promotion measurement: this desktop process
//! generates one fresh lab certificate, prints its DER SPKI as hex plus the
//! bound endpoint, then accepts one Conscrypt client from an Android device.
//! Both sides derive role-separated pairing proofs from their local exporters.
//! The fixed pairing secret is public test data and must never be used for pairing.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use rcgen::generate_simple_self_signed;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use sonimesh_protocol::{PAIRING_TLS_EXPORTER_LABEL, PairingBinding, PairingRole};
use zeroize::{Zeroize, Zeroizing};

const EXPORTER_BYTES: usize = 32;
/// Fixed public probe context shared with the Android instrumentation side.
const INTEROP_CONTEXT: [u8; 16] = [
    0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x00,
];
/// The Android client signals readiness by sending this byte first.
const CLIENT_READY: u8 = 0x01;
const SOCKET_TIMEOUT: Duration = Duration::from_secs(30);
const LAB_PAIRING_SECRET: [u8; 32] = [0x42; 32];

type ProbeResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn main() {
    let address = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:47610".to_owned());
    if let Err(error) = run_server(&address) {
        eprintln!("interop-server: failed: {error}");
        std::process::exit(1);
    }
}

fn run_server(address: &str) -> ProbeResult<()> {
    let certified_key = generate_simple_self_signed(vec!["sonimesh-interop.lab".to_owned()])?;
    let certificate = certified_key.cert.der().clone();
    let spki_pin = certified_key.key_pair.public_key_der();
    let mut private_key_der = Zeroizing::new(certified_key.key_pair.serialize_der());
    let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(private_key_der.to_vec()));
    private_key_der.zeroize();

    // TLS 1.3-only server config via the same boundary the production
    // listener uses; no client authentication, SPKI pin not needed here.
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(vec![certificate], private_key)?;
    let config = Arc::new(config);

    let listener = TcpListener::bind(address)?;
    let local = listener.local_addr()?;

    println!("SPKI_HEX {}", hex(&spki_pin));
    println!("LISTENING {local}");
    println!("READY");

    listener.set_nonblocking(true)?;
    let deadline = std::time::Instant::now() + SOCKET_TIMEOUT;
    let stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() >= deadline {
                    return Err("client connection timed out".into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    };
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(SOCKET_TIMEOUT))?;
    stream.set_write_timeout(Some(SOCKET_TIMEOUT))?;

    // The raw client signals readiness before the TLS handshake so a
    // misconfigured run fails fast instead of timing out mid-handshake.
    let mut marker = [0_u8; 1];
    let mut raw = stream;
    raw.read_exact(&mut marker)?;
    if marker[0] != CLIENT_READY {
        return Err(format!("unexpected client marker byte {:#04x}", marker[0]).into());
    }

    let connection = rustls::ServerConnection::new(Arc::clone(&config))?;
    let mut tls = rustls::StreamOwned::new(connection, raw);
    while tls.conn.is_handshaking() {
        tls.conn.complete_io(&mut tls.sock)?;
    }
    let protocol = tls
        .conn
        .protocol_version()
        .ok_or("no negotiated protocol")?;
    if protocol != rustls::ProtocolVersion::TLSv1_3 {
        return Err(format!("negotiated {protocol:?}, expected TLS 1.3").into());
    }
    println!("HANDSHAKE ok ({protocol:?})");

    let mut server_exporter = Zeroizing::new([0_u8; EXPORTER_BYTES]);
    tls.conn
        .export_keying_material(
            server_exporter.as_mut(),
            PAIRING_TLS_EXPORTER_LABEL,
            Some(INTEROP_CONTEXT.as_slice()),
        )
        .map_err(|_| "exporter call failed outside a TLS 1.3 session")?;

    let binding = PairingBinding::new(
        1,
        INTEROP_CONTEXT,
        LAB_PAIRING_SECRET,
        server_exporter.as_ref(),
    )?;
    let mut client_proof = Zeroizing::new([0_u8; EXPORTER_BYTES]);
    tls.read_exact(client_proof.as_mut())?;
    if !binding.verify_peer_proof(PairingRole::DesktopServer, client_proof.as_ref())? {
        return Err("client proof rejected".into());
    }
    let server_proof = Zeroizing::new(binding.local_proof(PairingRole::DesktopServer)?);
    tls.write_all(server_proof.as_ref())?;
    tls.flush()?;
    println!("PROOFS verified on server");
    println!("RESULT pass");
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02X}"));
    }
    out
}
