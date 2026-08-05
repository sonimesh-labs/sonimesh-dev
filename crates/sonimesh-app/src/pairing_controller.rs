use core::fmt;

use crate::{
    DesktopControlSnapshot, OutputController, PairingAttemptOutcome, PairingId,
    PairingProofVerification, PairingRecord, PairingRecordError, PairingRecordState, PairingSource,
    SessionEvent, SessionState, SessionTransitionError,
};
use sonimesh_platform::{AudioOutput, RenderSourceFactory};

/// Control-domain owner for one short-lived pairing invitation.
///
/// It coordinates an already-verified proof result with the desktop lifecycle,
/// but deliberately owns no QR payload, TLS connection, pairing secret, or
/// persistent identity material.
pub struct PairingInvitationController<const MAX_SOURCES: usize> {
    record: Option<PairingRecord<MAX_SOURCES>>,
}

/// Failure while creating, polling, expiring, revoking, or consuming an invitation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingInvitationError {
    /// A pairing-record bound or clock rule rejected the operation.
    Record(PairingRecordError),
    /// The shared lifecycle rejected the requested pairing transition.
    Transition(SessionTransitionError),
    /// An active invitation must expire before another can replace it.
    ActiveInvitation,
    /// There is no retained invitation to poll, expire, revoke, or consume.
    MissingInvitation,
    /// An invitation must have a non-zero lifetime.
    ZeroLifetime,
    /// Adding the requested lifetime would overflow the monotonic clock.
    ExpiryOverflow,
    /// The caller's lifecycle state cannot own this invitation operation.
    InvalidSessionState(SessionState),
}

impl fmt::Display for PairingInvitationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Record(error) => {
                write!(formatter, "pairing record rejected the operation: {error}")
            }
            Self::Transition(error) => {
                write!(formatter, "pairing lifecycle transition failed: {error}")
            }
            Self::ActiveInvitation => formatter.write_str("a pairing invitation is already active"),
            Self::MissingInvitation => formatter.write_str("there is no pairing invitation"),
            Self::ZeroLifetime => {
                formatter.write_str("pairing invitation lifetime must be non-zero")
            }
            Self::ExpiryOverflow => {
                formatter.write_str("pairing invitation expiry overflows the clock")
            }
            Self::InvalidSessionState(state) => {
                write!(
                    formatter,
                    "pairing operation requires a different session state than {state:?}"
                )
            }
        }
    }
}

impl std::error::Error for PairingInvitationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Record(error) => Some(error),
            Self::Transition(error) => Some(error),
            _ => None,
        }
    }
}

impl<const MAX_SOURCES: usize> PairingInvitationController<MAX_SOURCES> {
    /// Creates an owner with no retained pairing invitation.
    #[must_use]
    pub const fn new() -> Self {
        Self { record: None }
    }

    /// Returns the retained invitation state, including consumed or expired records.
    #[must_use]
    pub fn state(&self) -> Option<PairingRecordState> {
        self.record.as_ref().map(PairingRecord::state)
    }

    /// Observes pairing expiry and then projects receiver state for a low-rate
    /// control-surface adapter with the actual output lifecycle owner.
    ///
    /// This is the public snapshot entry point so a bridge cannot present an
    /// active invitation without first applying the pairing owner's monotonic
    /// clock. No invitation ID, proof, address, credential, or media material is
    /// included in the returned value. The output snapshot is read directly
    /// from `output`, so a bridge cannot fabricate output lifecycle state.
    ///
    /// # Errors
    ///
    /// Returns a pairing observation error after synchronizing the session where
    /// possible. An owner with no retained invitation still returns a snapshot.
    pub fn snapshot_at<O, F>(
        &mut self,
        session: &mut SessionState,
        now_ms: u64,
        output: &OutputController<O, F>,
    ) -> Result<DesktopControlSnapshot, PairingInvitationError>
    where
        O: AudioOutput,
        F: RenderSourceFactory,
    {
        if self.record.is_some() {
            self.observe(session, now_ms)?;
        }
        Ok(self.snapshot_after_observation(*session, output.snapshot()))
    }

    pub(crate) fn snapshot_after_observation(
        &self,
        session: SessionState,
        output: crate::OutputSnapshot,
    ) -> DesktopControlSnapshot {
        DesktopControlSnapshot::from_pairing_state(session, self.state(), output)
    }

    /// Starts a new active invitation from an idle desktop session.
    ///
    /// # Errors
    ///
    /// Returns without replacing any active invitation if its lifetime, fixed
    /// capacity, or lifecycle preconditions are invalid.
    pub fn begin(
        &mut self,
        session: &mut SessionState,
        id: PairingId,
        now_ms: u64,
        lifetime_ms: u64,
        max_attempts_per_source: u8,
    ) -> Result<(), PairingInvitationError> {
        if self.state() == Some(PairingRecordState::Active) {
            return Err(PairingInvitationError::ActiveInvitation);
        }
        if *session != SessionState::IDLE {
            return Err(PairingInvitationError::InvalidSessionState(*session));
        }
        if lifetime_ms == 0 {
            return Err(PairingInvitationError::ZeroLifetime);
        }
        let expires_at_ms = now_ms
            .checked_add(lifetime_ms)
            .ok_or(PairingInvitationError::ExpiryOverflow)?;
        let mut record = PairingRecord::new(id, expires_at_ms, max_attempts_per_source)
            .map_err(PairingInvitationError::Record)?;
        record
            .observe(now_ms)
            .map_err(PairingInvitationError::Record)?;

        session
            .apply(SessionEvent::StartPairing)
            .map_err(PairingInvitationError::Transition)?;
        self.record = Some(record);
        Ok(())
    }

    /// Polls expiration with the pairing owner's monotonic clock.
    ///
    /// A successful expiry, or a clock regression, returns the desktop to
    /// `Idle`; no future proof can consume that invitation.
    ///
    /// # Errors
    ///
    /// Returns a record error after synchronizing an expired record with the
    /// session lifecycle where possible.
    pub fn observe(
        &mut self,
        session: &mut SessionState,
        now_ms: u64,
    ) -> Result<PairingRecordState, PairingInvitationError> {
        let record = self
            .record
            .as_mut()
            .ok_or(PairingInvitationError::MissingInvitation)?;
        let observed = record.observe(now_ms);
        self.synchronize_state(session)?;
        observed.map_err(PairingInvitationError::Record)
    }

    /// Admits one source before invoking its transcript-bound proof verifier.
    ///
    /// The desktop enters `Ready` only after [`PairingAttemptOutcome::Paired`].
    /// Rejected proofs keep the invitation and session in the pairing state. The
    /// verifier is not invoked when the session or pairing record rejects the
    /// attempt.
    ///
    /// # Errors
    ///
    /// Returns a record error after synchronizing an expired invitation with
    /// the session lifecycle where possible.
    pub fn verify_proof(
        &mut self,
        session: &mut SessionState,
        now_ms: u64,
        source: PairingSource,
        verify: impl FnOnce() -> PairingProofVerification,
    ) -> Result<PairingAttemptOutcome, PairingInvitationError> {
        if *session != SessionState::PAIRING {
            return Err(PairingInvitationError::InvalidSessionState(*session));
        }
        let record = self
            .record
            .as_mut()
            .ok_or(PairingInvitationError::MissingInvitation)?;
        let outcome = record.verify_proof(now_ms, source, verify);

        match outcome {
            Ok(PairingAttemptOutcome::Paired) => {
                if !session.establish_trust() {
                    return Err(PairingInvitationError::InvalidSessionState(*session));
                }
                Ok(PairingAttemptOutcome::Paired)
            }
            Ok(rejected @ PairingAttemptOutcome::Rejected { .. }) => Ok(rejected),
            Err(error) => {
                self.synchronize_state(session)?;
                Err(PairingInvitationError::Record(error))
            }
        }
    }

    /// Expires the active invitation at the user's request.
    ///
    /// # Errors
    ///
    /// Returns an error when there is no invitation or its session state cannot
    /// be synchronized with the retained record.
    pub fn expire(
        &mut self,
        session: &mut SessionState,
    ) -> Result<PairingRecordState, PairingInvitationError> {
        let state = {
            let record = self
                .record
                .as_mut()
                .ok_or(PairingInvitationError::MissingInvitation)?;
            record.expire();
            record.state()
        };
        self.synchronize_state(session)?;
        Ok(state)
    }

    /// Revokes a retained invitation or its trusted in-memory session.
    ///
    /// The record becomes terminal and the shared lifecycle returns to `Idle`.
    /// This does not persist or delete production trust material.
    ///
    /// # Errors
    ///
    /// Returns an error when no invitation is retained or the shared lifecycle
    /// cannot be synchronized with the revoked record.
    pub fn revoke(
        &mut self,
        session: &mut SessionState,
    ) -> Result<PairingRecordState, PairingInvitationError> {
        let state = {
            let record = self
                .record
                .as_mut()
                .ok_or(PairingInvitationError::MissingInvitation)?;
            record.revoke();
            record.state()
        };
        self.synchronize_state(session)?;
        Ok(state)
    }

    fn synchronize_state(&self, session: &mut SessionState) -> Result<(), PairingInvitationError> {
        match self.state() {
            Some(PairingRecordState::Active) if *session != SessionState::PAIRING => {
                Err(PairingInvitationError::InvalidSessionState(*session))
            }
            Some(PairingRecordState::Expired | PairingRecordState::Revoked) => session
                .apply(SessionEvent::SessionExpired)
                .map_err(PairingInvitationError::Transition),
            // A consumed record is retained audit state. The verified session may
            // subsequently stream, reconnect, lose output, or later expire.
            Some(PairingRecordState::Active | PairingRecordState::Consumed) | None => Ok(()),
        }
    }
}

impl<const MAX_SOURCES: usize> Default for PairingInvitationController<MAX_SOURCES> {
    fn default() -> Self {
        Self::new()
    }
}
