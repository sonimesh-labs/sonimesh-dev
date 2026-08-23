//! Authenticated control-plane transport over an already-pinned TLS channel.
//!
//! ADR 0008 first pairing establishes the pinned TLS 1.3 connection and the
//! pairing binding derives role-separated transcript proofs from its
//! exporter. This module is the next link in that chain: a type-state
//! wrapper around the established stream that requires the ADR 0019 proof
//! exchange before it exposes the bounded control-envelope framing.
//!
//! [`AuthenticatedControlHandshake`] owns the pre-authentication state. Its
//! [`verify_peer_frame`](AuthenticatedControlHandshake::verify_peer_frame)
//! accepts exactly one canonical peer frame carrying this invitation's
//! pairing ID and the opposite role byte, verifies the carried proof in
//! constant time through the caller-built [`PairingBinding`], and returns
//! the transcript-bound media root together with the exact reply frame to
//! write back. Only after that verification succeeds does
//! [`into_framing`](AuthenticatedControlHandshake::into_framing) hand back
//! an [`AuthenticatedControlStream`], which forwards every call to the
//! bounded [`UntrustedControlStream`] framing.
//!
//! Because the framed state is reachable only through a verified exchange,
//! a caller cannot accidentally read or write control envelopes over an
//! unauthenticated channel: the unverified wrapper simply has no envelope
//! methods. This module still performs no TLS, no certificate validation,
//! no pairing-record consumption, and no attempt accounting; those stay
//! owned by the platform TLS provider and the application-domain pairing
//! owner respectively.

use core::fmt;
use std::io::{self, Read, Write};

use sonimesh_protocol::{
    PROOF_FRAME_LEN, PairingBinding, PairingProofFrame, PairingRole, SessionSecret,
    parse_pairing_proof_frame,
};

use crate::untrusted_control_stream::{
    UntrustedControlError, UntrustedControlPoll, UntrustedControlStream,
};

/// Failure while authenticating or using an authenticated control transport.
#[derive(Debug)]
pub enum AuthenticatedControlError {
    /// The pinned channel's exporter has a noncanonical length.
    InvalidExporterLength,
    /// The peer's proof frame failed its bounded wire validation.
    ProofFrame(sonimesh_protocol::PairingProofFrameError),
    /// The peer's frame carried another invitation's pairing ID.
    InvitationMismatch,
    /// The peer's frame carried this endpoint's own role instead of the
    /// opposite role (a reflected proof).
    WrongRole,
    /// The peer's proof did not verify against the transcript binding.
    ProofRejected,
    /// Framing or socket failure after the authenticated session started.
    Framing(UntrustedControlError),
}

impl fmt::Display for AuthenticatedControlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidExporterLength => {
                formatter.write_str("TLS exporter must be exactly 32 bytes")
            }
            Self::ProofFrame(error) => {
                write!(formatter, "peer proof frame was rejected: {error}")
            }
            Self::InvitationMismatch => {
                formatter.write_str("peer proof frame does not match this invitation")
            }
            Self::WrongRole => {
                formatter.write_str("peer proof frame carries a reflected role byte")
            }
            Self::ProofRejected => formatter.write_str("peer pairing proof was rejected"),
            Self::Framing(error) => {
                write!(formatter, "authenticated control stream failed: {error}")
            }
        }
    }
}

impl std::error::Error for AuthenticatedControlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ProofFrame(error) => Some(error),
            Self::Framing(error) => Some(error),
            _ => None,
        }
    }
}

/// One direction of a completed pairing proof exchange.
///
/// Returned by [`AuthenticatedControlHandshake::verify_peer_frame`] when the
/// peer's frame verified. It carries the transcript-bound media root so the
/// caller can provision the allocator without re-deriving anything, plus the
/// exact reply frame to write back over the pinned channel.
pub struct VerifiedPeerProof {
    media_root: SessionSecret,
    reply_frame: PairingProofFrame,
}

impl VerifiedPeerProof {
    /// Returns the one-time media root bound to the authenticated transcript.
    #[must_use]
    pub const fn media_root(&self) -> &SessionSecret {
        &self.media_root
    }

    /// Returns the exact reply frame to send back inside the pinned channel.
    #[must_use]
    pub const fn reply_frame(&self) -> &PairingProofFrame {
        &self.reply_frame
    }
}

impl fmt::Debug for VerifiedPeerProof {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedPeerProof")
            .field("media_root", &"[REDACTED]")
            .field("reply_frame", &"[REDACTED]")
            .finish()
    }
}

/// Pre-authentication state of one pinned control channel.
///
/// The wrapper exists so the envelope methods of
/// [`AuthenticatedControlStream`] are unreachable until
/// [`Self::verify_peer_frame`] succeeded for the peer's canonical frame.
pub struct AuthenticatedControlHandshake<S> {
    stream: S,
    binding: Option<PairingBinding>,
    pairing_id: [u8; 16],
    local_role: PairingRole,
    verified: bool,
    reply_wire: [u8; PROOF_FRAME_LEN],
}

impl<S> AuthenticatedControlHandshake<S> {
    /// Wraps an established pinned-channel stream for one pairing identity.
    ///
    /// The binding must already be derived from the scanned invitation and
    /// the live TLS exporter; this wrapper never builds one from raw secrets.
    ///
    /// # Errors
    ///
    /// Returns [`AuthenticatedControlError::InvalidExporterLength`] when the
    /// binding was built with anything other than the exact 32-byte export.
    pub fn new(
        stream: S,
        binding: Result<PairingBinding, sonimesh_protocol::PairingBindingError>,
        pairing_id: [u8; 16],
        local_role: PairingRole,
    ) -> Result<Self, AuthenticatedControlError> {
        // Any binding-construction failure means the exporter or protocol
        // version was noncanonical; the wrapper cannot exist without one.
        let binding = binding.map_err(|_| AuthenticatedControlError::InvalidExporterLength)?;
        Ok(Self {
            stream,
            binding: Some(binding),
            pairing_id,
            local_role,
            verified: false,
            reply_wire: [0_u8; PROOF_FRAME_LEN],
        })
    }

    /// Returns whether the peer's proof frame has verified.
    #[must_use]
    pub const fn is_verified(&self) -> bool {
        self.verified
    }
}

impl<S: Read + Write> AuthenticatedControlHandshake<S> {
    /// Reads exactly one peer proof frame from the pinned channel.
    ///
    /// This blocks until the whole frame arrives; callers drive it on their
    /// established connection thread after the TLS handshake completed, as
    /// the provider-spike loopback does.
    ///
    /// # Errors
    ///
    /// Returns an operating-system error when the channel closes early or a
    /// socket operation fails.
    pub fn read_peer_frame(&mut self) -> io::Result<[u8; PROOF_FRAME_LEN]> {
        let mut wire = [0_u8; PROOF_FRAME_LEN];
        self.stream.read_exact(&mut wire)?;
        Ok(wire)
    }

    /// Validates and verifies the received peer frame.
    ///
    /// The frame must decode canonically, carry this invitation's pairing
    /// ID, present the opposite role byte, and carry a proof that verifies in
    /// constant time under the transcript binding. On success the reply
    /// frame is rendered and remembered; [`Self::send_reply_frame`] writes
    /// it and [`Self::into_framing`] unlocks the envelope methods.
    ///
    /// # Errors
    ///
    /// Returns [`AuthenticatedControlError`] for every rejection shape;
    /// a rejected exchange leaves the wrapper unverified, so the envelope
    /// state stays unreachable.
    pub fn verify_peer_frame(
        &mut self,
        wire: &[u8],
    ) -> Result<VerifiedPeerProof, AuthenticatedControlError> {
        let Some(binding) = self.binding.as_ref() else {
            return Err(AuthenticatedControlError::InvalidExporterLength);
        };
        let frame =
            parse_pairing_proof_frame(wire).map_err(AuthenticatedControlError::ProofFrame)?;
        if frame.pairing_id != self.pairing_id {
            return Err(AuthenticatedControlError::InvitationMismatch);
        }
        if frame.role == self.local_role {
            return Err(AuthenticatedControlError::WrongRole);
        }

        let verified = binding
            .verify_peer_proof(self.local_role, &frame.proof)
            .map_err(|_| AuthenticatedControlError::InvalidExporterLength)?;
        if !verified {
            return Err(AuthenticatedControlError::ProofRejected);
        }

        let media_root = binding
            .media_root()
            .map_err(|_| AuthenticatedControlError::InvalidExporterLength)?;
        let reply_frame = PairingProofFrame {
            protocol_version: frame.protocol_version,
            role: self.local_role,
            pairing_id: self.pairing_id,
            proof: binding
                .local_proof(self.local_role)
                .map_err(|_| AuthenticatedControlError::InvalidExporterLength)?,
        };
        reply_frame.encode_into(&mut self.reply_wire).map_err(|_| {
            AuthenticatedControlError::ProofFrame(
                sonimesh_protocol::PairingProofFrameError::Truncated,
            )
        })?;
        self.verified = true;
        Ok(VerifiedPeerProof {
            media_root,
            reply_frame,
        })
    }

    /// Writes the rendered reply frame into the pinned channel.
    ///
    /// # Errors
    ///
    /// Returns an operating-system error when the channel rejects the write,
    /// or [`AuthenticatedControlError::WrongRole`] is never produced here;
    /// an unverified wrapper fails with
    /// [`io::ErrorKind::PermissionDenied`] instead of writing anything.
    pub fn send_reply_frame(&mut self) -> Result<(), AuthenticatedControlError> {
        if !self.verified {
            return Err(AuthenticatedControlError::ProofRejected);
        }
        self.stream
            .write_all(&self.reply_wire)
            .and_then(|()| self.stream.flush())
            .map_err(io::Error::other)
            .map_err(|error| {
                // Preserve ordinary socket failures as framing errors.
                AuthenticatedControlError::Framing(UntrustedControlError::Io(error))
            })
    }

    /// Consumes the verified handshake and unlocks the envelope framing.
    ///
    /// # Errors
    ///
    /// Returns [`AuthenticatedControlError::ProofRejected`] when the peer's
    /// frame has not verified; the stream is dropped either way, so a failed
    /// pairing can never leave a usable control channel behind.
    pub fn into_framing(
        mut self,
    ) -> Result<AuthenticatedControlStream<S>, AuthenticatedControlError> {
        if !self.verified {
            return Err(AuthenticatedControlError::ProofRejected);
        }
        self.binding.take();
        Ok(AuthenticatedControlStream {
            inner: UntrustedControlStream::from_io(self.stream),
        })
    }
}

impl<S> fmt::Debug for AuthenticatedControlHandshake<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthenticatedControlHandshake")
            .field("stream", &"[REDACTED]")
            .field("binding", &"[REDACTED]")
            .field("pairing_id", &"[REDACTED]")
            .field("local_role", &self.local_role)
            .field("verified", &self.verified)
            .finish_non_exhaustive()
    }
}

/// Post-authentication control-envelope transport.
///
/// Every method forwards to the bounded, semantically validating
/// [`UntrustedControlStream`] framing. This type is constructible only
/// through a verified [`AuthenticatedControlHandshake`], so its existence
/// witnesses that the pinned-channel peer proved the pairing transcript.
pub struct AuthenticatedControlStream<S> {
    inner: UntrustedControlStream<S>,
}

impl<S: Read + Write> AuthenticatedControlStream<S> {
    /// Queues and advances one semantically validated control frame.
    ///
    /// # Errors
    ///
    /// Forwards [`UntrustedControlError`] from the underlying framing.
    pub fn send(
        &mut self,
        envelope: &sonimesh_protocol::Envelope,
    ) -> Result<crate::untrusted_control_stream::UntrustedControlWrite, UntrustedControlError> {
        self.inner.send(envelope)
    }

    /// Advances a previously queued frame without waiting for capacity.
    ///
    /// # Errors
    ///
    /// Forwards [`UntrustedControlError`] from the underlying framing.
    pub fn flush_write(
        &mut self,
    ) -> Result<crate::untrusted_control_stream::UntrustedControlWrite, UntrustedControlError> {
        self.inner.flush_write()
    }

    /// Reads at most one complete frame without waiting for network data.
    ///
    /// # Errors
    ///
    /// Forwards [`UntrustedControlError`] from the underlying framing.
    pub fn poll(&mut self) -> Result<UntrustedControlPoll, UntrustedControlError> {
        self.inner.poll()
    }
}

impl<S> fmt::Debug for AuthenticatedControlStream<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthenticatedControlStream([REDACTED])")
    }
}

/// Builds a binding for one side of the pinned channel from public inputs.
///
/// Convenience for platform owners: identical to
/// [`PairingBinding::new`] but mapping errors onto this module's type so a
/// call site cannot mix error vocabularies.
///
/// # Errors
///
/// Returns [`AuthenticatedControlError::InvalidExporterLength`] unless the
/// exporter is exactly [`PAIRING_TLS_EXPORTER_LEN`] bytes and the protocol
/// version is supported.
pub fn binding_for_channel(
    protocol_version: u8,
    pairing_id: [u8; 16],
    pairing_secret: [u8; 32],
    tls_exporter: &[u8],
) -> Result<PairingBinding, AuthenticatedControlError> {
    PairingBinding::new(protocol_version, pairing_id, pairing_secret, tls_exporter)
        .map_err(|_| AuthenticatedControlError::InvalidExporterLength)
}

#[cfg(test)]
mod tests {
    use std::io::{self, Read, Write};

    use super::{
        AuthenticatedControlError, AuthenticatedControlHandshake, VerifiedPeerProof,
        binding_for_channel,
    };
    use sonimesh_protocol::{Envelope, Hello, PairingRole, envelope, parse_pairing_proof_frame};

    use crate::untrusted_control_stream::UntrustedControlWrite;

    const PAIRING_ID: [u8; 16] = [
        0x5A, 0x5B, 0x5C, 0x5D, 0x5E, 0x5F, 0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
        0x69,
    ];
    const PAIRING_SECRET: [u8; 32] = [0xA7; 32];
    const EXPORTER: [u8; 32] = [0xC3; 32];

    /// One connected pair of in-memory duplex pipes plus both role wrappers.
    struct Channel {
        desktop: AuthenticatedControlHandshake<Duplex>,
        android: AuthenticatedControlHandshake<Duplex>,
    }

    /// Bidirectional in-memory pipe: each half reads what the other writes.
    #[derive(Default)]
    struct Duplex {
        inbox: std::io::Cursor<Vec<u8>>,
        outbox: Vec<u8>,
    }

    impl Read for Duplex {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            Read::read(&mut self.inbox, buffer)
        }
    }

    impl Write for Duplex {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.outbox.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Connects two duplex halves so `a.outbox` feeds `b.inbox` and vice versa.
    fn connect(a: &mut Duplex, b: &mut Duplex) {
        let a_out = core::mem::take(&mut a.outbox);
        let b_out = core::mem::take(&mut b.outbox);
        a.inbox.get_mut().extend_from_slice(&b_out);
        b.inbox.get_mut().extend_from_slice(&a_out);
    }

    fn channel() -> Channel {
        let (mut desktop_io, mut android_io) = (Duplex::default(), Duplex::default());
        // Pre-connect: move any pending output between the two halves.
        connect(&mut desktop_io, &mut android_io);

        let desktop_binding =
            binding_for_channel(1, PAIRING_ID, PAIRING_SECRET, &EXPORTER).unwrap();
        let android_binding =
            binding_for_channel(1, PAIRING_ID, PAIRING_SECRET, &EXPORTER).unwrap();
        Channel {
            desktop: AuthenticatedControlHandshake::new(
                desktop_io,
                Ok(desktop_binding),
                PAIRING_ID,
                PairingRole::DesktopServer,
            )
            .unwrap(),
            android: AuthenticatedControlHandshake::new(
                android_io,
                Ok(android_binding),
                PAIRING_ID,
                PairingRole::AndroidClient,
            )
            .unwrap(),
        }
    }

    fn hello_envelope() -> Envelope {
        Envelope {
            protocol_version: 1,
            request_id: 42,
            payload: Some(envelope::Payload::Hello(Hello {
                supported_versions: vec![1],
                capabilities: vec![],
                display_name: "lab peer".to_owned(),
            })),
        }
    }

    #[test]
    fn verified_exchange_unlocks_the_framed_state_and_matches_roots() {
        use std::io::Write as _;

        let mut channel = channel();

        // Android derives its client proof, encodes the canonical frame, and
        // writes it into its outbox; desktop then reads it from its inbox.
        {
            let binding = binding_for_channel(1, PAIRING_ID, PAIRING_SECRET, &EXPORTER).unwrap();
            let client_frame = sonimesh_protocol::PairingProofFrame {
                protocol_version: 1,
                role: PairingRole::AndroidClient,
                pairing_id: PAIRING_ID,
                proof: binding.local_proof(PairingRole::AndroidClient).unwrap(),
            };
            let mut wire = [0_u8; 55];
            client_frame.encode_into(&mut wire).unwrap();
            channel.android.stream.write_all(&wire).unwrap();
        }
        // Move the android outbox into the desktop inbox.
        {
            let sent = core::mem::take(&mut channel.android.stream.outbox);
            channel
                .desktop
                .stream
                .inbox
                .get_mut()
                .extend_from_slice(&sent);
        }

        // The desktop verifies the peer frame and renders its reply.
        let peer_wire = channel.desktop.read_peer_frame().unwrap();
        let verified = channel
            .desktop
            .verify_peer_frame(&peer_wire)
            .expect("a correct client frame verifies");
        channel.desktop.send_reply_frame().expect("reply writes");

        // Move the desktop outbox into the android inbox and verify there.
        {
            let sent = core::mem::take(&mut channel.desktop.stream.outbox);
            channel
                .android
                .stream
                .inbox
                .get_mut()
                .extend_from_slice(&sent);
        }
        let server_wire = channel.android.read_peer_frame().unwrap();
        let decoded = parse_pairing_proof_frame(&server_wire).unwrap();
        assert_eq!(decoded.role, PairingRole::DesktopServer);

        let android_binding =
            binding_for_channel(1, PAIRING_ID, PAIRING_SECRET, &EXPORTER).unwrap();
        assert!(
            android_binding
                .verify_peer_proof(PairingRole::AndroidClient, &decoded.proof)
                .unwrap()
        );

        // Both sides now hold the identical transcript media root: seal one
        // datagram under each root and require byte equality.
        let context = sonimesh_protocol::MediaKeyContext::new(
            1,
            1,
            0,
            sonimesh_protocol::MediaDirection::AndroidToDesktop,
        )
        .unwrap();
        let mut left = sonimesh_protocol::MediaSealer::new(verified.media_root(), context).unwrap();
        let expected_root = android_binding.media_root().unwrap();
        let mut right = sonimesh_protocol::MediaSealer::new(&expected_root, context).unwrap();
        assert_eq!(
            left.seal(0, 1_000, &[7_u8; 16]).unwrap().datagram,
            right.seal(0, 1_000, &[7_u8; 16]).unwrap().datagram
        );
        let VerifiedPeerProof { .. } = verified;

        // Both wrappers are now verified and can transition to framing.
        assert!(channel.desktop.is_verified());
        assert!(!channel.android.is_verified());
        let mut framed_desktop = channel.desktop.into_framing().expect("desktop unlocks");

        // The framed stream forwards bounded envelope I/O.
        let write = framed_desktop.send(&hello_envelope()).unwrap();
        assert_eq!(write, UntrustedControlWrite::Complete);
    }

    #[test]
    fn unverified_handshake_cannot_reach_the_framed_state() {
        let mut channel = channel();
        let foreign = AuthenticatedControlHandshake::new(
            Duplex::default(),
            Err(sonimesh_protocol::PairingBindingError::InvalidExporterLength),
            PAIRING_ID,
            PairingRole::DesktopServer,
        )
        .unwrap_err();
        assert!(matches!(
            foreign,
            AuthenticatedControlError::InvalidExporterLength
        ));

        assert!(!channel.desktop.is_verified());
        assert!(matches!(
            channel.desktop.verify_peer_frame(&[0_u8; 55]),
            Err(AuthenticatedControlError::ProofFrame(_))
        ));
        assert!(matches!(
            channel.desktop.into_framing(),
            Err(AuthenticatedControlError::ProofRejected)
        ));
    }

    #[test]
    fn reflected_frames_are_rejected_at_the_role_boundary() {
        let mut channel = channel();
        // Build a canonical frame carrying the desktop's own local proof:
        // its role byte equals the wrapper's local role.
        let binding = binding_for_channel(1, PAIRING_ID, PAIRING_SECRET, &EXPORTER).unwrap();
        let reflected = sonimesh_protocol::PairingProofFrame {
            protocol_version: 1,
            role: PairingRole::DesktopServer,
            pairing_id: PAIRING_ID,
            proof: binding.local_proof(PairingRole::DesktopServer).unwrap(),
        };
        let mut wire = [0_u8; 55];
        reflected.encode_into(&mut wire).unwrap();

        assert!(matches!(
            channel.desktop.verify_peer_frame(&wire),
            Err(AuthenticatedControlError::WrongRole)
        ));
        assert!(!channel.desktop.is_verified());
    }

    #[test]
    fn foreign_invitation_frames_are_rejected_before_derivation() {
        let mut channel = channel();
        let binding = binding_for_channel(1, PAIRING_ID, PAIRING_SECRET, &EXPORTER).unwrap();
        let foreign = sonimesh_protocol::PairingProofFrame {
            protocol_version: 1,
            role: PairingRole::AndroidClient,
            pairing_id: [0x77; 16],
            proof: binding.local_proof(PairingRole::AndroidClient).unwrap(),
        };
        let mut wire = [0_u8; 55];
        foreign.encode_into(&mut wire).unwrap();

        assert!(matches!(
            channel.desktop.verify_peer_frame(&wire),
            Err(AuthenticatedControlError::InvitationMismatch)
        ));
        assert!(!channel.desktop.is_verified());
    }

    #[test]
    fn wrong_proofs_fail_closed_without_unlocking_the_channel() {
        let mut channel = channel();
        let other_binding =
            binding_for_channel(1, PAIRING_ID, PAIRING_SECRET, &[0xEE; 32]).unwrap();
        let wrong = sonimesh_protocol::PairingProofFrame {
            protocol_version: 1,
            role: PairingRole::AndroidClient,
            pairing_id: PAIRING_ID,
            proof: other_binding
                .local_proof(PairingRole::AndroidClient)
                .unwrap(),
        };
        let mut wire = [0_u8; 55];
        wrong.encode_into(&mut wire).unwrap();

        assert!(matches!(
            channel.desktop.verify_peer_frame(&wire),
            Err(AuthenticatedControlError::ProofRejected)
        ));
        assert!(!channel.desktop.is_verified());
        assert!(matches!(
            channel.desktop.send_reply_frame(),
            Err(AuthenticatedControlError::ProofRejected)
        ));
    }
}
