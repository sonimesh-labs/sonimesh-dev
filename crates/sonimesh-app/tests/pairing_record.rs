//! Bounded pairing invitation lifecycle tests without QR, TLS, or secrets.

use core::cell::Cell;

use sonimesh_app::{
    PairingAttemptOutcome, PairingId, PairingProofVerification, PairingRecord, PairingRecordError,
    PairingRecordState, PairingSource,
};

fn id() -> PairingId {
    PairingId::new([1; 16])
}

fn source(value: u8) -> PairingSource {
    PairingSource::new([value; 16])
}

#[test]
fn valid_proof_consumes_the_record_exactly_once() {
    let mut record = PairingRecord::<2>::new(id(), 100, 3).unwrap();
    let verifier_calls = Cell::new(0);

    assert_eq!(
        record.verify_proof(10, source(1), || {
            verifier_calls.set(verifier_calls.get() + 1);
            PairingProofVerification::Verified
        }),
        Ok(PairingAttemptOutcome::Paired)
    );
    assert_eq!(record.state(), PairingRecordState::Consumed);
    assert_eq!(
        record.verify_proof(11, source(1), || {
            verifier_calls.set(verifier_calls.get() + 1);
            PairingProofVerification::Verified
        }),
        Err(PairingRecordError::Consumed)
    );
    assert_eq!(verifier_calls.get(), 1);
    record.expire();
    assert_eq!(record.state(), PairingRecordState::Consumed);

    record.revoke();
    assert_eq!(record.state(), PairingRecordState::Revoked);
    assert_eq!(
        record.verify_proof(12, source(1), || {
            verifier_calls.set(verifier_calls.get() + 1);
            PairingProofVerification::Verified
        }),
        Err(PairingRecordError::Revoked)
    );
    assert_eq!(verifier_calls.get(), 1);
}

#[test]
fn revocation_is_terminal_and_rejects_replayed_proofs() {
    let mut record = PairingRecord::<1>::new(id(), 100, 1).unwrap();
    let verifier_calls = Cell::new(0);

    record.revoke();
    assert_eq!(record.state(), PairingRecordState::Revoked);
    assert_eq!(record.observe(10), Ok(PairingRecordState::Revoked));
    assert_eq!(
        record.verify_proof(10, source(1), || {
            verifier_calls.set(verifier_calls.get() + 1);
            PairingProofVerification::Verified
        }),
        Err(PairingRecordError::Revoked)
    );
    assert_eq!(verifier_calls.get(), 0);
    record.expire();
    assert_eq!(record.state(), PairingRecordState::Revoked);
}

#[test]
fn failures_are_rate_limited_per_opaque_source() {
    let mut record = PairingRecord::<2>::new(id(), 100, 2).unwrap();

    assert_eq!(
        record.verify_proof(10, source(1), || PairingProofVerification::Rejected),
        Ok(PairingAttemptOutcome::Rejected {
            attempts_remaining: 1,
        })
    );
    assert_eq!(
        record.verify_proof(11, source(1), || PairingProofVerification::Rejected),
        Ok(PairingAttemptOutcome::Rejected {
            attempts_remaining: 0,
        })
    );
    assert_eq!(
        record.verify_proof(12, source(1), || PairingProofVerification::Verified),
        Err(PairingRecordError::SourceRateLimited)
    );
    assert_eq!(
        record.verify_proof(13, source(2), || PairingProofVerification::Verified),
        Ok(PairingAttemptOutcome::Paired)
    );
}

#[test]
fn verifier_failure_consumes_an_attempt_without_consuming_the_record() {
    let mut record = PairingRecord::<1>::new(id(), 100, 1).unwrap();
    let verifier_calls = Cell::new(0);

    assert_eq!(
        record.verify_proof(10, source(1), || {
            verifier_calls.set(verifier_calls.get() + 1);
            PairingProofVerification::Failed
        }),
        Err(PairingRecordError::VerificationFailed)
    );
    assert_eq!(verifier_calls.get(), 1);
    assert_eq!(record.state(), PairingRecordState::Active);
    assert_eq!(
        record.verify_proof(11, source(1), || PairingProofVerification::Verified),
        Err(PairingRecordError::SourceRateLimited)
    );
}

#[test]
fn rejected_admission_never_invokes_the_proof_verifier() {
    let mut record = PairingRecord::<1>::new(id(), 20, 1).unwrap();
    let verifier_calls = Cell::new(0);
    let rejected = || {
        verifier_calls.set(verifier_calls.get() + 1);
        PairingProofVerification::Rejected
    };

    assert_eq!(
        record.verify_proof(10, source(1), rejected),
        Ok(PairingAttemptOutcome::Rejected {
            attempts_remaining: 0,
        })
    );
    assert_eq!(verifier_calls.get(), 1);

    assert_eq!(
        record.verify_proof(11, source(1), || {
            verifier_calls.set(verifier_calls.get() + 1);
            PairingProofVerification::Verified
        }),
        Err(PairingRecordError::SourceRateLimited)
    );
    assert_eq!(
        record.verify_proof(12, source(2), || {
            verifier_calls.set(verifier_calls.get() + 1);
            PairingProofVerification::Verified
        }),
        Err(PairingRecordError::SourceTableFull)
    );
    assert_eq!(verifier_calls.get(), 1);

    assert_eq!(
        record.verify_proof(20, source(1), || {
            verifier_calls.set(verifier_calls.get() + 1);
            PairingProofVerification::Verified
        }),
        Err(PairingRecordError::Expired)
    );
    assert_eq!(verifier_calls.get(), 1);
}

#[test]
fn expiration_and_clock_regression_fail_closed() {
    let mut record = PairingRecord::<1>::new(id(), 20, 1).unwrap();

    assert_eq!(
        record.verify_proof(20, source(1), || PairingProofVerification::Verified),
        Err(PairingRecordError::Expired)
    );
    assert_eq!(record.state(), PairingRecordState::Expired);

    let mut active = PairingRecord::<1>::new(id(), 20, 1).unwrap();
    let verifier_calls = Cell::new(0);
    assert!(
        active
            .verify_proof(10, source(1), || {
                verifier_calls.set(verifier_calls.get() + 1);
                PairingProofVerification::Rejected
            })
            .is_ok()
    );
    assert_eq!(
        active.verify_proof(9, source(1), || {
            verifier_calls.set(verifier_calls.get() + 1);
            PairingProofVerification::Verified
        }),
        Err(PairingRecordError::TimeRegressed)
    );
    assert_eq!(active.state(), PairingRecordState::Expired);
    assert_eq!(
        active.verify_proof(11, source(1), || {
            verifier_calls.set(verifier_calls.get() + 1);
            PairingProofVerification::Verified
        }),
        Err(PairingRecordError::Expired)
    );
    assert_eq!(verifier_calls.get(), 1);
}

#[test]
fn source_table_is_bounded_without_storing_raw_addresses() {
    let mut record = PairingRecord::<1>::new(id(), 100, 2).unwrap();

    assert!(
        record
            .verify_proof(1, source(1), || PairingProofVerification::Rejected)
            .is_ok()
    );
    assert_eq!(
        record.verify_proof(2, source(2), || PairingProofVerification::Rejected),
        Err(PairingRecordError::SourceTableFull)
    );
}
