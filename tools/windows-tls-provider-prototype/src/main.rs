//! Windows-focused feasibility probe for a TLS 1.3 DER-SPKI pin and exporter.
//!
//! This does not implement QR pairing, trust storage, proof exchange, or media
//! transport. It creates one in-memory lab certificate for a loopback probe.

use std::fmt;
use std::io;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use rcgen::generate_simple_self_signed;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{WebPkiSupportedAlgorithms, verify_tls13_signature_with_raw_key};
use rustls::pki_types::{
    CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, SubjectPublicKeyInfoDer,
    UnixTime,
};
use rustls::{
    ClientConfig, ClientConnection, DigitallySignedStruct, Error, ProtocolVersion, ServerConfig,
    ServerConnection, SignatureScheme, version,
};
use sonimesh_protocol::{PAIRING_TLS_EXPORTER_LABEL, PAIRING_TLS_EXPORTER_LEN};
use x509_parser::parse_x509_certificate;
use zeroize::{Zeroize, Zeroizing};

const PROBE_CONTEXT: [u8; 16] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
];
const SOCKET_TIMEOUT: Duration = Duration::from_secs(5);

type ProbeResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn main() {
    if let Ok(()) = run_probe() {
        println!("tls13_handshake: ok");
        println!("tls13_certificate_verify: ok");
        println!("exporter: matched ({PAIRING_TLS_EXPORTER_LEN} bytes)");
        println!("altered_spki: rejected");
    } else {
        eprintln!("windows-tls-provider-prototype: failed");
        std::process::exit(1);
    }
}

fn run_probe() -> ProbeResult<()> {
    let certified_key = generate_simple_self_signed(vec!["localhost".to_owned()])?;
    let certificate = certified_key.cert.der().clone();
    let spki_pin = certified_key.key_pair.public_key_der();
    let mut private_key_der = Zeroizing::new(certified_key.key_pair.serialize_der());
    let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(private_key_der.to_vec()));
    private_key_der.zeroize();

    let server_config = Arc::new(server_config(certificate, private_key)?);
    let positive = run_loopback(Arc::clone(&server_config), spki_pin.clone())?;
    if positive.client_exporter.as_ref() != positive.server_exporter.as_ref() {
        return Err(io::Error::other("client and server exporters differed").into());
    }

    let mut altered_pin = spki_pin;
    let final_byte = altered_pin
        .last_mut()
        .ok_or_else(|| io::Error::other("generated SPKI was empty"))?;
    *final_byte ^= 0x01;
    if run_loopback(server_config, altered_pin).is_ok() {
        return Err(io::Error::other("altered SPKI completed a TLS handshake").into());
    }

    Ok(())
}

fn server_config(
    certificate: CertificateDer<'static>,
    private_key: PrivateKeyDer<'static>,
) -> ProbeResult<ServerConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    Ok(ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(vec![certificate], private_key)?)
}

struct ProbeExporters {
    client_exporter: Zeroizing<[u8; PAIRING_TLS_EXPORTER_LEN]>,
    server_exporter: Zeroizing<[u8; PAIRING_TLS_EXPORTER_LEN]>,
}

fn run_loopback(
    server_config: Arc<ServerConfig>,
    spki_pin: Vec<u8>,
) -> ProbeResult<ProbeExporters> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let address = listener.local_addr()?;
    let server = thread::spawn(move || server_exporter(listener, server_config));

    let client_result = client_exporter(address, spki_pin);
    let server_result = server
        .join()
        .map_err(|_| io::Error::other("TLS server thread panicked"))?;

    Ok(ProbeExporters {
        client_exporter: client_result?,
        server_exporter: server_result?,
    })
}

fn client_exporter(
    address: std::net::SocketAddr,
    spki_pin: Vec<u8>,
) -> ProbeResult<Zeroizing<[u8; PAIRING_TLS_EXPORTER_LEN]>> {
    let stream = TcpStream::connect_timeout(&address, SOCKET_TIMEOUT)?;
    configure_socket(&stream)?;
    let config = client_config(spki_pin)?;
    let server_name = ServerName::try_from("localhost")?.to_owned();
    let mut connection = ClientConnection::new(Arc::new(config), server_name)?;

    complete_client_handshake(&mut connection, stream)?;
    confirm_tls13(connection.protocol_version())?;
    export_client(&connection)
}

#[allow(clippy::needless_pass_by_value)]
fn server_exporter(
    listener: TcpListener,
    config: Arc<ServerConfig>,
) -> ProbeResult<Zeroizing<[u8; PAIRING_TLS_EXPORTER_LEN]>> {
    // The server thread owns the listener so returning always drops the bound port.
    let (stream, _) = listener.accept()?;
    configure_socket(&stream)?;
    let mut connection = ServerConnection::new(config)?;

    complete_server_handshake(&mut connection, stream)?;
    confirm_tls13(connection.protocol_version())?;
    export_server(&connection)
}

fn configure_socket(stream: &TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(SOCKET_TIMEOUT))?;
    stream.set_write_timeout(Some(SOCKET_TIMEOUT))
}

fn complete_client_handshake(
    connection: &mut ClientConnection,
    mut stream: TcpStream,
) -> ProbeResult<()> {
    while connection.is_handshaking() {
        connection.complete_io(&mut stream)?;
    }
    Ok(())
}

fn complete_server_handshake(
    connection: &mut ServerConnection,
    mut stream: TcpStream,
) -> ProbeResult<()> {
    while connection.is_handshaking() {
        connection.complete_io(&mut stream)?;
    }
    Ok(())
}

fn confirm_tls13(version: Option<ProtocolVersion>) -> ProbeResult<()> {
    if version == Some(ProtocolVersion::TLSv1_3) {
        Ok(())
    } else {
        Err(io::Error::other("connection did not negotiate TLS 1.3").into())
    }
}

fn export_client(
    connection: &ClientConnection,
) -> ProbeResult<Zeroizing<[u8; PAIRING_TLS_EXPORTER_LEN]>> {
    let output = connection.export_keying_material(
        [0_u8; PAIRING_TLS_EXPORTER_LEN],
        PAIRING_TLS_EXPORTER_LABEL,
        Some(&PROBE_CONTEXT),
    )?;
    Ok(Zeroizing::new(output))
}

fn export_server(
    connection: &ServerConnection,
) -> ProbeResult<Zeroizing<[u8; PAIRING_TLS_EXPORTER_LEN]>> {
    let output = connection.export_keying_material(
        [0_u8; PAIRING_TLS_EXPORTER_LEN],
        PAIRING_TLS_EXPORTER_LABEL,
        Some(&PROBE_CONTEXT),
    )?;
    Ok(Zeroizing::new(output))
}

fn client_config(spki_pin: Vec<u8>) -> ProbeResult<ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(SpkiPinVerifier::new(
        spki_pin,
        provider.signature_verification_algorithms,
    ));
    Ok(ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth())
}

/// TLS 1.3-only verifier that accepts only an exact DER-SPKI pin.
struct SpkiPinVerifier {
    expected_spki: Vec<u8>,
    supported_algorithms: WebPkiSupportedAlgorithms,
}

impl SpkiPinVerifier {
    const fn new(expected_spki: Vec<u8>, supported_algorithms: WebPkiSupportedAlgorithms) -> Self {
        Self {
            expected_spki,
            supported_algorithms,
        }
    }

    fn verify_pin(&self, end_entity: &CertificateDer<'_>) -> Result<(), Error> {
        let (remaining, certificate) = parse_x509_certificate(end_entity.as_ref())
            .map_err(|_| Error::General("server certificate could not be parsed".into()))?;
        if !remaining.is_empty() || certificate.public_key().raw != self.expected_spki {
            return Err(Error::General(
                "server SPKI did not match the supplied pin".into(),
            ));
        }
        Ok(())
    }
}

impl fmt::Debug for SpkiPinVerifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SpkiPinVerifier([REDACTED])")
    }
}

impl ServerCertVerifier for SpkiPinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        self.verify_pin(end_entity)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::General(
            "TLS 1.2 is disabled for this verifier".into(),
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        _cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        let spki = SubjectPublicKeyInfoDer::from(self.expected_spki.clone());
        verify_tls13_signature_with_raw_key(message, &spki, dss, &self.supported_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.supported_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_tls13_exporter_requires_the_exact_spki_pin() {
        run_probe().expect("fresh TLS 1.3 pin and exporter probe should pass");
    }

    #[test]
    fn probe_context_has_the_required_pairing_id_length() {
        assert_eq!(PROBE_CONTEXT.len(), 16);
    }
}
