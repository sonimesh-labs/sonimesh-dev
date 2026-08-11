//! Deterministic property coverage for the single-use pairing record lifecycle.
//!
//! The pairing record is the first-pairing rate-limit boundary: an invitation
//! must admit a bounded number of proof attempts per opaque source, consume
//! itself on the first valid proof, expire on clock regression or lifetime
//! exhaustion without ever reviving, and stay permanently terminal once
//! consumed or revoked. The lifecycle is monotonic, so branches that are
//! mutually exclusive in one record (for example a clock regression that
//! expires the invitation versus a later valid proof) are covered by three
//! seeded walks, each on a fresh record and compared step by step against an
//! independent spec model; terminal-state invariants are checked after every
//! operation in every walk.

use sonimesh_app::{
    PairingAttemptOutcome, PairingId, PairingProofVerification, PairingRecord, PairingRecordError,
    PairingRecordState, PairingSource,
};

const MAX_SOURCES: usize = 3;
const MAX_ATTEMPTS_PER_SOURCE: u8 = 3;
const SOURCE_COUNT: usize = 4;

/// Small deterministic xorshift64 generator; no external dependency.
struct XorShift64(u64);

impl XorShift64 {
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 32) as u32
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        self.next_u32() as usize % bound
    }
}

fn id() -> PairingId {
    PairingId::new([0x42; 16])
}

fn source(value: u8) -> PairingSource {
    PairingSource::new([value; 16])
}

/// Independent spec-level model of the pairing record lifecycle. It uses a
/// compact source-attempt list and a single transition rule per operation
/// rather than the implementation's fixed slot table and nested guards, so the
/// two cannot share a structural bug by construction.
struct PairingRecordModel {
    state: PairingRecordState,
    last_observed_ms: u64,
    attempts: Vec<(PairingSource, u8)>,
    paired_count: usize,
}

impl PairingRecordModel {
    fn new() -> Self {
        Self {
            state: PairingRecordState::Active,
            last_observed_ms: 0,
            attempts: Vec::new(),
            paired_count: 0,
        }
    }

    fn observe(
        &mut self,
        now_ms: u64,
        expires_at_ms: u64,
    ) -> Result<PairingRecordState, PairingRecordError> {
        if matches!(
            self.state,
            PairingRecordState::Consumed | PairingRecordState::Revoked
        ) {
            return Ok(self.state);
        }
        if now_ms < self.last_observed_ms {
            self.state = PairingRecordState::Expired;
            return Err(PairingRecordError::TimeRegressed);
        }
        self.last_observed_ms = now_ms;
        if self.state == PairingRecordState::Active && now_ms >= expires_at_ms {
            self.state = PairingRecordState::Expired;
        }
        Ok(self.state)
    }

    fn expire(&mut self) {
        if self.state == PairingRecordState::Active {
            self.state = PairingRecordState::Expired;
        }
    }

    fn revoke(&mut self) {
        self.state = PairingRecordState::Revoked;
    }

    fn verify_proof(
        &mut self,
        now_ms: u64,
        expires_at_ms: u64,
        proof_source: PairingSource,
        result: PairingProofVerification,
    ) -> Result<PairingAttemptOutcome, PairingRecordError> {
        if self.state == PairingRecordState::Consumed {
            return Err(PairingRecordError::Consumed);
        }
        if self.state == PairingRecordState::Revoked {
            return Err(PairingRecordError::Revoked);
        }
        if self.observe(now_ms, expires_at_ms)? == PairingRecordState::Expired {
            return Err(PairingRecordError::Expired);
        }
        if !self
            .attempts
            .iter()
            .any(|(existing, _)| *existing == proof_source)
            && self.attempts.len() >= MAX_SOURCES
        {
            return Err(PairingRecordError::SourceTableFull);
        }
        let index = self
            .attempts
            .iter()
            .position(|(existing, _)| *existing == proof_source)
            .unwrap_or_else(|| {
                self.attempts.push((proof_source, 0));
                self.attempts.len() - 1
            });
        if self.attempts[index].1 >= MAX_ATTEMPTS_PER_SOURCE {
            return Err(PairingRecordError::SourceRateLimited);
        }
        self.attempts[index].1 += 1;
        match result {
            PairingProofVerification::Verified => {
                self.state = PairingRecordState::Consumed;
                self.paired_count += 1;
                Ok(PairingAttemptOutcome::Paired)
            }
            PairingProofVerification::Rejected => Ok(PairingAttemptOutcome::Rejected {
                attempts_remaining: MAX_ATTEMPTS_PER_SOURCE - self.attempts[index].1,
            }),
            PairingProofVerification::Failed => Err(PairingRecordError::VerificationFailed),
        }
    }
}

/// Branch and invariant counters accumulated across the seeded walks.
#[derive(Default)]
struct WalkCounters {
    observe: [usize; 4],
    verify: [usize; 9],
    states: [bool; 4],
}

fn count_observe_outcome(
    real: Result<PairingRecordState, PairingRecordError>,
    counters: &mut WalkCounters,
) {
    match real {
        Ok(PairingRecordState::Active) => counters.observe[0] += 1,
        Ok(PairingRecordState::Expired) => counters.observe[1] += 1,
        Ok(PairingRecordState::Consumed | PairingRecordState::Revoked) => counters.observe[2] += 1,
        Err(PairingRecordError::TimeRegressed) => counters.observe[3] += 1,
        Err(error) => panic!("observe returned an unexpected error: {error:?}"),
    }
}

fn count_verify_outcome(
    real: Result<PairingAttemptOutcome, PairingRecordError>,
    counters: &mut WalkCounters,
) {
    match real {
        Ok(PairingAttemptOutcome::Paired) => counters.verify[0] += 1,
        Ok(PairingAttemptOutcome::Rejected { attempts_remaining }) => {
            assert!(
                attempts_remaining < MAX_ATTEMPTS_PER_SOURCE,
                "remaining attempts must be bounded"
            );
            counters.verify[1] += 1;
        }
        Err(PairingRecordError::SourceRateLimited) => counters.verify[2] += 1,
        Err(PairingRecordError::SourceTableFull) => counters.verify[3] += 1,
        Err(PairingRecordError::Expired) => counters.verify[4] += 1,
        Err(PairingRecordError::Consumed) => counters.verify[5] += 1,
        Err(PairingRecordError::Revoked) => counters.verify[6] += 1,
        Err(PairingRecordError::VerificationFailed) => counters.verify[7] += 1,
        Err(PairingRecordError::TimeRegressed) => counters.verify[8] += 1,
        Err(error) => panic!("verify returned an unexpected error: {error:?}"),
    }
}

/// One deterministic walk over a fresh record. Weights are out of ten for
/// operations and out of one hundred for verification results.
struct WalkConfig {
    seed: u64,
    iterations: usize,
    expires_at_ms: u64,
    observe_weight: u32,
    expire_weight: u32,
    revoke_weight: u32,
    verified_weight: u32,
    rejected_weight: u32,
    max_forward_step: u64,
    regression_frequency: u32,
    revoke_at_end: bool,
}

fn run_walk(config: &WalkConfig, counters: &mut WalkCounters) {
    let mut rng = XorShift64(config.seed);
    let mut record =
        PairingRecord::<MAX_SOURCES>::new(id(), config.expires_at_ms, MAX_ATTEMPTS_PER_SOURCE)
            .expect("valid pairing configuration");
    let mut model = PairingRecordModel::new();
    let mut now_ms = 1_u64;

    for _ in 0..config.iterations {
        if config.regression_frequency != 0 && rng.next_u32() % config.regression_frequency == 0 {
            let step = u64::try_from(rng.next_usize(16)).expect("small bound") + 1;
            now_ms = now_ms.saturating_sub(step);
        } else {
            let bound = usize::try_from(config.max_forward_step).expect("small bound");
            let step = u64::try_from(rng.next_usize(bound)).expect("small bound") + 1;
            now_ms = now_ms.saturating_add(step);
        }

        let operation = rng.next_u32() % 10;
        let previous = record.state();
        if operation < config.observe_weight {
            let real = record.observe(now_ms);
            let expected = model.observe(now_ms, config.expires_at_ms);
            assert_eq!(real, expected, "observe at {now_ms}");
            count_observe_outcome(real, counters);
        } else if operation < config.observe_weight + config.expire_weight {
            record.expire();
            model.expire();
        } else if operation < config.observe_weight + config.expire_weight + config.revoke_weight {
            record.revoke();
            model.revoke();
        } else {
            let source_value = u8::try_from(1 + rng.next_usize(SOURCE_COUNT)).expect("small bound");
            let result_roll = rng.next_u32() % 100;
            let result = if result_roll < config.verified_weight {
                PairingProofVerification::Verified
            } else if result_roll < config.verified_weight + config.rejected_weight {
                PairingProofVerification::Rejected
            } else {
                PairingProofVerification::Failed
            };
            let real = record.verify_proof(now_ms, source(source_value), move || result);
            let expected =
                model.verify_proof(now_ms, config.expires_at_ms, source(source_value), result);
            assert_eq!(
                real, expected,
                "verify at {now_ms} from source {source_value}"
            );
            count_verify_outcome(real, counters);
        }

        assert_eq!(record.state(), model.state, "state drift at {now_ms}");
        match record.state() {
            PairingRecordState::Active => counters.states[0] = true,
            PairingRecordState::Consumed => counters.states[1] = true,
            PairingRecordState::Expired => counters.states[2] = true,
            PairingRecordState::Revoked => counters.states[3] = true,
        }
        if previous == PairingRecordState::Revoked {
            assert_eq!(
                record.state(),
                PairingRecordState::Revoked,
                "revoked must be terminal"
            );
        }
        if previous == PairingRecordState::Consumed {
            assert!(
                matches!(
                    record.state(),
                    PairingRecordState::Consumed | PairingRecordState::Revoked
                ),
                "consumed must stay consumed or be revoked"
            );
        }
    }

    if config.revoke_at_end {
        record.revoke();
        model.revoke();
        assert_eq!(
            record.state(),
            model.state,
            "revoke must agree with the model"
        );
        counters.states[3] = true;
        for _ in 0..64 {
            let real = record.observe(now_ms);
            let expected = model.observe(now_ms, config.expires_at_ms);
            assert_eq!(real, expected, "observe after revocation");
            count_observe_outcome(real, counters);
            let result = PairingProofVerification::Verified;
            let real = record.verify_proof(now_ms, source(1), move || result);
            let expected = model.verify_proof(now_ms, config.expires_at_ms, source(1), result);
            assert_eq!(real, expected, "verify after revocation");
            count_verify_outcome(real, counters);
        }
    }

    assert!(
        model.paired_count <= 1,
        "a single-use invitation must pair at most once"
    );
    assert!(
        model.attempts.len() <= MAX_SOURCES,
        "the source table must stay bounded"
    );
}

/// `record`/`model`/`real`/`expected`/`now` are short local names, not a design
/// smell, so the noisy `similar_names` lint is allowed here.
#[allow(clippy::similar_names)]
#[test]
fn seeded_lifecycle_walks_match_a_spec_model() {
    let mut counters = WalkCounters::default();

    // Clock walk: lifetime expiry and clock regression expire the invitation
    // and proofs are always rejected or failed.
    run_walk(
        &WalkConfig {
            seed: 0xA11C_E202_6000_0001,
            iterations: 60_000,
            expires_at_ms: 10_000,
            observe_weight: 5,
            expire_weight: 0,
            revoke_weight: 0,
            verified_weight: 0,
            rejected_weight: 60,
            max_forward_step: 20,
            regression_frequency: 64,
            revoke_at_end: false,
        },
        &mut counters,
    );

    // Rate-limit walk: rejected and failed proofs saturate every source and
    // fill the fixed source table without consuming the invitation.
    run_walk(
        &WalkConfig {
            seed: 0xA11C_E202_6000_0002,
            iterations: 40_000,
            expires_at_ms: u64::MAX,
            observe_weight: 2,
            expire_weight: 0,
            revoke_weight: 0,
            verified_weight: 0,
            rejected_weight: 70,
            max_forward_step: 4,
            regression_frequency: 0,
            revoke_at_end: false,
        },
        &mut counters,
    );

    // Consume walk: the first verified proof consumes the invitation exactly
    // once, later proofs are rejected as consumed, and a final revocation is
    // terminal.
    run_walk(
        &WalkConfig {
            seed: 0xA11C_E202_6000_0003,
            iterations: 30_000,
            expires_at_ms: u64::MAX,
            observe_weight: 2,
            expire_weight: 0,
            revoke_weight: 0,
            verified_weight: 70,
            rejected_weight: 30,
            max_forward_step: 4,
            regression_frequency: 0,
            revoke_at_end: true,
        },
        &mut counters,
    );

    assert!(
        counters.observe.iter().all(|&count| count > 0),
        "the walks must exercise every observe outcome: {:?}",
        counters.observe
    );
    assert!(
        counters.verify.iter().all(|&count| count > 0),
        "the walks must exercise every verify outcome: {:?}",
        counters.verify
    );
    assert!(
        counters.states.iter().all(|&seen| seen),
        "the walks must reach every lifecycle state: {:?}",
        counters.states
    );
}
