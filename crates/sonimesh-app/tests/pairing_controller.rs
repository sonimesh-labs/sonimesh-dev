//! Pairing invitation orchestration tests without a TLS or secret implementation.

use core::cell::Cell;

use sonimesh_app::{
    PairingAttemptOutcome, PairingId, PairingInvitationController, PairingInvitationError,
    PairingProofVerification, PairingRecordError, PairingRecordState, PairingSource, SessionPhase,
    SessionState,
};

fn id(value: u8) -> PairingId {
    PairingId::new([value; 16])
}

fn source(value: u8) -> PairingSource {
    PairingSource::new([value; 16])
}

#[test]
fn accepted_proof_is_the_only_path_from_pairing_to_ready() {
    let mut controller = PairingInvitationController::<2>::new();
    let mut session = SessionState::IDLE;
    controller.begin(&mut session, id(1), 100, 50, 2).unwrap();

    assert_eq!(session.phase(), SessionPhase::Pairing);
    assert_eq!(
        controller.verify_proof(&mut session, 110, source(1), || {
            PairingProofVerification::Rejected
        }),
        Ok(PairingAttemptOutcome::Rejected {
            attempts_remaining: 1
        })
    );
    assert_eq!(session.phase(), SessionPhase::Pairing);

    assert_eq!(
        controller.verify_proof(&mut session, 111, source(1), || {
            PairingProofVerification::Verified
        }),
        Ok(PairingAttemptOutcome::Paired)
    );
    assert_eq!(session.phase(), SessionPhase::Ready);
    assert_eq!(controller.state(), Some(PairingRecordState::Consumed));
}

#[test]
fn verifier_failure_does_not_establish_trust() {
    let mut controller = PairingInvitationController::<1>::new();
    let mut session = SessionState::IDLE;
    controller.begin(&mut session, id(1), 100, 50, 2).unwrap();

    assert_eq!(
        controller.verify_proof(&mut session, 110, source(1), || {
            PairingProofVerification::Failed
        }),
        Err(PairingInvitationError::Record(
            PairingRecordError::VerificationFailed
        ))
    );
    assert_eq!(session.phase(), SessionPhase::Pairing);
    assert_eq!(controller.state(), Some(PairingRecordState::Active));
}

#[test]
fn invalid_session_rejects_before_invoking_the_proof_verifier() {
    let mut controller = PairingInvitationController::<1>::new();
    let mut session = SessionState::IDLE;
    let verifier_calls = Cell::new(0);

    assert_eq!(
        controller.verify_proof(&mut session, 100, source(1), || {
            verifier_calls.set(verifier_calls.get() + 1);
            PairingProofVerification::Verified
        }),
        Err(PairingInvitationError::InvalidSessionState(
            SessionState::IDLE
        ))
    );
    assert_eq!(verifier_calls.get(), 0);
}

#[test]
fn expiration_and_clock_regression_return_session_to_idle() {
    let mut controller = PairingInvitationController::<1>::new();
    let mut session = SessionState::IDLE;
    controller.begin(&mut session, id(1), 100, 20, 1).unwrap();

    assert_eq!(
        controller.verify_proof(&mut session, 99, source(1), || {
            PairingProofVerification::Verified
        }),
        Err(PairingInvitationError::Record(
            PairingRecordError::TimeRegressed
        ))
    );
    assert_eq!(session.phase(), SessionPhase::Idle);
    assert_eq!(controller.state(), Some(PairingRecordState::Expired));

    controller.begin(&mut session, id(2), 100, 20, 1).unwrap();
    assert_eq!(
        controller.observe(&mut session, 120),
        Ok(PairingRecordState::Expired)
    );
    assert_eq!(session.phase(), SessionPhase::Idle);

    controller.begin(&mut session, id(3), 200, 20, 1).unwrap();
    assert_eq!(
        controller.observe(&mut session, 210),
        Ok(PairingRecordState::Active)
    );
    assert_eq!(
        controller.verify_proof(&mut session, 209, source(1), || {
            PairingProofVerification::Verified
        }),
        Err(PairingInvitationError::Record(
            PairingRecordError::TimeRegressed
        ))
    );
    assert_eq!(session.phase(), SessionPhase::Idle);
    assert_eq!(controller.state(), Some(PairingRecordState::Expired));
}

#[test]
fn active_invitation_cannot_be_replaced_or_expired_record_revived() {
    let mut controller = PairingInvitationController::<1>::new();
    let mut session = SessionState::IDLE;
    controller.begin(&mut session, id(1), 100, 50, 1).unwrap();

    assert_eq!(
        controller.begin(&mut session, id(2), 101, 50, 1),
        Err(PairingInvitationError::ActiveInvitation)
    );
    assert_eq!(
        controller.expire(&mut session),
        Ok(PairingRecordState::Expired)
    );
    assert_eq!(session.phase(), SessionPhase::Idle);
    assert_eq!(
        controller.verify_proof(&mut session, 102, source(1), || {
            PairingProofVerification::Verified
        }),
        Err(PairingInvitationError::InvalidSessionState(
            SessionState::IDLE
        ))
    );
}

#[test]
fn revocation_retires_a_trusted_session_without_recreating_trust() {
    let mut controller = PairingInvitationController::<1>::new();
    let mut session = SessionState::IDLE;
    controller.begin(&mut session, id(1), 100, 50, 1).unwrap();
    assert_eq!(
        controller.verify_proof(&mut session, 101, source(1), || {
            PairingProofVerification::Verified
        }),
        Ok(PairingAttemptOutcome::Paired)
    );
    assert_eq!(session.phase(), SessionPhase::Ready);

    assert_eq!(
        controller.revoke(&mut session),
        Ok(PairingRecordState::Revoked)
    );
    assert_eq!(session.phase(), SessionPhase::Idle);
    assert_eq!(controller.state(), Some(PairingRecordState::Revoked));
    assert_eq!(
        controller.verify_proof(&mut session, 102, source(1), || {
            PairingProofVerification::Verified
        }),
        Err(PairingInvitationError::InvalidSessionState(
            SessionState::IDLE
        ))
    );
}

#[test]
fn invalid_creation_leaves_session_and_controller_empty() {
    let mut controller = PairingInvitationController::<1>::new();
    let mut session = SessionState::IDLE;

    assert_eq!(
        controller.begin(&mut session, id(1), 100, 0, 1),
        Err(PairingInvitationError::ZeroLifetime)
    );
    assert_eq!(session.phase(), SessionPhase::Idle);
    assert_eq!(controller.state(), None);

    assert_eq!(
        controller.begin(&mut session, id(1), u64::MAX, 1, 1),
        Err(PairingInvitationError::ExpiryOverflow)
    );
    assert_eq!(session.phase(), SessionPhase::Idle);
    assert_eq!(controller.state(), None);
}
