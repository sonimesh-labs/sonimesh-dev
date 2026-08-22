//! Windows-focused feasibility probe for a TLS 1.3 DER-SPKI pin and exporter.
//!
//! This does not implement QR pairing, trust storage, proof exchange, or media
//! transport. It creates one in-memory lab certificate for a loopback probe.

use std::fmt;
use std::io::{self, Read as _, Write as _};
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
    ServerConnection, SignatureScheme, StreamOwned, version,
};
use sonimesh_protocol::{
    PAIRING_TLS_EXPORTER_LABEL, PAIRING_TLS_EXPORTER_LEN, PROOF_FRAME_LEN, PairingBinding,
    PairingProofFrame, PairingRole, parse_pairing_proof_frame,
};
use x509_parser::parse_x509_certificate;
use zeroize::{Zeroize, Zeroizing};

const PROBE_CONTEXT: [u8; 16] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
];
const SOCKET_TIMEOUT: Duration = Duration::from_secs(5);

/// Fixed non-secret lab secret for the in-memory handshake-composition
/// stage. It never persists beyond the process and is never printed.
const LAB_INVITATION_SECRET: [u8; 32] = [
    0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xAB, 0xAC, 0xAD, 0xAE, 0xAF,
    0xB0, 0xB1, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xBB, 0xBC, 0xBD, 0xBE, 0xBF,
];

type ProbeResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn main() {
    if let Ok(version) = run_probe() {
        for line in success_output_lines(version) {
            println!("{line}");
        }
    } else {
        eprintln!("windows-tls-provider-prototype: failed");
        std::process::exit(1);
    }
}

fn run_probe() -> ProbeResult<ProtocolVersion> {
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
    if positive.client_version != positive.server_version {
        return Err(io::Error::other("client and server negotiated different TLS versions").into());
    }

    // The transport probe passed; drive the pure desktop pairing composition
    // in memory with the real exported bytes before the altered-pin stage.
    run_handshake_composition(&positive.client_exporter, &spki_pin)?;

    // The composition passed in memory; now exchange the transcript proofs as
    // bounded wire frames over a second live loopback TLS connection so the
    // frame layout is exercised end to end across a real channel boundary.
    run_frame_exchange(
        Arc::clone(&server_config),
        &positive.client_exporter,
        &spki_pin,
    )?;

    let mut altered_pin = spki_pin;
    let final_byte = altered_pin
        .last_mut()
        .ok_or_else(|| io::Error::other("generated SPKI was empty"))?;
    *final_byte ^= 0x01;
    if run_loopback(server_config, altered_pin).is_ok() {
        return Err(io::Error::other("altered SPKI completed a TLS handshake").into());
    }

    Ok(positive.client_version)
}

fn success_output_lines(version: ProtocolVersion) -> [&'static str; 8] {
    [
        "tls13_handshake: ok",
        "tls13_certificate_verify: ok",
        tls_version_output_line(version),
        "exporter: matched (32 bytes)",
        "handshake_composition: ok",
        "handshake_media_root_seal: matched",
        "proof_frame_exchange: ok (both roles verified)",
        "altered_spki: rejected",
    ]
}

fn tls_version_output_line(version: ProtocolVersion) -> &'static str {
    match version {
        ProtocolVersion::TLSv1_3 => "tls_version: TLSv1.3",
        _ => "tls_version: unexpected",
    }
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
    client_version: ProtocolVersion,
    server_version: ProtocolVersion,
}

/// Drives the pure desktop pairing composition in memory with the real
/// exported bytes (spike protocol: in-memory handshake-composition stage).
///
/// Both roles run in this one process: the invitation is rendered with the
/// generated SPKI as its pin, the Android-role proof is derived from the
/// real exporter, and `verify_scanned_proof` must reach the trusted outcome
/// with a media root that seals a datagram identical to an independent
/// derivation. No network pairing message is exchanged and nothing persists.
fn run_handshake_composition(
    exporter: &Zeroizing<[u8; PAIRING_TLS_EXPORTER_LEN]>,
    spki_pin: &[u8],
) -> ProbeResult<()> {
    use sonimesh_app::{
        PairingHandshakeOutcome, PairingId, PairingInvitationController, PairingQrInvitation,
        ScannedProofAttempt, SessionPhase, SessionState, begin_from_rendered_payload,
        verify_scanned_proof,
    };
    use sonimesh_protocol::{MAX_QR_PAYLOAD_LEN, MediaDirection, MediaKeyContext};

    const INVITATION_ID: PairingId = PairingId::new([
        0x5A, 0x5B, 0x5C, 0x5D, 0x5E, 0x5F, 0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
        0x69,
    ]);

    let mut pairing = PairingInvitationController::<4>::new();
    let mut session = SessionState::IDLE;
    let mut payload_out = vec![0_u8; MAX_QR_PAYLOAD_LEN];
    let written = begin_from_rendered_payload(
        &mut pairing,
        &mut session,
        1_000,
        60_000,
        2,
        PairingQrInvitation {
            id: INVITATION_ID,
            pairing_secret: LAB_INVITATION_SECRET,
            control_host: "localhost",
            control_port: 47_610,
            desktop_tls_spki: spki_pin,
            expires_at_ms: 61_000,
        },
        &mut payload_out,
    )
    .map_err(|error| io::Error::other(format!("invitation render failed: {error}")))?;

    // The Android peer derives its proof from the real exported transcript.
    let peer_binding = sonimesh_protocol::PairingBinding::new(
        1,
        INVITATION_ID.as_bytes(),
        LAB_INVITATION_SECRET,
        exporter.as_ref(),
    )
    .map_err(|error| io::Error::other(format!("peer binding failed: {error}")))?;
    let client_proof = peer_binding
        .local_proof(sonimesh_protocol::PairingRole::AndroidClient)
        .map_err(|error| io::Error::other(format!("peer proof failed: {error}")))?;

    let outcome = verify_scanned_proof(
        &mut pairing,
        &mut session,
        1_500,
        sonimesh_app::PairingSource::new([0xB0; 16]),
        exporter.as_ref(),
        ScannedProofAttempt {
            scanned_payload: &payload_out[..written],
            expected_pairing_id: INVITATION_ID,
            pairing_secret: LAB_INVITATION_SECRET,
            peer_proof: &client_proof,
        },
    )
    .map_err(|error| io::Error::other(format!("handshake rejected: {error}")))?;

    let PairingHandshakeOutcome::Ready { media_root } = outcome else {
        return Err(io::Error::other("handshake did not reach the trusted outcome").into());
    };
    if session.phase() != SessionPhase::Ready {
        return Err(io::Error::other("session did not reach the ready phase").into());
    }

    // The handshake root must match an independent derivation of the same
    // transcript: both roots seal one identical datagram.
    let expected_root = peer_binding
        .media_root()
        .map_err(|error| io::Error::other(format!("media root failed: {error}")))?;
    let context = MediaKeyContext {
        session_id: 0x5A5B_5C5D_5E5F_6061,
        stream_id: 1,
        key_epoch: 0,
        direction: MediaDirection::AndroidToDesktop,
    };
    let mut from_handshake = sonimesh_protocol::MediaSealer::new(&media_root, context)
        .map_err(|error| io::Error::other(format!("handshake sealer failed: {error}")))?;
    let mut from_expected = sonimesh_protocol::MediaSealer::new(&expected_root, context)
        .map_err(|error| io::Error::other(format!("expected sealer failed: {error}")))?;
    let sealed_handshake = from_handshake
        .seal(0, 7_000, &[0x33_u8; 48])
        .map_err(|error| io::Error::other(format!("handshake seal failed: {error}")))?;
    let sealed_expected = from_expected
        .seal(0, 7_000, &[0x33_u8; 48])
        .map_err(|error| io::Error::other(format!("expected seal failed: {error}")))?;
    if sealed_handshake.datagram != sealed_expected.datagram {
        return Err(io::Error::other("media root seal mismatched the derivation").into());
    }
    Ok(())
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

    let (client_exporter, client_version) = client_result?;
    let (server_exporter, server_version) = server_result?;
    Ok(ProbeExporters {
        client_exporter,
        server_exporter,
        client_version,
        server_version,
    })
}

/// Exchanges the transcript proofs as bounded wire frames over one live
/// loopback TLS connection (spike protocol: proof-frame exchange stage).
///
/// The Android-role side derives its client proof from the real exporter,
/// wraps it in a `PairingProofFrame`, and writes exactly the 55-byte frame
/// into the pinned TLS channel; the desktop side parses it, checks that its
/// pairing ID matches, verifies the carried proof through the pure binding,
/// then answers with its own server frame which the client verifies the same
/// way.
fn run_frame_exchange(
    server_config: Arc<ServerConfig>,
    exporter: &Zeroizing<[u8; PAIRING_TLS_EXPORTER_LEN]>,
    spki_pin: &[u8],
) -> ProbeResult<()> {
    const INVITATION_ID: [u8; 16] = [
        0x5A, 0x5B, 0x5C, 0x5D, 0x5E, 0x5F, 0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
        0x69,
    ];

    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let address = listener.local_addr()?;

    let desktop_secret = LAB_INVITATION_SECRET;
    // Clone for the server thread before moving; the original stays in this
    // scope so the client closure below can borrow it.
    let server_exporter = exporter.clone();
    let server = thread::spawn(move || -> ProbeResult<()> {
        let (stream, _) = listener.accept()?;
        configure_socket(&stream)?;
        let mut tls = StreamOwned::new(ServerConnection::new(server_config)?, stream);
        while tls.conn.is_handshaking() {
            tls.conn.complete_io(&mut tls.sock)?;
        }

        // Receive exactly one bounded client frame from the pinned channel.
        let mut wire = [0_u8; PROOF_FRAME_LEN];
        tls.read_exact(&mut wire)?;
        let received = parse_pairing_proof_frame(&wire)
            .map_err(|error| io::Error::other(format!("client frame rejected: {error}")))?;
        if received.pairing_id != INVITATION_ID || received.role != PairingRole::AndroidClient {
            return Err(io::Error::other("client frame identity mismatched").into());
        }

        let binding =
            PairingBinding::new(1, INVITATION_ID, desktop_secret, server_exporter.as_ref())
                .map_err(|error| io::Error::other(format!("desktop binding failed: {error}")))?;
        if !binding
            .verify_peer_proof(PairingRole::DesktopServer, &received.proof)
            .map_err(|error| io::Error::other(format!("client verification failed: {error}")))?
        {
            return Err(io::Error::other("client proof did not verify").into());
        }

        // Answer with the desktop server-role frame over the same channel.
        let server_frame = PairingProofFrame {
            protocol_version: 1,
            role: PairingRole::DesktopServer,
            pairing_id: INVITATION_ID,
            proof: binding
                .local_proof(PairingRole::DesktopServer)
                .map_err(|error| {
                    io::Error::other(format!("server proof derivation failed: {error}"))
                })?,
        };
        let mut wire = [0_u8; PROOF_FRAME_LEN];
        let written = server_frame
            .encode_into(&mut wire)
            .map_err(|error| io::Error::other(format!("server frame rejected: {error}")))?;
        tls.write_all(&wire[..written])?;
        tls.flush()?;
        Ok(())
    });

    // Android-role client: same invitation inputs, live exporter. The server
    // thread owns its cloned copy; this closure borrows the original.
    let client_result = (|| -> ProbeResult<()> {
        let stream = TcpStream::connect_timeout(&address, SOCKET_TIMEOUT)?;
        configure_socket(&stream)?;
        let config = client_config(spki_pin.to_vec())?;
        let server_name = ServerName::try_from("localhost")?.to_owned();
        let mut tls = StreamOwned::new(
            ClientConnection::new(Arc::new(config), server_name)?,
            stream,
        );
        while tls.conn.is_handshaking() {
            tls.conn.complete_io(&mut tls.sock)?;
        }

        let binding =
            PairingBinding::new(1, INVITATION_ID, LAB_INVITATION_SECRET, exporter.as_ref())
                .map_err(|error| io::Error::other(format!("android binding failed: {error}")))?;
        let client_frame = PairingProofFrame {
            protocol_version: 1,
            role: PairingRole::AndroidClient,
            pairing_id: INVITATION_ID,
            proof: binding
                .local_proof(PairingRole::AndroidClient)
                .map_err(|error| {
                    io::Error::other(format!("client proof derivation failed: {error}"))
                })?,
        };
        send_proof_frame(&mut tls, &client_frame)?;

        // Verify the desktop's server frame from the same pinned channel.
        let received = read_proof_frame(&mut tls)?;
        if received.pairing_id != INVITATION_ID || received.role != PairingRole::DesktopServer {
            return Err(io::Error::other("server frame identity mismatched").into());
        }
        if !binding
            .verify_peer_proof(PairingRole::AndroidClient, &received.proof)
            .map_err(|error| io::Error::other(format!("server verification failed: {error}")))?
        {
            return Err(io::Error::other("server proof did not verify").into());
        }

        // The exchange must compose with the media-root handoff: the client's
        // root is derived and sealed against a fixed datagram.
        seal_exchange_media_root(&binding)?;
        Ok(())
    })();

    let server_result = server
        .join()
        .map_err(|_| io::Error::other("frame-exchange server thread panicked"))?;
    client_result?;
    server_result?;
    Ok(())
}

/// Writes one bounded proof frame into the pinned TLS stream.
fn send_proof_frame(
    tls: &mut (impl io::Read + io::Write),
    frame: &PairingProofFrame,
) -> ProbeResult<()> {
    let mut wire = [0_u8; PROOF_FRAME_LEN];
    let written = frame
        .encode_into(&mut wire)
        .map_err(|error| io::Error::other(format!("proof frame rejected: {error}")))?;
    tls.write_all(&wire[..written])?;
    tls.flush()?;
    Ok(())
}

/// Reads exactly one bounded proof frame from the pinned TLS stream and parses it.
fn read_proof_frame(tls: &mut (impl io::Read + io::Write)) -> ProbeResult<PairingProofFrame> {
    let mut wire = [0_u8; PROOF_FRAME_LEN];
    tls.read_exact(&mut wire)?;
    let frame = parse_pairing_proof_frame(&wire)
        .map_err(|error| io::Error::other(format!("proof frame rejected: {error}")))?;
    Ok(frame)
}

/// Derives the exchange media root and seals one fixed datagram with it.
fn seal_exchange_media_root(binding: &PairingBinding) -> ProbeResult<()> {
    let root = binding
        .media_root()
        .map_err(|error| io::Error::other(format!("media root failed: {error}")))?;
    let context = sonimesh_protocol::MediaKeyContext {
        session_id: 0x5A5B_5C5D_5E5F_6061,
        stream_id: 1,
        key_epoch: 0,
        direction: sonimesh_protocol::MediaDirection::AndroidToDesktop,
    };
    let mut sealer = sonimesh_protocol::MediaSealer::new(&root, context)
        .map_err(|error| io::Error::other(format!("exchange sealer failed: {error}")))?;
    let _ = sealer
        .seal(0, 7_000, &[0x33_u8; 48])
        .map_err(|error| io::Error::other(format!("exchange seal failed: {error}")))?;
    Ok(())
}

fn client_exporter(
    address: std::net::SocketAddr,
    spki_pin: Vec<u8>,
) -> ProbeResult<(Zeroizing<[u8; PAIRING_TLS_EXPORTER_LEN]>, ProtocolVersion)> {
    let stream = TcpStream::connect_timeout(&address, SOCKET_TIMEOUT)?;
    configure_socket(&stream)?;
    let config = client_config(spki_pin)?;
    let server_name = ServerName::try_from("localhost")?.to_owned();
    let mut connection = ClientConnection::new(Arc::new(config), server_name)?;

    complete_client_handshake(&mut connection, stream)?;
    let version = confirm_tls13(connection.protocol_version())?;
    Ok((export_client(&connection)?, version))
}

#[allow(clippy::needless_pass_by_value)]
fn server_exporter(
    listener: TcpListener,
    config: Arc<ServerConfig>,
) -> ProbeResult<(Zeroizing<[u8; PAIRING_TLS_EXPORTER_LEN]>, ProtocolVersion)> {
    // The server thread owns the listener so returning always drops the bound port.
    let (stream, _) = listener.accept()?;
    configure_socket(&stream)?;
    let mut connection = ServerConnection::new(config)?;

    complete_server_handshake(&mut connection, stream)?;
    let version = confirm_tls13(connection.protocol_version())?;
    Ok((export_server(&connection)?, version))
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

fn confirm_tls13(version: Option<ProtocolVersion>) -> ProbeResult<ProtocolVersion> {
    match version {
        Some(version @ ProtocolVersion::TLSv1_3) => Ok(version),
        _ => Err(io::Error::other("connection did not negotiate TLS 1.3").into()),
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
        assert_eq!(
            run_probe().expect("fresh TLS 1.3 pin and exporter probe should pass"),
            ProtocolVersion::TLSv1_3
        );
    }

    #[test]
    fn successful_probe_output_is_exact_and_non_sensitive() {
        assert_eq!(
            success_output_lines(ProtocolVersion::TLSv1_3),
            [
                "tls13_handshake: ok",
                "tls13_certificate_verify: ok",
                "tls_version: TLSv1.3",
                "exporter: matched (32 bytes)",
                "handshake_composition: ok",
                "handshake_media_root_seal: matched",
                "proof_frame_exchange: ok (both roles verified)",
                "altered_spki: rejected",
            ]
        );
    }

    #[test]
    fn probe_context_has_the_required_pairing_id_length() {
        assert_eq!(PROBE_CONTEXT.len(), 16);
    }
}
