//! Bounded pairing invitation lifecycle tests without QR, TLS, or secrets.

use sonimesh_app::{
    PairingAttemptOutcome, PairingId, PairingRecord, PairingRecordError, PairingRecordState,
    PairingSource,
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

    assert_eq!(
        record.record_proof(10, source(1), true),
        Ok(PairingAttemptOutcome::Paired)
    );
    assert_eq!(record.state(), PairingRecordState::Consumed);
    assert_eq!(
        record.record_proof(11, source(1), true),
        Err(PairingRecordError::Consumed)
    );
    record.expire();
    assert_eq!(record.state(), PairingRecordState::Consumed);
}

#[test]
fn failures_are_rate_limited_per_opaque_source() {
    let mut record = PairingRecord::<2>::new(id(), 100, 2).unwrap();

    assert_eq!(
        record.record_proof(10, source(1), false),
        Ok(PairingAttemptOutcome::Rejected {
            attempts_remaining: 1,
        })
    );
    assert_eq!(
        record.record_proof(11, source(1), false),
        Ok(PairingAttemptOutcome::Rejected {
            attempts_remaining: 0,
        })
    );
    assert_eq!(
        record.record_proof(12, source(1), true),
        Err(PairingRecordError::SourceRateLimited)
    );
    assert_eq!(
        record.record_proof(13, source(2), true),
        Ok(PairingAttemptOutcome::Paired)
    );
}

#[test]
fn expiration_and_clock_regression_fail_closed() {
    let mut record = PairingRecord::<1>::new(id(), 20, 1).unwrap();

    assert_eq!(
        record.record_proof(20, source(1), true),
        Err(PairingRecordError::Expired)
    );
    assert_eq!(record.state(), PairingRecordState::Expired);

    let mut active = PairingRecord::<1>::new(id(), 20, 1).unwrap();
    assert!(active.record_proof(10, source(1), false).is_ok());
    assert_eq!(
        active.record_proof(9, source(1), true),
        Err(PairingRecordError::TimeRegressed)
    );
    assert_eq!(active.state(), PairingRecordState::Expired);
    assert_eq!(
        active.record_proof(11, source(1), true),
        Err(PairingRecordError::Expired)
    );
}

#[test]
fn source_table_is_bounded_without_storing_raw_addresses() {
    let mut record = PairingRecord::<1>::new(id(), 100, 2).unwrap();

    assert!(record.record_proof(1, source(1), false).is_ok());
    assert_eq!(
        record.record_proof(2, source(2), false),
        Err(PairingRecordError::SourceTableFull)
    );
}
