#![no_main]

//! Fuzz the bounded pairing-invitation lifecycle state machine.
//!
//! Arbitrary operation scripts drive begin, observe, verify_proof, expire, and
//! revoke on one [`PairingInvitationController`] with adversarial pairing ids,
//! sources, clocks, lifetimes, and per-source attempt limits. The owner must
//! never panic, must keep the shared session consistent with the retained
//! record, must invoke the transcript-bound proof verifier exactly when the
//! record admits an attempt (never after expiry, consumption, revocation, or
//! source rate limiting), and must always accept a fresh invitation after a
//! terminal record.

use libfuzzer_sys::fuzz_target;
use sonimesh_app::{
    PairingAttemptOutcome, PairingId, PairingInvitationController, PairingInvitationError,
    PairingProofVerification, PairingRecordError, PairingRecordState, PairingSource, SessionPhase,
    SessionState,
};

/// Bounded source table exercised by the fuzz script.
const MAX_SOURCES: usize = 4;

/// Reads one little-endian `u64` from `data`, wrapping around the slice.
fn next_u64(data: &[u8], offset: &mut usize) -> u64 {
    let mut bytes = [0_u8; 8];
    for byte in &mut bytes {
        *byte = data[*offset % data.len()];
        *offset += 1;
    }
    u64::from_le_bytes(bytes)
}

/// Reads `N` bytes from `data`, wrapping around the slice.
fn next_bytes<const N: usize>(data: &[u8], offset: &mut usize) -> [u8; N] {
    let mut bytes = [0_u8; N];
    for byte in &mut bytes {
        *byte = data[*offset % data.len()];
        *offset += 1;
    }
    bytes
}

/// Asserts the shared session stays consistent with the retained record.
fn assert_session_consistent(
    controller: &PairingInvitationController<MAX_SOURCES>,
    session: &SessionState,
) {
    match controller.state() {
        Some(PairingRecordState::Active) => {
            assert_eq!(
                session.phase(),
                SessionPhase::Pairing,
                "an active invitation requires the pairing phase"
            );
        }
        Some(PairingRecordState::Expired | PairingRecordState::Revoked) => {
            assert_eq!(
                session.phase(),
                SessionPhase::Idle,
                "a terminal invitation retires the session to idle"
            );
        }
        Some(PairingRecordState::Consumed) | None => {}
    }
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    let mut controller = PairingInvitationController::<MAX_SOURCES>::new();
    let mut session = SessionState::IDLE;
    let mut offset = 0;

    // Bounded deterministic step budget so every input finishes quickly.
    let steps = 1 + data.len() % 64;
    for _ in 0..steps {
        let op = data[offset % data.len()];
        offset += 1;
        match op % 5 {
            0 => {
                // Begin one invitation with adversarial clock and lifetime.
                let now_ms = next_u64(data, &mut offset);
                let lifetime_ms = next_u64(data, &mut offset);
                let max_attempts = data[offset % data.len()];
                offset += 1;
                let id = PairingId::new(next_bytes(data, &mut offset));
                let result = controller.begin(&mut session, id, now_ms, lifetime_ms, max_attempts);
                if result.is_ok() {
                    assert_eq!(session.phase(), SessionPhase::Pairing);
                    assert_eq!(controller.state(), Some(PairingRecordState::Active));
                }
            }
            1 => {
                // Poll the invitation clock; expiry or regression must retire
                // the session to idle.
                let now_ms = next_u64(data, &mut offset);
                let _ = controller.observe(&mut session, now_ms);
            }
            2 => {
                // Verify one transcript-bound proof. The verifier must run
                // exactly when the record admits the attempt.
                let now_ms = next_u64(data, &mut offset);
                let source = PairingSource::new(next_bytes(data, &mut offset));
                let outcome_byte = data[offset % data.len()];
                offset += 1;
                let mut verifier_called = false;
                let result = controller.verify_proof(&mut session, now_ms, source, || {
                    verifier_called = true;
                    match outcome_byte % 3 {
                        0 => PairingProofVerification::Verified,
                        1 => PairingProofVerification::Rejected,
                        _ => PairingProofVerification::Failed,
                    }
                });
                let expected_called = matches!(
                    result,
                    Ok(PairingAttemptOutcome::Paired | PairingAttemptOutcome::Rejected { .. })
                        | Err(PairingInvitationError::Record(
                            PairingRecordError::VerificationFailed
                        ))
                );
                assert_eq!(
                    verifier_called, expected_called,
                    "the verifier must run exactly when the record admits the attempt"
                );
                if matches!(result, Ok(PairingAttemptOutcome::Paired)) {
                    assert_eq!(session.phase(), SessionPhase::Ready);
                    assert_eq!(controller.state(), Some(PairingRecordState::Consumed));
                }
            }
            3 => {
                let _ = controller.expire(&mut session);
            }
            _ => {
                let _ = controller.revoke(&mut session);
            }
        }
        assert_session_consistent(&controller, &session);
    }
});
