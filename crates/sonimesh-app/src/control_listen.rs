//! Invitation-gated pinned TLS control listener for the desktop.
//!
//! ADR 0021 names the desktop stack; this module owns the accept shape the
//! production server will run: one bounded listener that accepts a
//! connection, completes the pinned TLS 1.3 handshake, reads exactly one
//! ADR 0019 proof frame from that channel, admits it through
//! [`verify_scanned_proof`] (which charges the attempt, consumes a verified
//! invitation, and renders the exact reply frame), and writes the reply back
//! into the channel. The caller receives either a trusted outcome with the
//! transcript-bound media root and the still-open framed channel, or a typed
//! failure; nothing persists between attempts beyond the invitation
//! lifecycle itself.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

pub use sonimesh_network::ServerConfig as TlsServerConfig;
use sonimesh_network::{
    DesktopTlsError, PinnedServerChannel, accept_pinned_server_over_io, map_pinned_server_transport,
};
use sonimesh_protocol::{PAIRING_TLS_EXPORTER_LEN, PROOF_FRAME_LEN};

use crate::{
    PairingHandshakeError, PairingHandshakeOutcome, PairingId, PairingInvitationController,
    PairingSource, ScannedProofAttempt, SessionState, verify_scanned_proof,
};

/// Total time allowed for the pairing exchange after TCP acceptance.
pub const DEFAULT_PAIRING_IO_TIMEOUT: Duration = Duration::from_secs(30);

struct DeadlineStream {
    stream: TcpStream,
    deadline: Instant,
}

impl DeadlineStream {
    fn remaining(&self) -> std::io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "pairing deadline elapsed")
            })
    }
}

impl Read for DeadlineStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.stream.set_read_timeout(Some(self.remaining()?))?;
        let count = self.stream.read(buffer)?;
        self.remaining()?;
        Ok(count)
    }
}

impl Write for DeadlineStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        let count = self.stream.write(buffer)?;
        self.remaining()?;
        Ok(count)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.remaining()?;
        self.stream.flush()?;
        self.remaining()?;
        Ok(())
    }
}

/// Non-blocking listener handle for a caller-owned pairing accept loop.
///
/// The handle only accepts TCP peers. The caller decides when to run the
/// invitation-gated TLS/proof exchange by passing an accepted stream to
/// [`accept_pairing_stream`]. Pairing state remains caller-owned and therefore
/// stays serialized at the handshake boundary.
pub struct PairingListener {
    listener: TcpListener,
}

impl PairingListener {
    /// Binds a non-blocking pairing listener.
    ///
    /// # Errors
    ///
    /// Returns the underlying socket error when binding or configuring the
    /// listener fails.
    pub fn bind(address: impl ToSocketAddrs) -> Result<Self, std::io::Error> {
        Self::from_listener(TcpListener::bind(address)?)
    }

    /// Converts an existing listener to the non-blocking polling boundary.
    ///
    /// # Errors
    ///
    /// Returns the underlying socket error when enabling non-blocking mode
    /// fails.
    pub fn from_listener(listener: TcpListener) -> Result<Self, std::io::Error> {
        listener.set_nonblocking(true)?;
        Ok(Self { listener })
    }

    /// Returns the bound endpoint.
    ///
    /// # Errors
    ///
    /// Returns the underlying socket error when the local endpoint cannot be
    /// queried.
    pub fn local_addr(&self) -> Result<std::net::SocketAddr, std::io::Error> {
        self.listener.local_addr()
    }

    /// Accepts one peer without waiting for a connection.
    ///
    /// Returns `Ok(None)` when the listener has no queued peer yet.
    ///
    /// # Errors
    ///
    /// Returns [`ControlListenError::Socket`] for an accept error other than
    /// `WouldBlock`.
    pub fn try_accept(&self) -> Result<Option<TcpStream>, ControlListenError> {
        match self.listener.accept() {
            Ok((stream, _)) => Ok(Some(stream)),
            Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock) => Ok(None),
            Err(error) => Err(ControlListenError::Socket(error)),
        }
    }
}

/// Failure while accepting and verifying one pinned pairing connection.
pub enum ControlListenError {
    /// Binding or accepting on the control socket failed.
    Socket(std::io::Error),
    /// The pinned TLS handshake failed or did not negotiate TLS 1.3.
    Tls(DesktopTlsError),
    /// Reading the peer's proof frame failed before parsing.
    PeerFrameIo(std::io::Error),
    /// The composed handshake rejected the attempt (QR decode, identity,
    /// admission, or proof verification).
    Handshake(crate::PairingHandshakeError),
    /// Writing the reply frame failed after a trusted outcome; the session
    /// is already trusted but this channel is unusable.
    ReplyWrite(std::io::Error),
}

impl core::fmt::Debug for ControlListenError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Socket(error) => formatter
                .debug_struct("Socket")
                .field("kind", &error.kind())
                .finish(),
            Self::Tls(error) => formatter
                .debug_struct("Tls")
                .field("category", &tls_error_category(error))
                .finish(),
            Self::PeerFrameIo(error) => formatter
                .debug_struct("PeerFrameIo")
                .field("kind", &error.kind())
                .finish(),
            Self::Handshake(error) => formatter
                .debug_struct("Handshake")
                .field("category", &handshake_error_category(*error))
                .finish(),
            Self::ReplyWrite(error) => formatter
                .debug_struct("ReplyWrite")
                .field("kind", &error.kind())
                .finish(),
        }
    }
}

fn tls_error_category(error: &DesktopTlsError) -> &'static str {
    match error {
        DesktopTlsError::InvalidPin => "invalid_pin",
        DesktopTlsError::InvalidServerName => "invalid_server_name",
        DesktopTlsError::Configuration(_) => "configuration",
        DesktopTlsError::Handshake(_) => "handshake",
        DesktopTlsError::NotTls13 => "not_tls13",
        DesktopTlsError::Io(_) => "io",
    }
}

fn handshake_error_category(error: PairingHandshakeError) -> &'static str {
    match error {
        PairingHandshakeError::Qr(_) => "qr",
        PairingHandshakeError::InvitationMismatch => "invitation_mismatch",
        PairingHandshakeError::Binding(_) => "binding",
        PairingHandshakeError::Invitation(_) => "invitation",
        PairingHandshakeError::ProofRejected => "proof_rejected",
        PairingHandshakeError::InvalidExporterLength => "invalid_exporter_length",
        PairingHandshakeError::ProofFrame(_) => "proof_frame",
    }
}

impl core::fmt::Display for ControlListenError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Socket(error) => write!(formatter, "control listener I/O failed: {error}"),
            Self::Tls(error) => write!(formatter, "pinned TLS handshake failed: {error}"),
            Self::PeerFrameIo(error) => {
                write!(formatter, "peer proof frame could not be read: {error}")
            }
            Self::Handshake(error) => {
                write!(formatter, "pairing handshake rejected the attempt: {error}")
            }
            Self::ReplyWrite(error) => {
                write!(formatter, "reply frame could not be written: {error}")
            }
        }
    }
}

impl std::error::Error for ControlListenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Socket(error) | Self::PeerFrameIo(error) | Self::ReplyWrite(error) => Some(error),
            Self::Tls(error) => Some(error),
            Self::Handshake(error) => Some(error),
        }
    }
}

/// One trusted desktop pairing outcome over a live pinned channel.
pub struct TrustedControlChannel {
    channel: PinnedServerChannel,
    exporter: Zeroizing<[u8; PAIRING_TLS_EXPORTER_LEN]>,
}

impl TrustedControlChannel {
    /// Hands the framed TLS channel to the ADR 0020 wrapper.
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        PinnedServerChannel,
        Zeroizing<[u8; PAIRING_TLS_EXPORTER_LEN]>,
    ) {
        (self.channel, self.exporter)
    }
}

impl core::fmt::Debug for TrustedControlChannel {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("TrustedControlChannel([REDACTED])")
    }
}

/// Everything one invitation needs to accept and verify its peer.
///
/// `scanned_payload` must be the exact bytes that
/// [`begin_from_rendered_payload`](crate::begin_from_rendered_payload)
/// wrote, so the admitted scan is the rendered invitation itself.
pub struct InvitationInputs<'a> {
    /// The one-time 128-bit invitation identifier.
    pub invitation: PairingId,
    /// The one-time 256-bit pairing secret rendered into that invitation.
    pub pairing_secret: [u8; 32],
    /// The exact rendered QR payload bytes.
    pub scanned_payload: &'a [u8],
}

impl core::fmt::Debug for InvitationInputs<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("InvitationInputs")
            .field("invitation", &"[REDACTED]")
            .field("pairing_secret", &"[REDACTED]")
            .field("scanned_payload_len", &self.scanned_payload.len())
            .finish()
    }
}

/// Accepts exactly one pinned pairing connection against the active
/// invitation and returns the trusted channel with its media root.
///
/// The listener must already be bound by the caller so the same endpoint
/// that rendered into the QR payload is used here. The function performs
/// the full sequence: pinned TLS handshake, one bounded proof-frame read,
/// attempt admission through the real owner, reply write.
/// `now_ms` must be sampled immediately before this call from the same
/// monotonic clock used to create the invitation.
///
/// # Errors
///
/// Returns [`ControlListenError`] for every failure shape along that
/// sequence. A well-formed but wrong proof surfaces as
/// [`ControlListenError::Handshake`] wrapping
/// [`PairingHandshakeError::ProofRejected`] while the invitation keeps its
/// remaining attempts.
pub fn accept_pairing_connection(
    listener: &TcpListener,
    tls: Arc<TlsServerConfig>,
    pairing: &mut PairingInvitationController<4>,
    session: &mut SessionState,
    now_ms: u64,
    source: PairingSource,
    inputs: &InvitationInputs<'_>,
) -> Result<(TrustedControlChannel, sonimesh_protocol::SessionSecret), ControlListenError> {
    let started = Instant::now();
    let (stream, _) = listener.accept().map_err(ControlListenError::Socket)?;
    accept_pairing_stream_with_timeout(
        stream,
        tls,
        pairing,
        session,
        now_ms,
        source,
        inputs,
        DEFAULT_PAIRING_IO_TIMEOUT,
        started,
    )
}

/// Completes one invitation-gated exchange on an already accepted TCP stream.
///
/// This is the handshake half of [`PairingListener`]. It is separate from
/// accepting so a caller can poll or schedule socket admission without making
/// the pairing owner or the accept loop concurrent.
/// `now_ms` must be sampled immediately before this call from the same
/// monotonic clock used to create the invitation.
///
/// # Errors
///
/// Returns [`ControlListenError`] when the pinned TLS handshake, proof-frame
/// exchange, invitation admission, or reply write fails.
pub fn accept_pairing_stream(
    stream: TcpStream,
    tls: Arc<TlsServerConfig>,
    pairing: &mut PairingInvitationController<4>,
    session: &mut SessionState,
    now_ms: u64,
    source: PairingSource,
    inputs: &InvitationInputs<'_>,
) -> Result<(TrustedControlChannel, sonimesh_protocol::SessionSecret), ControlListenError> {
    accept_pairing_stream_with_timeout(
        stream,
        tls,
        pairing,
        session,
        now_ms,
        source,
        inputs,
        DEFAULT_PAIRING_IO_TIMEOUT,
        Instant::now(),
    )
}

#[allow(clippy::too_many_arguments)]
fn accept_pairing_stream_with_timeout(
    stream: TcpStream,
    tls: Arc<TlsServerConfig>,
    pairing: &mut PairingInvitationController<4>,
    session: &mut SessionState,
    now_ms: u64,
    source: PairingSource,
    inputs: &InvitationInputs<'_>,
    io_timeout: Duration,
    started: Instant,
) -> Result<(TrustedControlChannel, sonimesh_protocol::SessionSecret), ControlListenError> {
    let deadline = Instant::now().checked_add(io_timeout).ok_or_else(|| {
        ControlListenError::Socket(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "pairing deadline overflow",
        ))
    })?;
    // Accepted sockets may inherit the listener's non-blocking mode.
    stream
        .set_nonblocking(false)
        .map_err(ControlListenError::Socket)?;
    let stream = DeadlineStream { stream, deadline };
    let (mut channel, exporter) =
        accept_pinned_server_over_io(tls, &inputs.invitation.as_bytes(), stream)
            .map_err(ControlListenError::Tls)?;

    // Read exactly one canonical peer frame from the live channel.
    let mut wire = [0_u8; PROOF_FRAME_LEN];
    channel
        .read_exact(&mut wire)
        .map_err(ControlListenError::PeerFrameIo)?;
    channel
        .sock
        .remaining()
        .map_err(ControlListenError::PeerFrameIo)?;

    let outcome = verify_scanned_proof(
        pairing,
        session,
        admission_time_ms(now_ms, started),
        source,
        exporter.as_ref(),
        ScannedProofAttempt {
            scanned_payload: inputs.scanned_payload,
            expected_pairing_id: inputs.invitation,
            pairing_secret: inputs.pairing_secret,
            peer_proof_frame: &wire,
        },
    )
    .map_err(ControlListenError::Handshake)?;

    let PairingHandshakeOutcome::Ready {
        media_root,
        reply_frame,
    } = outcome
    else {
        // verify_scanned_proof returns Ok(Rejected{..}) for a well-formed
        // wrong proof; surface it as the composition-level rejection.
        return Err(ControlListenError::Handshake(
            PairingHandshakeError::ProofRejected,
        ));
    };

    let mut reply_wire = [0_u8; PROOF_FRAME_LEN];
    reply_frame
        .encode_into(&mut reply_wire)
        .map_err(|_| ControlListenError::ReplyWrite(std::io::Error::other("frame overflow")))?;
    channel
        .write_all(&reply_wire)
        .and_then(|()| channel.flush())
        .map_err(ControlListenError::ReplyWrite)?;
    // The deadline protects only connection establishment and proof exchange;
    // the authenticated control stream owns its idle policy separately.
    channel
        .sock
        .stream
        .set_read_timeout(None)
        .and_then(|()| channel.sock.stream.set_write_timeout(None))
        .map_err(ControlListenError::ReplyWrite)?;
    let channel = map_pinned_server_transport(channel, |socket| socket.stream);

    Ok((TrustedControlChannel { channel, exporter }, media_root))
}

fn admission_time_ms(now_ms: u64, started: Instant) -> u64 {
    let Some(elapsed) = Instant::now().checked_duration_since(started) else {
        return u64::MAX;
    };
    let millis = elapsed.as_millis();
    now_ms.saturating_add(u64::try_from(millis).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, mpsc};
    use std::thread;
    use std::time::{Duration, Instant};

    use rcgen::generate_simple_self_signed;
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
    use zeroize::{Zeroize, Zeroizing};

    use super::{
        ControlListenError, InvitationInputs, PairingId, PairingInvitationController,
        PairingListener, PairingSource, SessionState, accept_pairing_stream_with_timeout,
    };
    use sonimesh_network::{DesktopTlsError, client_config, connect_pinned_client, server_config};

    #[test]
    fn control_listen_debug_redacts_backend_and_pairing_details() {
        let socket =
            ControlListenError::Socket(std::io::Error::other("127.0.0.1:43123 private-endpoint"));
        let tls = ControlListenError::Tls(DesktopTlsError::Io(std::io::Error::other(
            "certificate-private-key-material",
        )));
        let handshake = ControlListenError::Handshake(super::PairingHandshakeError::ProofRejected);

        let socket_debug = format!("{socket:?}");
        let tls_debug = format!("{tls:?}");
        let handshake_debug = format!("{handshake:?}");

        assert!(socket_debug.contains("kind: Other"));
        assert!(tls_debug.contains("category: \"io\""));
        assert!(handshake_debug.contains("category: \"proof_rejected\""));
        for debug in [socket_debug, tls_debug, handshake_debug] {
            assert!(!debug.contains("43123"));
            assert!(!debug.contains("private-endpoint"));
            assert!(!debug.contains("certificate-private-key-material"));
        }
    }

    #[test]
    fn nonblocking_pairing_listener_reports_empty_then_accepts() {
        let listener = PairingListener::bind("127.0.0.1:0").expect("listener binds");
        assert!(
            listener
                .try_accept()
                .expect("empty poll succeeds")
                .is_none()
        );

        let client = TcpStream::connect(listener.local_addr().expect("listener address"))
            .expect("client connects");
        let mut accepted = None;
        for _ in 0..1_000 {
            if let Some(stream) = listener.try_accept().expect("accept poll succeeds") {
                accepted = Some(stream);
                break;
            }
            thread::yield_now();
        }

        assert!(accepted.is_some(), "queued peer was not accepted");
        drop(client);
    }

    #[test]
    fn stalled_peer_times_out_before_proof_admission() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener binds");
        let address = listener.local_addr().expect("listener address");
        let certified_key =
            generate_simple_self_signed(vec!["localhost".to_owned()]).expect("certificate");
        let certificate = certified_key.cert.der().clone();
        let mut private_key_der = Zeroizing::new(certified_key.key_pair.serialize_der());
        let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(private_key_der.to_vec()));
        private_key_der.zeroize();
        let tls = Arc::new(server_config(certificate, private_key).expect("server config"));

        let server = thread::spawn(move || {
            let mut pairing = PairingInvitationController::<4>::new();
            let mut session = SessionState::IDLE;
            let empty_payload = [];
            let inputs = InvitationInputs {
                invitation: PairingId::new([0x31; 16]),
                pairing_secret: [0x32; 32],
                scanned_payload: &empty_payload,
            };
            let (stream, _) = listener.accept().expect("peer connects");
            accept_pairing_stream_with_timeout(
                stream,
                tls,
                &mut pairing,
                &mut session,
                0,
                PairingSource::new([0x33; 16]),
                &inputs,
                Duration::from_millis(100),
                Instant::now(),
            )
        });

        let client = TcpStream::connect(address).expect("client connects");
        let result = server.join().expect("timeout worker does not panic");
        drop(client);
        assert!(matches!(
            result,
            Err(ControlListenError::Tls(DesktopTlsError::Io(error)))
                if matches!(error.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock)
        ));
    }

    #[test]
    fn authenticated_peer_times_out_before_sending_the_proof_frame() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener binds");
        let address = listener.local_addr().expect("listener address");
        let invitation = PairingId::new([0x41; 16]);
        let certified_key =
            generate_simple_self_signed(vec!["localhost".to_owned()]).expect("certificate");
        let certificate = certified_key.cert.der().clone();
        let spki_pin = certified_key.key_pair.public_key_der();
        let mut private_key_der = Zeroizing::new(certified_key.key_pair.serialize_der());
        let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(private_key_der.to_vec()));
        private_key_der.zeroize();
        let tls = Arc::new(server_config(certificate, private_key).expect("server config"));

        let server = thread::spawn(move || {
            let mut pairing = PairingInvitationController::<4>::new();
            let mut session = SessionState::IDLE;
            let empty_payload = [];
            let inputs = InvitationInputs {
                invitation,
                pairing_secret: [0x42; 32],
                scanned_payload: &empty_payload,
            };
            let (stream, _) = listener.accept().expect("peer connects");
            accept_pairing_stream_with_timeout(
                stream,
                tls,
                &mut pairing,
                &mut session,
                0,
                PairingSource::new([0x43; 16]),
                &inputs,
                Duration::from_millis(100),
                Instant::now(),
            )
        });

        let config = client_config(spki_pin.clone()).expect("client config");
        let (_channel, _exporter) = connect_pinned_client(
            config,
            "localhost",
            &invitation.as_bytes(),
            TcpStream::connect(address).expect("client connects"),
        )
        .expect("TLS handshake completes");
        let result = server.join().expect("timeout worker does not panic");
        assert!(matches!(
            result,
            Err(ControlListenError::PeerFrameIo(error))
                if matches!(error.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock)
        ));
    }

    #[test]
    fn trickled_tls_record_cannot_extend_exchange_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener binds");
        let address = listener.local_addr().expect("listener address");
        let key = generate_simple_self_signed(vec!["localhost".to_owned()]).expect("certificate");
        let certificate = key.cert.der().clone();
        let mut private = Zeroizing::new(key.key_pair.serialize_der());
        let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(private.to_vec()));
        private.zeroize();
        let tls = Arc::new(server_config(certificate, private_key).expect("server config"));
        let (result_sender, result_receiver) = mpsc::channel();
        let server = thread::spawn(move || {
            let mut pairing = PairingInvitationController::<4>::new();
            let mut session = SessionState::IDLE;
            let inputs = InvitationInputs {
                invitation: PairingId::new([0x51; 16]),
                pairing_secret: [0x52; 32],
                scanned_payload: &[],
            };
            let (stream, _) = listener.accept().expect("peer connects");
            let result = accept_pairing_stream_with_timeout(
                stream,
                tls,
                &mut pairing,
                &mut session,
                0,
                PairingSource::new([0x53; 16]),
                &inputs,
                Duration::from_millis(400),
                Instant::now(),
            );
            result_sender
                .send((result, session))
                .expect("receiver remains");
        });
        let mut client = TcpStream::connect(address).expect("client connects");
        // A TLS handshake record advertising 255 bytes, followed slowly by
        // an incomplete handshake. Each write arrives within the old timeout.
        client
            .write_all(&[0x16, 0x03, 0x01, 0x00, 0xff])
            .expect("header");
        let writer = thread::spawn(move || {
            for _ in 0..30 {
                if client.write_all(&[0x01]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(60));
            }
        });
        let (result, session) = result_receiver
            .recv_timeout(Duration::from_millis(1100))
            .expect("absolute deadline terminates a trickling peer");
        assert!(
            matches!(result, Err(ControlListenError::Tls(DesktopTlsError::Io(error)))
            if matches!(error.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock))
        );
        assert_eq!(session, SessionState::IDLE);
        server.join().expect("server joins");
        writer.join().expect("writer joins");
    }

    #[test]
    fn trickled_proof_cannot_extend_exchange_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener binds");
        let address = listener.local_addr().expect("listener address");
        let invitation = PairingId::new([0x61; 16]);
        let key = generate_simple_self_signed(vec!["localhost".to_owned()]).expect("certificate");
        let certificate = key.cert.der().clone();
        let pin = key.key_pair.public_key_der();
        let mut private = Zeroizing::new(key.key_pair.serialize_der());
        let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(private.to_vec()));
        private.zeroize();
        let tls = Arc::new(server_config(certificate, private_key).expect("server config"));
        let (result_sender, result_receiver) = mpsc::channel();
        let server = thread::spawn(move || {
            let mut pairing = PairingInvitationController::<4>::new();
            let mut session = SessionState::IDLE;
            let inputs = InvitationInputs {
                invitation,
                pairing_secret: [0x62; 32],
                scanned_payload: &[],
            };
            let (stream, _) = listener.accept().expect("peer connects");
            let result = accept_pairing_stream_with_timeout(
                stream,
                tls,
                &mut pairing,
                &mut session,
                0,
                PairingSource::new([0x63; 16]),
                &inputs,
                Duration::from_millis(400),
                Instant::now(),
            );
            result_sender
                .send((result, session))
                .expect("receiver remains");
        });
        let (mut channel, _) = connect_pinned_client(
            client_config(pin).expect("client config"),
            "localhost",
            &invitation.as_bytes(),
            TcpStream::connect(address).expect("client connects"),
        )
        .expect("TLS handshake completes");
        let writer = thread::spawn(move || {
            for _ in 0..30 {
                if channel
                    .write_all(&[0x01])
                    .and_then(|()| channel.flush())
                    .is_err()
                {
                    break;
                }
                thread::sleep(Duration::from_millis(60));
            }
        });
        let (result, session) = result_receiver
            .recv_timeout(Duration::from_millis(1100))
            .expect("absolute deadline terminates a trickling peer");
        assert!(matches!(result, Err(ControlListenError::PeerFrameIo(error))
            if matches!(error.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock)));
        assert_eq!(session, SessionState::IDLE);
        server.join().expect("server joins");
        writer.join().expect("writer joins");
    }
}
