use core::fmt;

use crate::{
    PairingAttemptOutcome, PairingId, PairingRecord, PairingRecordError, PairingRecordState,
    PairingSource, SessionEvent, SessionState, SessionTransitionError,
};

/// Control-domain owner for one short-lived pairing invitation.
///
/// It coordinates an already-verified proof result with the desktop lifecycle,
/// but deliberately owns no QR payload, TLS connection, pairing secret, or
/// persistent identity material.
pub struct PairingInvitationController<const MAX_SOURCES: usize> {
    record: Option<PairingRecord<MAX_SOURCES>>,
}

/// Failure while creating, polling, expiring, or consuming an invitation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingInvitationError {
    /// A pairing-record bound or clock rule rejected the operation.
    Record(PairingRecordError),
    /// The shared lifecycle rejected the requested pairing transition.
    Transition(SessionTransitionError),
    /// An active invitation must expire before another can replace it.
    ActiveInvitation,
    /// There is no retained invitation to poll, expire, or consume.
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

impl std::error::Error for PairingInvitationError {}

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

    /// Records the outcome of one transcript-bound proof verified by the TLS owner.
    ///
    /// The desktop enters `Ready` only after [`PairingAttemptOutcome::Paired`].
    /// Rejected proofs keep the invitation and session in the pairing state.
    ///
    /// # Errors
    ///
    /// Returns a record error after synchronizing an expired invitation with
    /// the session lifecycle where possible.
    pub fn record_proof(
        &mut self,
        session: &mut SessionState,
        now_ms: u64,
        source: PairingSource,
        proof_verified: bool,
    ) -> Result<PairingAttemptOutcome, PairingInvitationError> {
        if *session != SessionState::PAIRING {
            return Err(PairingInvitationError::InvalidSessionState(*session));
        }
        let record = self
            .record
            .as_mut()
            .ok_or(PairingInvitationError::MissingInvitation)?;
        let outcome = record.record_proof(now_ms, source, proof_verified);

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

    fn synchronize_state(&self, session: &mut SessionState) -> Result<(), PairingInvitationError> {
        match self.state() {
            Some(PairingRecordState::Active) if *session != SessionState::PAIRING => {
                Err(PairingInvitationError::InvalidSessionState(*session))
            }
            Some(PairingRecordState::Consumed) if *session != SessionState::READY => {
                Err(PairingInvitationError::InvalidSessionState(*session))
            }
            Some(PairingRecordState::Expired) => session
                .apply(SessionEvent::SessionExpired)
                .map_err(PairingInvitationError::Transition),
            Some(PairingRecordState::Active | PairingRecordState::Consumed) | None => Ok(()),
        }
    }
}

impl<const MAX_SOURCES: usize> Default for PairingInvitationController<MAX_SOURCES> {
    fn default() -> Self {
        Self::new()
    }
}
