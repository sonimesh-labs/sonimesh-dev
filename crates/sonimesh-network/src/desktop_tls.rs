//! Desktop production TLS provider behind the ADR 0020 wrapper.
//!
//! ADR 0021 names Rustls with the `ring` provider as the desktop production
//! stack for pairing and authenticated control-plane connections: TLS 1.3
//! only, with the QR-carried DER SPKI as the sole trust anchor. This module
//! owns exactly that connection-establishment boundary; the proof exchange,
//! framing, coordinator token, and pairing records stay in their existing
//! modules.

use std::fmt;
use std::sync::Arc;

use rustls::SignatureScheme;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{WebPkiSupportedAlgorithms, verify_tls13_signature_with_raw_key};
use rustls::pki_types::{
    CertificateDer, PrivateKeyDer, ServerName, SubjectPublicKeyInfoDer, UnixTime,
};
use rustls::{
    ClientConfig, ClientConnection, ConnectionCommon, DigitallySignedStruct, Error,
    ProtocolVersion, ServerConfig, ServerConnection, StreamOwned, version,
};
use zeroize::Zeroizing;

use sonimesh_protocol::{PAIRING_TLS_EXPORTER_LABEL, PAIRING_TLS_EXPORTER_LEN};

/// Drives one channel's TLS handshake to completion over its socket.
macro_rules! drive_handshake {
    ($channel:expr) => {
        while $channel.conn.is_handshaking() {
            $channel
                .conn
                .complete_io(&mut $channel.sock)
                .map_err(DesktopTlsError::Io)?;
        }
    };
}

/// Failure while establishing or using a pinned desktop TLS channel.
pub enum DesktopTlsError {
    /// The pinned bytes are not a plausible DER SubjectPublicKeyInfo.
    InvalidPin,
    /// The scanned host is not a valid TLS server name.
    InvalidServerName,
    /// The supplied private key or certificate was rejected by Rustls.
    Configuration(rustls::Error),
    /// The TLS handshake itself failed (a wrong pin included).
    Handshake(rustls::Error),
    /// The connection did not negotiate TLS 1.3, or the exporter call ran
    /// outside a TLS 1.3 session.
    NotTls13,
    /// A socket operation failed during the handshake.
    Io(std::io::Error),
}

impl fmt::Debug for DesktopTlsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPin => formatter.write_str("InvalidPin"),
            Self::InvalidServerName => formatter.write_str("InvalidServerName"),
            Self::Configuration(_) => formatter.write_str("Configuration([REDACTED])"),
            Self::Handshake(_) => formatter.write_str("Handshake([REDACTED])"),
            Self::NotTls13 => formatter.write_str("NotTls13"),
            Self::Io(error) => formatter
                .debug_struct("Io")
                .field("kind", &error.kind())
                .finish(),
        }
    }
}

impl fmt::Display for DesktopTlsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPin => formatter.write_str("the pinned DER SPKI is not plausible"),
            Self::InvalidServerName => {
                formatter.write_str("the control host is not a valid TLS server name")
            }
            Self::Configuration(error) => write!(formatter, "TLS configuration failed: {error}"),
            Self::Handshake(error) => write!(formatter, "TLS handshake failed: {error}"),
            Self::NotTls13 => formatter.write_str("connection did not negotiate TLS 1.3"),
            Self::Io(error) => write!(formatter, "TLS channel I/O failed: {error}"),
        }
    }
}

impl std::error::Error for DesktopTlsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Configuration(error) | Self::Handshake(error) => Some(error),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

/// TLS 1.3-only verifier whose only trust anchor is one exact DER SPKI.
///
/// Mirrors the spike's `SpkiPinVerifier`: the end-entity certificate must
/// parse as a single X.509 certificate whose raw public key equals the pin,
/// the TLS 1.3 `CertificateVerify` signature is verified against that raw
/// key with the provider's supported algorithms, and TLS 1.2 is rejected
/// unconditionally.
struct SpkiPinVerifier {
    expected_spki: Vec<u8>,
    supported_algorithms: WebPkiSupportedAlgorithms,
}

impl SpkiPinVerifier {
    fn new(expected_spki: Vec<u8>, supported_algorithms: WebPkiSupportedAlgorithms) -> Self {
        Self {
            expected_spki,
            supported_algorithms,
        }
    }

    fn verify_pin(&self, end_entity: &CertificateDer<'_>) -> Result<(), Error> {
        let (remaining, certificate) = x509_parser::parse_x509_certificate(end_entity.as_ref())
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

/// Rejects pins that cannot be a DER SubjectPublicKeyInfo before any I/O.
///
/// A DER SPKI always opens with a SEQUENCE tag and a length byte; deeper
/// validity is enforced by the first handshake, where a malformed pin makes
/// the raw-key `CertificateVerify` verification fail closed.
fn plausible_pin(spki_pin: &[u8]) -> Result<(), DesktopTlsError> {
    match (spki_pin.first(), spki_pin.len()) {
        (Some(0x30), length) if length >= 2 => Ok(()),
        _ => Err(DesktopTlsError::InvalidPin),
    }
}

/// Builds the pinned client configuration: TLS 1.3-only, raw-SPKI trust.
///
/// # Errors
///
/// Returns [`DesktopTlsError::InvalidPin`] when the pin cannot be a DER
/// SubjectPublicKeyInfo.
pub fn client_config(spki_pin: Vec<u8>) -> Result<ClientConfig, DesktopTlsError> {
    plausible_pin(&spki_pin)?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(SpkiPinVerifier::new(
        spki_pin,
        provider.signature_verification_algorithms,
    ));
    Ok(ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&version::TLS13])
        .map_err(DesktopTlsError::Configuration)?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth())
}

/// Builds the server-side configuration presenting one certificate and key.
///
/// Used by the desktop pairing listener; the certificate's SPKI must equal
/// the pin embedded in the rendered QR invitation, which the caller already
/// guarantees by construction.
///
/// # Errors
///
/// Forwards Rustls configuration errors as [`DesktopTlsError::Configuration`].
pub fn server_config(
    certificate: CertificateDer<'static>,
    private_key: PrivateKeyDer<'static>,
) -> Result<ServerConfig, DesktopTlsError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&version::TLS13])
        .map_err(DesktopTlsError::Configuration)?
        .with_no_client_auth()
        .with_single_cert(vec![certificate], private_key)
        .map_err(DesktopTlsError::Configuration)
}

/// A completed pinned TLS channel usable as a plain `Read + Write` stream.
///
/// Wrapping `rustls::StreamOwned` keeps the TLS state machine and the socket
/// together so the caller can drive proof frames through the same channel
/// the exporter came from.
pub type PinnedClientChannel = rustls::StreamOwned<ClientConnection, std::net::TcpStream>;

/// Server-side counterpart of [`PinnedClientChannel`].
pub type PinnedServerChannel = rustls::StreamOwned<ServerConnection, std::net::TcpStream>;

/// Completes the client role of one pinned connection over a TCP stream.
///
/// Drives the handshake to completion, requires TLS 1.3, extracts exactly
/// [`PAIRING_TLS_EXPORTER_LEN`] bytes with the pairing label and context,
/// and returns the readable/writable channel plus the zeroized exporter.
/// The channel feeds
/// [`AuthenticatedControlHandshake`](crate::AuthenticatedControlHandshake)
/// directly.
///
/// # Errors
///
/// Returns the matching [`DesktopTlsError`] variant for every failure shape:
/// invalid server name, socket error, handshake rejection (including a wrong
/// pin), non-TLS 1.3 negotiation, or exporter failure.
pub fn connect_pinned_client(
    config: ClientConfig,
    server_name: &str,
    context: &[u8; 16],
    stream: std::net::TcpStream,
) -> Result<
    (
        PinnedClientChannel,
        Zeroizing<[u8; PAIRING_TLS_EXPORTER_LEN]>,
    ),
    DesktopTlsError,
> {
    let name = ServerName::try_from(server_name.to_owned())
        .map_err(|_| DesktopTlsError::InvalidServerName)?
        .to_owned();
    let connection =
        ClientConnection::new(Arc::new(config), name).map_err(DesktopTlsError::Configuration)?;
    let mut channel = StreamOwned::new(connection, stream);
    drive_handshake!(channel);
    confirm_tls13(channel.conn.protocol_version())?;
    let exporter = extract_exporter(&channel.conn, context)?;
    Ok((channel, exporter))
}

/// Completes the server role of one pinned connection over a TCP stream.
///
/// # Errors
///
/// Returns the matching [`DesktopTlsError`] variant for every failure shape.
pub fn accept_pinned_server(
    config: Arc<ServerConfig>,
    context: &[u8; 16],
    stream: std::net::TcpStream,
) -> Result<
    (
        PinnedServerChannel,
        Zeroizing<[u8; PAIRING_TLS_EXPORTER_LEN]>,
    ),
    DesktopTlsError,
> {
    accept_pinned_server_over_io(config, context, stream)
}

/// Completes the server handshake over caller-managed blocking I/O.
///
/// The caller is responsible for bounding I/O and retaining the transport
/// with the returned TLS state throughout the proof exchange.
///
/// # Errors
///
/// Returns the matching [`DesktopTlsError`] for configuration, handshake,
/// transport, protocol version, or exporter failures.
#[allow(clippy::type_complexity)] // The live channel and its exporter travel together.
pub fn accept_pinned_server_over_io<S: std::io::Read + std::io::Write>(
    config: Arc<ServerConfig>,
    context: &[u8; 16],
    stream: S,
) -> Result<
    (
        StreamOwned<ServerConnection, S>,
        Zeroizing<[u8; PAIRING_TLS_EXPORTER_LEN]>,
    ),
    DesktopTlsError,
> {
    let connection = ServerConnection::new(config).map_err(DesktopTlsError::Configuration)?;
    let mut channel = StreamOwned::new(connection, stream);
    drive_handshake!(channel);
    confirm_tls13(channel.conn.protocol_version())?;
    let exporter = extract_exporter(&channel.conn, context)?;
    Ok((channel, exporter))
}

/// Unwraps a temporary transport wrapper while preserving the live TLS state.
pub fn map_pinned_server_transport<S: std::io::Read + std::io::Write>(
    channel: StreamOwned<ServerConnection, S>,
    unwrap: impl FnOnce(S) -> std::net::TcpStream,
) -> PinnedServerChannel {
    StreamOwned::new(channel.conn, unwrap(channel.sock))
}

fn extract_exporter<C>(
    connection: &C,
    context: &[u8; 16],
) -> Result<Zeroizing<[u8; PAIRING_TLS_EXPORTER_LEN]>, DesktopTlsError>
where
    C: ExportPairingMaterial,
{
    let mut exporter = Zeroizing::new([0_u8; PAIRING_TLS_EXPORTER_LEN]);
    connection
        .export_pairing_material(exporter.as_mut(), Some(context.as_slice()))
        .map_err(|_| DesktopTlsError::NotTls13)?;
    Ok(exporter)
}

trait ExportPairingMaterial {
    fn export_pairing_material(
        &self,
        output: &mut [u8],
        context: Option<&[u8]>,
    ) -> Result<(), rustls::Error>;
}

impl ExportPairingMaterial for ClientConnection {
    fn export_pairing_material(
        &self,
        output: &mut [u8],
        context: Option<&[u8]>,
    ) -> Result<(), rustls::Error> {
        ConnectionCommon::export_keying_material(self, output, PAIRING_TLS_EXPORTER_LABEL, context)
            .map(|_| ())
    }
}

impl ExportPairingMaterial for ServerConnection {
    fn export_pairing_material(
        &self,
        output: &mut [u8],
        context: Option<&[u8]>,
    ) -> Result<(), rustls::Error> {
        ConnectionCommon::export_keying_material(self, output, PAIRING_TLS_EXPORTER_LABEL, context)
            .map(|_| ())
    }
}

fn confirm_tls13(negotiated: Option<ProtocolVersion>) -> Result<(), DesktopTlsError> {
    match negotiated {
        Some(ProtocolVersion::TLSv1_3) => Ok(()),
        _ => Err(DesktopTlsError::NotTls13),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
    use zeroize::{Zeroize, Zeroizing};

    use super::{PAIRING_TLS_EXPORTER_LEN, client_config, plausible_pin};
    use crate::desktop_tls::{
        DesktopTlsError, accept_pinned_server, connect_pinned_client, server_config,
    };

    /// Both exporters from one completed loopback channel.
    type LoopbackExporters = (
        Zeroizing<[u8; PAIRING_TLS_EXPORTER_LEN]>,
        Zeroizing<[u8; PAIRING_TLS_EXPORTER_LEN]>,
    );

    #[test]
    fn malformed_pins_fail_closed_before_any_io() {
        assert!(matches!(
            plausible_pin(&[]),
            Err(DesktopTlsError::InvalidPin)
        ));
        assert!(matches!(
            plausible_pin(&[0x04, 0x20]),
            Err(DesktopTlsError::InvalidPin)
        ));
        assert!(matches!(
            client_config(vec![0xFF; 32]),
            Err(DesktopTlsError::InvalidPin)
        ));
        // A SEQUENCE-shaped buffer passes the plausibility gate; deeper
        // validity stays enforced by the first handshake.
        let shaped = vec![0x30_u8, 0x1E, 0x02, 0x03];
        assert!(client_config(shaped).is_ok());
    }

    #[test]
    fn config_debug_does_not_leak_the_pinned_spki() {
        let mut shaped = vec![0x30_u8, 0x1E, 0x02, 0x03];
        shaped.extend(std::iter::repeat_n(0x55_u8, 28));
        let config = client_config(shaped.clone()).unwrap();
        // The pinned SPKI bytes never appear in the configuration's Debug
        // output; the verifier redacts its stored pin.
        let debug = format!("{config:?}");
        assert!(!debug.contains(&format!("{}{}", 0x55, 0x55)));
        assert!(!debug.contains("0x5555"));
    }

    #[test]
    fn error_debug_redacts_backend_details() {
        let error = DesktopTlsError::Io(std::io::Error::other(
            "127.0.0.1:43123 private TLS endpoint",
        ));
        let debug = format!("{error:?}");

        assert!(debug.contains("kind: Other"));
        assert!(!debug.contains("43123"));
        assert!(!debug.contains("private TLS endpoint"));
        assert_eq!(
            format!("{error}"),
            "TLS channel I/O failed: 127.0.0.1:43123 private TLS endpoint"
        );
    }

    #[test]
    fn server_configuration_requires_a_consistent_key_pair() {
        // A mismatched certificate/key pair fails closed at configuration
        // time instead of mid-handshake.
        let spki_der = vec![0x30_u8, 0x1E, 0x02, 0x03];
        assert!(matches!(
            server_config(
                rustls::pki_types::CertificateDer::from(spki_der),
                rustls::pki_types::PrivatePkcs8KeyDer::from(vec![0x02; 32]).into()
            ),
            Err(DesktopTlsError::Configuration(_))
        ));
    }

    /// One loopback pinned channel: fresh lab certificate, real TLS 1.3
    /// handshake, and both exporters extracted in memory.
    fn run_loopback(altered_pin: bool) -> Result<LoopbackExporters, DesktopTlsError> {
        fn io_other(error: std::io::Error) -> DesktopTlsError {
            DesktopTlsError::Io(error)
        }

        let certified_key = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()])
            .expect("lab certificate generates");
        let certificate = certified_key.cert.der().clone();
        let mut spki_pin = certified_key.key_pair.public_key_der();
        if altered_pin {
            let last = spki_pin.last_mut().expect("generated SPKI is never empty");
            *last ^= 0x01;
        }
        let mut private_key_der = Zeroizing::new(certified_key.key_pair.serialize_der());
        let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(private_key_der.to_vec()));
        private_key_der.zeroize();

        let server_config = Arc::new(
            server_config(certificate, private_key).expect("server config accepts its own key"),
        );
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(io_other)?;
        let address = listener.local_addr().map_err(io_other)?;
        let context = [0x5A_u8; 16];

        let server = std::thread::spawn(move || -> Result<_, DesktopTlsError> {
            let (stream, _) = listener.accept().map_err(io_other)?;
            accept_pinned_server(Arc::clone(&server_config), &context, stream)
                .map(|(_connection, exporter)| exporter)
        });

        let client_result = (|| -> Result<_, DesktopTlsError> {
            let stream =
                std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_secs(5))
                    .map_err(io_other)?;
            let config = client_config(spki_pin)?;
            connect_pinned_client(config, "localhost", &context, stream)
                .map(|(_connection, exporter)| exporter)
        })();

        let server_result = server.join().expect("server thread does not panic");
        Ok((client_result?, server_result?))
    }

    #[test]
    fn loopback_pinned_handshake_exports_equal_bytes_on_both_sides() {
        let (client_exporter, server_exporter) = run_loopback(false).expect("positive path");
        assert_eq!(client_exporter.as_ref(), server_exporter.as_ref());
    }

    #[test]
    fn altered_pin_fails_the_handshake_closed() {
        assert!(run_loopback(true).is_err());
    }
}
