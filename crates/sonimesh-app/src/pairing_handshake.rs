//! Desktop-side pairing handshake composition over real protocol components.
//!
//! This module composes the boundaries that already exist individually:
//! [`parse_pairing_qr_payload`] validates the scanned invitation,
//! [`PairingBinding`] derives the role-separated proofs and the one-time
//! media root, and [`PairingInvitationController`] owns the bounded
//! invitation lifecycle that admits exactly one transcript-bound proof.
//! The composition exists so a platform pairing owner cannot accidentally
//! skip a step: trust is established only when the decoded QR view, the
//! externally supplied TLS exporter bytes, and the peer's proof all agree
//! inside one admitted attempt. It performs no TLS, no network I/O, and no
//! persistent identity storage; the exporter must come from the pinned TLS
//! 1.3 connection described by ADR 0008.

use core::fmt;

use sonimesh_protocol::{
    PAIRING_TLS_EXPORTER_LEN, PairingBinding, PairingBindingError, PairingQrPayload, PairingRole,
    SessionSecret, parse_pairing_qr_payload,
};

use crate::{
    PairingAttemptOutcome, PairingId, PairingInvitationController, PairingInvitationError,
    PairingProofVerification, PairingRecordState, PairingSource, SessionPhase, SessionState,
};

/// Failure while composing a desktop-side pairing handshake.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingHandshakeError {
    /// The scanned QR payload failed its bounded validation.
    Qr(sonimesh_protocol::QrPayloadError),
    /// The scanned invitation does not match the active invitation record.
    InvitationMismatch,
    /// The pairing binding rejected an input length or version.
    Binding(PairingBindingError),
    /// The invitation owner or shared lifecycle rejected the operation.
    Invitation(PairingInvitationError),
    /// The peer proof did not verify for the expected role.
    ProofRejected,
    /// The TLS exporter could not be used to derive the binding.
    InvalidExporterLength,
}

impl fmt::Display for PairingHandshakeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Qr(error) => write!(formatter, "pairing QR payload was rejected: {error}"),
            Self::InvitationMismatch => {
                formatter.write_str("scanned invitation does not match the active invitation")
            }
            Self::Binding(error) => {
                write!(formatter, "pairing binding derivation failed: {error}")
            }
            Self::Invitation(error) => {
                write!(
                    formatter,
                    "pairing invitation rejected the handshake: {error}"
                )
            }
            Self::ProofRejected => formatter.write_str("peer pairing proof was rejected"),
            Self::InvalidExporterLength => {
                formatter.write_str("TLS exporter must be exactly 32 bytes")
            }
        }
    }
}

impl std::error::Error for PairingHandshakeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Qr(error) => Some(error),
            Self::Binding(error) => Some(error),
            Self::Invitation(error) => Some(error),
            _ => None,
        }
    }
}

/// Outcome of one composed desktop-side pairing handshake.
///
/// `Ready` carries the one-time media root derived from the same exporter
/// transcript that authenticated the peer proof; it becomes the
/// [`crate::MediaSessionAllocator`] root once the trusted session starts
/// streaming. Rejected outcomes keep the invitation active with its bounded
/// remaining attempts.
pub enum PairingHandshakeOutcome {
    /// The proof verified; the session is trusted and the media root is bound.
    Ready {
        /// One-time media root for the ADR 0005/0017 allocator boundary.
        media_root: SessionSecret,
    },
    /// The proof was well-formed but invalid; attempts remain bounded.
    Rejected {
        /// Failed attempts this source may still submit before rate limiting.
        attempts_remaining: u8,
    },
}

impl fmt::Debug for PairingHandshakeOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ready { .. } => formatter
                .debug_struct("Ready")
                .field("media_root", &"[REDACTED]")
                .finish(),
            Self::Rejected { attempts_remaining } => formatter
                .debug_struct("Rejected")
                .field("attempts_remaining", attempts_remaining)
                .finish(),
        }
    }
}

/// Maximum pairing sources one handshake-composed invitation may admit.
///
/// Matches the single-invitation MVP: one desktop invitation, one bounded
/// source table shared with the existing controller boundary.
pub const MAX_HANDSHAKE_SOURCES: usize = 4;

/// Everything the desktop renders into one pairing QR image.
///
/// The pairing secret must come from a cryptographically secure random
/// source and is rendered exactly once; it never persists beyond the
/// invitation lifetime. The SPKI is the exact DER `SubjectPublicKeyInfo`
/// the desktop's TLS certificate will present.
#[derive(Clone, Copy)]
pub struct PairingQrInvitation<'a> {
    /// One-time 128-bit invitation identifier.
    pub id: PairingId,
    /// One-time 256-bit pairing secret.
    pub pairing_secret: [u8; 32],
    /// Desktop control endpoint host or IP literal.
    pub control_host: &'a str,
    /// Desktop control endpoint TCP port.
    pub control_port: u16,
    /// Exact DER desktop TLS SPKI bytes to pin.
    pub desktop_tls_spki: &'a [u8],
    /// Monotonic expiry in milliseconds for the pairing record.
    pub expires_at_ms: u64,
}

impl fmt::Debug for PairingQrInvitation<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PairingQrInvitation")
            .field("id", &"[REDACTED]")
            .field("pairing_secret", &"[REDACTED]")
            .field("control_host", &"[REDACTED]")
            .field("control_port", &self.control_port)
            .field("desktop_tls_spki_len", &self.desktop_tls_spki.len())
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

/// Begins an invitation and returns the exact QR payload bytes to render.
/// The payload is encoded with the same version-one layout the Android side
/// parses ([`parse_pairing_qr_payload`]), then decoded back as a self-check
/// so a rendered image can never drift from the record's identity. The
/// returned count is the exact number of bytes written to `payload_out`.
///
/// # Errors
///
/// Returns [`PairingHandshakeError`] when the fields are outside the
/// documented bounds, the buffer is too small, the decode self-check fails,
/// or the invitation owner rejects the begin transition.
pub fn begin_from_rendered_payload(
    pairing: &mut PairingInvitationController<MAX_HANDSHAKE_SOURCES>,
    session: &mut SessionState,
    now_ms: u64,
    lifetime_ms: u64,
    max_attempts_per_source: u8,
    invitation: PairingQrInvitation<'_>,
    payload_out: &mut [u8],
) -> Result<usize, PairingHandshakeError> {
    let id_bytes = invitation.id.as_bytes();
    let protocol_version = u8::try_from(sonimesh_protocol::CONTROL_PROTOCOL_VERSION)
        .map_err(|_| PairingHandshakeError::Binding(PairingBindingError::InvalidProtocolVersion))?;
    let payload = PairingQrPayload {
        protocol_version,
        pairing_id: id_bytes,
        pairing_secret: invitation.pairing_secret,
        control_host: invitation.control_host,
        control_port: invitation.control_port,
        desktop_tls_spki: invitation.desktop_tls_spki,
        expires_at_ms: invitation.expires_at_ms,
    };
    let written = payload
        .encode_into(payload_out)
        .map_err(PairingHandshakeError::Qr)?;
    let view =
        parse_pairing_qr_payload(&payload_out[..written]).map_err(PairingHandshakeError::Qr)?;
    if view.pairing_id != id_bytes {
        return Err(PairingHandshakeError::InvitationMismatch);
    }

    pairing
        .begin(
            session,
            invitation.id,
            now_ms,
            lifetime_ms,
            max_attempts_per_source,
        )
        .map_err(PairingHandshakeError::Invitation)?;
    Ok(written)
}

/// One admitted scan-and-verify attempt over an already-pinned TLS channel.
#[derive(Clone, Copy)]
pub struct ScannedProofAttempt<'a> {
    /// Raw QR payload bytes exactly as scanned.
    pub scanned_payload: &'a [u8],
    /// The invitation identity this scan must match.
    pub expected_pairing_id: PairingId,
    /// The one-time secret the desktop rendered into that invitation.
    pub pairing_secret: [u8; 32],
    /// The peer's proof received inside the pinned TLS channel.
    pub peer_proof: &'a [u8],
}

/// Admits one scanned proof attempt through the real binding and owner.
///
/// The caller supplies the raw QR bytes the scanner read, the TLS exporter
/// from its already-pinned TLS 1.3 connection, and the attempt inputs. The
/// composition decodes the scan, rejects a mismatched invitation before
/// deriving anything, builds the [`PairingBinding`] from the exporter
/// transcript, verifies the peer proof for the opposite role in constant
/// time through the binding (reflections fail), and charges exactly one
/// admitted attempt on the invitation record.
///
/// A verified proof consumes the invitation and yields the media root bound
/// to this transcript; any rejection keeps the invitation active until its
/// attempts or lifetime end.
///
/// # Errors
///
/// Returns [`PairingHandshakeError`] for a rejected scan, mismatched
/// invitation, invalid exporter or derivation input, or an admission failure
/// (expired, consumed, revoked, rate-limited, or table-full). A well-formed
/// but incorrect proof returns [`PairingHandshakeOutcome::Rejected`] instead
/// of an error, matching the record's distinction between an invalid proof
/// and a failed verifier.
pub fn verify_scanned_proof(
    pairing: &mut PairingInvitationController<MAX_HANDSHAKE_SOURCES>,
    session: &mut SessionState,
    now_ms: u64,
    source: PairingSource,
    tls_exporter: &[u8],
    attempt: ScannedProofAttempt<'_>,
) -> Result<PairingHandshakeOutcome, PairingHandshakeError> {
    if tls_exporter.len() != PAIRING_TLS_EXPORTER_LEN {
        return Err(PairingHandshakeError::InvalidExporterLength);
    }
    let view =
        parse_pairing_qr_payload(attempt.scanned_payload).map_err(PairingHandshakeError::Qr)?;
    let expected_id = attempt.expected_pairing_id.as_bytes();
    if view.pairing_id != expected_id {
        return Err(PairingHandshakeError::InvitationMismatch);
    }
    if pairing.state() != Some(PairingRecordState::Active) {
        return Err(PairingHandshakeError::Invitation(
            PairingInvitationError::MissingInvitation,
        ));
    }

    let binding = PairingBinding::new(
        view.protocol_version,
        view.pairing_id,
        attempt.pairing_secret,
        tls_exporter,
    )
    .map_err(PairingHandshakeError::Binding)?;

    // The desktop is the server role: the peer must present the client-role
    // proof. `verify_peer_proof` derives the opposite role internally, so a
    // reflected server proof cannot verify as its own peer proof.
    let outcome = pairing.verify_proof(session, now_ms, source, || {
        match binding.verify_peer_proof(PairingRole::DesktopServer, attempt.peer_proof) {
            Ok(true) => PairingProofVerification::Verified,
            Ok(false) => PairingProofVerification::Rejected,
            Err(_) => PairingProofVerification::Failed,
        }
    });

    match outcome {
        Ok(PairingAttemptOutcome::Paired) => {
            let media_root = binding
                .media_root()
                .map_err(PairingHandshakeError::Binding)?;
            debug_assert_eq!(
                session.phase(),
                SessionPhase::Ready,
                "the invitation owner must establish trust after a paired proof"
            );
            Ok(PairingHandshakeOutcome::Ready { media_root })
        }
        Ok(PairingAttemptOutcome::Rejected { attempts_remaining }) => {
            Ok(PairingHandshakeOutcome::Rejected { attempts_remaining })
        }
        Err(error) => Err(PairingHandshakeError::Invitation(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_HANDSHAKE_SOURCES, PairingHandshakeError, PairingHandshakeOutcome, PairingQrInvitation,
        ScannedProofAttempt, begin_from_rendered_payload, verify_scanned_proof,
    };
    use crate::{
        PairingId, PairingInvitationController, PairingRecordState, PairingSource, SessionPhase,
        SessionState,
    };
    use sonimesh_protocol::{MAX_QR_PAYLOAD_LEN, PAIRING_TLS_EXPORTER_LEN};

    const HOST: &str = "desktop-01.lab.local";
    const PORT: u16 = 47_610;
    const EXPIRY_MS: u64 = 2_000_000;

    fn id(byte: u8) -> PairingId {
        PairingId::new([byte; 16])
    }

    fn source(byte: u8) -> PairingSource {
        PairingSource::new([byte; 16])
    }

    struct Invitation {
        pairing: PairingInvitationController<MAX_HANDSHAKE_SOURCES>,
        session: SessionState,
        id: PairingId,
        secret: [u8; 32],
        payload: Vec<u8>,
    }

    fn invitation_inputs(id: PairingId, secret: [u8; 32]) -> PairingQrInvitation<'static> {
        PairingQrInvitation {
            id,
            pairing_secret: secret,
            control_host: HOST,
            control_port: PORT,
            desktop_tls_spki: &[0x40; 64],
            expires_at_ms: EXPIRY_MS,
        }
    }

    fn begin_invitation(now_ms: u64) -> Invitation {
        let mut pairing = PairingInvitationController::<MAX_HANDSHAKE_SOURCES>::new();
        let mut session = SessionState::IDLE;
        let id = id(0x21);
        let secret = test_pairing_secret();
        let mut payload_out = vec![0_u8; MAX_QR_PAYLOAD_LEN];
        let written = begin_from_rendered_payload(
            &mut pairing,
            &mut session,
            now_ms,
            10_000,
            3,
            invitation_inputs(id, secret),
            &mut payload_out,
        )
        .expect("a canonical invitation always encodes and begins");
        payload_out.truncate(written);
        Invitation {
            pairing,
            session,
            id,
            secret,
            payload: payload_out,
        }
    }

    fn exporter(seed: u8) -> Vec<u8> {
        vec![seed; PAIRING_TLS_EXPORTER_LEN]
    }

    const fn test_pairing_secret() -> [u8; 32] {
        let mut secret = [0_u8; 32];
        let mut byte = 0x30_u8;
        let mut index = 0;
        while index < secret.len() {
            secret[index] = byte;
            byte += 1;
            index += 1;
        }
        secret
    }

    #[test]
    fn rendered_payload_round_trips_into_the_same_invitation() {
        let invitation = begin_invitation(100);
        assert_eq!(invitation.pairing.state(), Some(PairingRecordState::Active));
        assert_eq!(invitation.session.phase(), SessionPhase::Pairing);
        assert!(invitation.payload.len() < MAX_QR_PAYLOAD_LEN);

        // The scanned bytes decode to the exact invitation identity.
        let view = sonimesh_protocol::parse_pairing_qr_payload(&invitation.payload).unwrap();
        assert_eq!(view.pairing_id, invitation.id.as_bytes());
        assert_eq!(view.control_host, HOST);
        assert_eq!(view.control_port, PORT);
    }

    #[test]
    fn a_correct_client_proof_establishes_trust_and_yields_the_media_root() {
        let mut invitation = begin_invitation(100);
        let exporter = exporter(0xA0);

        // The Android peer derives its proof from the same public vector
        // inputs: the pairing binding's client role under the desktop's
        // invitation secret and this exporter.
        let binding = sonimesh_protocol::PairingBinding::new(
            1,
            invitation.id.as_bytes(),
            invitation.secret,
            &exporter,
        )
        .expect("canonical handshake inputs derive a binding");
        let client_proof = binding
            .local_proof(sonimesh_protocol::PairingRole::AndroidClient)
            .expect("proof derivation succeeds");

        let outcome = verify_scanned_proof(
            &mut invitation.pairing,
            &mut invitation.session,
            150,
            source(0x44),
            &exporter,
            ScannedProofAttempt {
                scanned_payload: &invitation.payload,
                expected_pairing_id: invitation.id,
                pairing_secret: invitation.secret,
                peer_proof: &client_proof,
            },
        )
        .expect("a correct peer proof completes the handshake");

        let PairingHandshakeOutcome::Ready { media_root } = outcome else {
            panic!("a verified proof must yield the ready outcome");
        };
        assert_eq!(invitation.session.phase(), SessionPhase::Ready);
        assert_eq!(
            invitation.pairing.state(),
            Some(PairingRecordState::Consumed)
        );

        // The media root matches an independent derivation from the same
        // transcript, so the allocator receives the ADR 0005 root. The
        // SessionSecret type has no public byte accessor, so equality is
        // proven by sealing one packet under both roots and comparing the
        // complete datagrams byte for byte.
        let expected_root = binding.media_root().unwrap();
        let context = sonimesh_protocol::MediaKeyContext {
            session_id: 7,
            stream_id: 1,
            key_epoch: 0,
            direction: sonimesh_protocol::MediaDirection::AndroidToDesktop,
        };
        let plaintext = [0x5A_u8; 16];
        let sealed_probe = {
            let mut sealer = sonimesh_protocol::MediaSealer::new(&media_root, context).unwrap();
            sealer.seal(0, 1_000, &plaintext).unwrap()
        };
        let expected_sealed = {
            let mut sealer = sonimesh_protocol::MediaSealer::new(&expected_root, context).unwrap();
            sealer.seal(0, 1_000, &plaintext).unwrap()
        };
        assert_eq!(sealed_probe.datagram, expected_sealed.datagram);
    }

    #[test]
    fn a_wrong_proof_is_rejected_without_consuming_the_invitation() {
        let mut invitation = begin_invitation(100);
        let exporter = exporter(0xA0);
        let binding = sonimesh_protocol::PairingBinding::new(
            1,
            invitation.id.as_bytes(),
            invitation.secret,
            &exporter,
        )
        .unwrap();
        // The server-role proof is a reflected local proof and must fail.
        let reflected = binding
            .local_proof(sonimesh_protocol::PairingRole::DesktopServer)
            .unwrap();

        let outcome = verify_scanned_proof(
            &mut invitation.pairing,
            &mut invitation.session,
            150,
            source(0x44),
            &exporter,
            ScannedProofAttempt {
                scanned_payload: &invitation.payload,
                expected_pairing_id: invitation.id,
                pairing_secret: invitation.secret,
                peer_proof: &reflected,
            },
        )
        .expect("a well-formed wrong proof is a rejection, not an error");

        assert!(matches!(
            outcome,
            PairingHandshakeOutcome::Rejected {
                attempts_remaining: 2
            }
        ));
        assert_eq!(invitation.session.phase(), SessionPhase::Pairing);
        assert_eq!(invitation.pairing.state(), Some(PairingRecordState::Active));

        // The correct proof still completes within the remaining attempts.
        let client_proof = binding
            .local_proof(sonimesh_protocol::PairingRole::AndroidClient)
            .unwrap();
        assert!(matches!(
            verify_scanned_proof(
                &mut invitation.pairing,
                &mut invitation.session,
                160,
                source(0x44),
                &exporter,
                ScannedProofAttempt {
                    scanned_payload: &invitation.payload,
                    expected_pairing_id: invitation.id,
                    pairing_secret: invitation.secret,
                    peer_proof: &client_proof,
                },
            ),
            Ok(PairingHandshakeOutcome::Ready { .. })
        ));
        assert_eq!(invitation.session.phase(), SessionPhase::Ready);
    }

    #[test]
    fn a_scanned_payload_for_another_invitation_is_rejected_before_derivation() {
        let mut invitation = begin_invitation(100);
        let exporter = exporter(0xA0);
        let other_secret = [0x77_u8; 32];
        let other_id = id(0x99);
        let mut other_payload = vec![0_u8; MAX_QR_PAYLOAD_LEN];
        let mut other_session = SessionState::IDLE;
        let written = begin_from_rendered_payload(
            &mut PairingInvitationController::<MAX_HANDSHAKE_SOURCES>::new(),
            &mut other_session,
            100,
            10_000,
            3,
            invitation_inputs(other_id, other_secret),
            &mut other_payload,
        )
        .expect("the other invitation encodes");

        let error = verify_scanned_proof(
            &mut invitation.pairing,
            &mut invitation.session,
            150,
            source(0x44),
            &exporter,
            ScannedProofAttempt {
                scanned_payload: &other_payload[..written],
                expected_pairing_id: invitation.id,
                pairing_secret: invitation.secret,
                peer_proof: &[0; 32],
            },
        )
        .expect_err("a foreign invitation must not be admitted");

        assert_eq!(error, PairingHandshakeError::InvitationMismatch);
        assert_eq!(invitation.pairing.state(), Some(PairingRecordState::Active));
        assert_eq!(invitation.session.phase(), SessionPhase::Pairing);
    }

    #[test]
    fn a_noncanonical_exporter_fails_closed_before_charging_an_attempt() {
        let mut invitation = begin_invitation(100);
        let short_exporter = exporter(0xA0);
        let short_exporter = &short_exporter[..31];

        let error = verify_scanned_proof(
            &mut invitation.pairing,
            &mut invitation.session,
            150,
            source(0x44),
            short_exporter,
            ScannedProofAttempt {
                scanned_payload: &invitation.payload,
                expected_pairing_id: invitation.id,
                pairing_secret: invitation.secret,
                peer_proof: &[0; 32],
            },
        )
        .expect_err("a short exporter must fail closed");

        assert_eq!(error, PairingHandshakeError::InvalidExporterLength);
        assert_eq!(invitation.pairing.state(), Some(PairingRecordState::Active));
        assert_eq!(invitation.session.phase(), SessionPhase::Pairing);
    }

    #[test]
    fn a_corrupted_scan_is_rejected_without_charging_an_attempt() {
        let mut invitation = begin_invitation(100);
        // Flip a bit in the declared host length so the lengths no longer
        // match the payload size; this must fail before any derivation.
        let mut corrupted = invitation.payload.clone();
        corrupted[6] = corrupted[6].wrapping_add(1);

        let error = verify_scanned_proof(
            &mut invitation.pairing,
            &mut invitation.session,
            150,
            source(0x44),
            &exporter(0xA0),
            ScannedProofAttempt {
                scanned_payload: &corrupted,
                expected_pairing_id: invitation.id,
                pairing_secret: invitation.secret,
                peer_proof: &[0; 32],
            },
        )
        .expect_err("a corrupted scan must not be admitted");

        assert!(matches!(error, PairingHandshakeError::Qr(_)));
        assert_eq!(invitation.pairing.state(), Some(PairingRecordState::Active));
        assert_eq!(invitation.session.phase(), SessionPhase::Pairing);
    }

    #[test]
    fn an_expired_invitation_rejects_before_deriving_anything() {
        let mut invitation = begin_invitation(100);

        let error = verify_scanned_proof(
            &mut invitation.pairing,
            &mut invitation.session,
            10_100,
            source(0x44),
            &exporter(0xA0),
            ScannedProofAttempt {
                scanned_payload: &invitation.payload,
                expected_pairing_id: invitation.id,
                pairing_secret: invitation.secret,
                peer_proof: &[0; 32],
            },
        )
        .expect_err("an expired invitation must reject the handshake");

        assert!(matches!(error, PairingHandshakeError::Invitation(_)));
        assert_eq!(
            invitation.pairing.state(),
            Some(PairingRecordState::Expired)
        );
        assert_eq!(invitation.session.phase(), SessionPhase::Idle);
    }
}
