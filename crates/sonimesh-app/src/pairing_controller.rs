use core::fmt;

use crate::{
    DesktopControlSnapshot, OutputSnapshotSource, PairingAttemptOutcome, PairingId,
    PairingProofVerification, PairingRecord, PairingRecordError, PairingRecordState, PairingSource,
    SessionEvent, SessionState, SessionTransitionError,
};

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
    /// from the output owner through [`OutputSnapshotSource`], so a bridge
    /// cannot fabricate output lifecycle state and either the platform output
    /// controller or the receiver output lifecycle controller can feed it.
    ///
    /// # Errors
    ///
    /// Returns a pairing observation error after synchronizing the session where
    /// possible. An owner with no retained invitation still returns a snapshot.
    pub fn snapshot_at(
        &mut self,
        session: &mut SessionState,
        now_ms: u64,
        output: &impl OutputSnapshotSource,
    ) -> Result<DesktopControlSnapshot, PairingInvitationError> {
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
        // The pairing owner has no reconnect window; the control-session
        // coordinator observes the reconnect clock and injects the bounded
        // remaining time after this pairing projection.
        DesktopControlSnapshot::from_pairing_state(session, self.state(), output, None)
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
        self.ensure_state_can_synchronize(*session)?;
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
        self.ensure_state_can_synchronize(*session)?;
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
        self.ensure_state_can_synchronize(*session)?;
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

    fn ensure_state_can_synchronize(
        &self,
        session: SessionState,
    ) -> Result<(), PairingInvitationError> {
        if self.state() == Some(PairingRecordState::Active) && session != SessionState::PAIRING {
            return Err(PairingInvitationError::InvalidSessionState(session));
        }
        Ok(())
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

#[cfg(test)]
mod tests {
    use sonimesh_platform::OutputHealth;

    use super::*;
    use crate::control_snapshot::PairingSnapshotState;
    use crate::{OutputSnapshot, SessionPhase};

    struct TestOutput;

    impl OutputSnapshotSource for TestOutput {
        fn snapshot(&self) -> OutputSnapshot {
            OutputSnapshot::from_health(true, false, false, OutputHealth::Healthy)
        }
    }

    fn id(byte: u8) -> PairingId {
        PairingId::new([byte; 16])
    }

    fn source(byte: u8) -> PairingSource {
        PairingSource::new([byte; 16])
    }

    fn controller() -> PairingInvitationController<2> {
        PairingInvitationController::new()
    }

    #[test]
    fn fresh_controller_has_no_invitation_and_projects_an_idle_snapshot() {
        let mut controller = controller();
        let mut session = SessionState::IDLE;

        assert_eq!(controller.state(), None);
        let snapshot = controller
            .snapshot_at(&mut session, 100, &TestOutput)
            .unwrap();
        assert_eq!(snapshot.session(), SessionPhase::Idle);
        assert_eq!(snapshot.pairing(), PairingSnapshotState::Inactive);
        assert!(snapshot.output().selection_retained());
        assert_eq!(controller.state(), None);
    }

    #[test]
    fn begin_requires_an_idle_session_and_non_zero_lifetime() {
        let mut controller = controller();
        let mut session = SessionState::IDLE;

        assert_eq!(
            controller.begin(&mut session, id(1), 100, 0, 3),
            Err(PairingInvitationError::ZeroLifetime)
        );
        assert_eq!(
            controller.begin(&mut session, id(1), u64::MAX, 1, 3),
            Err(PairingInvitationError::ExpiryOverflow)
        );
        assert_eq!(
            controller.begin(&mut session, id(1), 100, 1, 0),
            Err(PairingInvitationError::Record(
                PairingRecordError::InvalidAttemptLimit
            ))
        );
        assert_eq!(controller.state(), None);
        assert_eq!(session, SessionState::IDLE);

        // A non-idle session cannot start pairing.
        let mut streaming = SessionState::IDLE;
        streaming.apply(SessionEvent::StartPairing).unwrap();
        streaming.establish_trust();
        streaming.apply(SessionEvent::StartStream).unwrap();
        assert_eq!(
            controller.begin(&mut streaming, id(1), 100, 1_000, 3),
            Err(PairingInvitationError::InvalidSessionState(
                SessionState::STREAMING
            ))
        );
    }

    #[test]
    fn begin_admits_one_active_invitation_and_advances_to_pairing() {
        let mut controller = controller();
        let mut session = SessionState::IDLE;

        controller
            .begin(&mut session, id(1), 100, 1_000, 3)
            .unwrap();
        assert_eq!(controller.state(), Some(PairingRecordState::Active));
        assert_eq!(session, SessionState::PAIRING);

        // A second active invitation cannot replace the first.
        assert_eq!(
            controller.begin(&mut session, id(2), 200, 1_000, 3),
            Err(PairingInvitationError::ActiveInvitation)
        );
        assert_eq!(controller.state(), Some(PairingRecordState::Active));
    }

    #[test]
    fn observe_expiry_returns_the_desktop_to_idle() {
        let mut controller = controller();
        let mut session = SessionState::IDLE;
        controller
            .begin(&mut session, id(1), 100, 1_000, 3)
            .unwrap();

        assert_eq!(
            controller.observe(&mut session, 1_099).unwrap(),
            PairingRecordState::Active
        );
        assert_eq!(
            controller.observe(&mut session, 1_100).unwrap(),
            PairingRecordState::Expired
        );
        assert_eq!(session, SessionState::IDLE);
        // The expired record is retained for the UI snapshot.
        assert_eq!(controller.state(), Some(PairingRecordState::Expired));
    }

    #[test]
    fn observe_clock_regression_force_expires_and_returns_to_idle() {
        let mut controller = controller();
        let mut session = SessionState::IDLE;
        controller
            .begin(&mut session, id(1), 100, 1_000, 3)
            .unwrap();
        controller.observe(&mut session, 500).unwrap();

        assert_eq!(
            controller.observe(&mut session, 400),
            Err(PairingInvitationError::Record(
                PairingRecordError::TimeRegressed
            ))
        );
        assert_eq!(session, SessionState::IDLE);
        assert_eq!(controller.state(), Some(PairingRecordState::Expired));
    }

    #[test]
    fn verified_proof_moves_the_session_to_ready() {
        let mut controller = controller();
        let mut session = SessionState::IDLE;
        controller
            .begin(&mut session, id(1), 100, 1_000, 3)
            .unwrap();

        let outcome = controller
            .verify_proof(&mut session, 200, source(1), || {
                PairingProofVerification::Verified
            })
            .unwrap();
        assert_eq!(outcome, PairingAttemptOutcome::Paired);
        assert_eq!(session, SessionState::READY);
        assert_eq!(controller.state(), Some(PairingRecordState::Consumed));
    }

    #[test]
    fn rejected_proof_keeps_the_invitation_and_session_in_pairing() {
        let mut controller = controller();
        let mut session = SessionState::IDLE;
        controller
            .begin(&mut session, id(1), 100, 1_000, 3)
            .unwrap();

        let outcome = controller
            .verify_proof(&mut session, 200, source(1), || {
                PairingProofVerification::Rejected
            })
            .unwrap();
        assert_eq!(
            outcome,
            PairingAttemptOutcome::Rejected {
                attempts_remaining: 2
            }
        );
        assert_eq!(session, SessionState::PAIRING);
        assert_eq!(controller.state(), Some(PairingRecordState::Active));
    }

    #[test]
    fn rate_limited_source_keeps_the_session_in_pairing() {
        let mut controller = controller();
        let mut session = SessionState::IDLE;
        controller
            .begin(&mut session, id(1), 100, 1_000, 2)
            .unwrap();
        for _ in 0..2 {
            controller
                .verify_proof(&mut session, 200, source(1), || {
                    PairingProofVerification::Rejected
                })
                .unwrap();
        }

        assert_eq!(
            controller.verify_proof(&mut session, 300, source(1), || {
                PairingProofVerification::Verified
            }),
            Err(PairingInvitationError::Record(
                PairingRecordError::SourceRateLimited
            ))
        );
        assert_eq!(session, SessionState::PAIRING);
    }

    #[test]
    fn expired_proof_returns_the_session_to_idle_and_never_invokes_verifier() {
        let mut controller = controller();
        let mut session = SessionState::IDLE;
        controller
            .begin(&mut session, id(1), 100, 1_000, 3)
            .unwrap();
        let mut verify_calls = 0;

        assert_eq!(
            controller.verify_proof(&mut session, 1_100, source(1), || {
                verify_calls += 1;
                PairingProofVerification::Verified
            }),
            Err(PairingInvitationError::Record(PairingRecordError::Expired))
        );
        assert_eq!(verify_calls, 0);
        assert_eq!(session, SessionState::IDLE);
        assert_eq!(controller.state(), Some(PairingRecordState::Expired));
    }

    #[test]
    fn verify_proof_requires_the_pairing_session_state() {
        let mut controller = controller();
        let mut session = SessionState::IDLE;
        controller
            .begin(&mut session, id(1), 100, 1_000, 3)
            .unwrap();

        // A ready session cannot admit proofs: only the pairing phase may.
        let mut ready = SessionState::IDLE;
        ready.apply(SessionEvent::StartPairing).unwrap();
        ready.establish_trust();
        assert_eq!(
            controller.verify_proof(&mut ready, 200, source(1), || {
                PairingProofVerification::Verified
            }),
            Err(PairingInvitationError::InvalidSessionState(
                SessionState::READY
            ))
        );
    }

    #[test]
    fn expire_and_revoke_return_the_desktop_to_idle() {
        let mut controller = controller();
        let mut session = SessionState::IDLE;
        controller
            .begin(&mut session, id(1), 100, 1_000, 3)
            .unwrap();

        assert_eq!(
            controller.expire(&mut session).unwrap(),
            PairingRecordState::Expired
        );
        assert_eq!(session, SessionState::IDLE);

        controller
            .begin(&mut session, id(2), 200, 1_000, 3)
            .unwrap();
        assert_eq!(
            controller.revoke(&mut session).unwrap(),
            PairingRecordState::Revoked
        );
        assert_eq!(session, SessionState::IDLE);
    }

    #[test]
    fn operations_without_an_invitation_report_missing() {
        let mut controller = controller();
        let mut session = SessionState::IDLE;

        assert_eq!(
            controller.observe(&mut session, 100),
            Err(PairingInvitationError::MissingInvitation)
        );
        assert_eq!(
            controller.expire(&mut session),
            Err(PairingInvitationError::MissingInvitation)
        );
        assert_eq!(
            controller.revoke(&mut session),
            Err(PairingInvitationError::MissingInvitation)
        );
        assert_eq!(controller.state(), None);
    }

    #[test]
    fn invalid_session_state_does_not_mutate_an_active_invitation() {
        let mut controller = controller();
        let mut pairing_session = SessionState::IDLE;
        controller
            .begin(&mut pairing_session, id(1), 100, 1_000, 3)
            .unwrap();
        let mut wrong_session = SessionState::READY;

        assert_eq!(
            controller.observe(&mut wrong_session, 200),
            Err(PairingInvitationError::InvalidSessionState(
                SessionState::READY
            ))
        );
        assert_eq!(controller.state(), Some(PairingRecordState::Active));

        assert_eq!(
            controller.expire(&mut wrong_session),
            Err(PairingInvitationError::InvalidSessionState(
                SessionState::READY
            ))
        );
        assert_eq!(controller.state(), Some(PairingRecordState::Active));

        assert_eq!(
            controller.revoke(&mut wrong_session),
            Err(PairingInvitationError::InvalidSessionState(
                SessionState::READY
            ))
        );
        assert_eq!(controller.state(), Some(PairingRecordState::Active));
        assert_eq!(wrong_session, SessionState::READY);
    }

    #[test]
    fn snapshot_projects_active_pairing_state() {
        let mut controller = controller();
        let mut session = SessionState::IDLE;
        controller
            .begin(&mut session, id(1), 100, 1_000, 3)
            .unwrap();

        let snapshot = controller
            .snapshot_at(&mut session, 500, &TestOutput)
            .unwrap();
        assert_eq!(snapshot.session(), SessionPhase::Pairing);
        assert_eq!(snapshot.pairing(), PairingSnapshotState::Active);
        assert!(snapshot.output().selection_retained());
    }

    #[test]
    fn snapshot_after_expiry_projects_expired_pairing_state() {
        let mut controller = controller();
        let mut session = SessionState::IDLE;
        controller
            .begin(&mut session, id(1), 100, 1_000, 3)
            .unwrap();
        controller.observe(&mut session, 1_100).unwrap();

        let snapshot = controller
            .snapshot_at(&mut session, 1_200, &TestOutput)
            .unwrap();
        assert_eq!(snapshot.session(), SessionPhase::Idle);
        assert_eq!(snapshot.pairing(), PairingSnapshotState::Expired);
    }

    #[test]
    fn error_display_is_actionable_and_preserves_sources() {
        assert_eq!(
            PairingInvitationError::ActiveInvitation.to_string(),
            "a pairing invitation is already active"
        );
        assert_eq!(
            PairingInvitationError::MissingInvitation.to_string(),
            "there is no pairing invitation"
        );
        assert_eq!(
            PairingInvitationError::Record(PairingRecordError::SourceRateLimited).to_string(),
            "pairing record rejected the operation: pairing source exceeded its attempt limit"
        );
        assert_eq!(
            PairingInvitationError::InvalidSessionState(SessionState::STREAMING).to_string(),
            "pairing operation requires a different session state than SessionState(Streaming)"
        );
    }
}
