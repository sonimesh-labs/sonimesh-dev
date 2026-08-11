//! Deterministic property coverage for the pairing-invitation orchestration seam.
//!
//! [`PairingInvitationController`] is the only path that may move the desktop
//! session from `Idle` to `Pairing` (through a bounded single-use invitation)
//! and from `Pairing` to `Ready` (through an accepted proof). It must reject
//! every operation that does not match the shared lifecycle, invoke the proof
//! verifier exactly when the session and record admit the attempt, keep the
//! retained record and session phase coordinated, and never let a consumed or
//! expired invitation revive. The record and the session state machine are
//! already proven against independent models elsewhere; this test models the
//! orchestration layer itself and compares every step of seeded random walks
//! against it, including a verifier-invocation counter that pins the
//! fail-closed "reject before verifying" contract.

use core::cell::Cell;

use sonimesh_app::{
    MAX_PAIRING_ATTEMPTS_PER_SOURCE, PairingAttemptOutcome, PairingId, PairingInvitationController,
    PairingInvitationError, PairingProofVerification, PairingRecord, PairingRecordError,
    PairingRecordState, PairingSource, SessionPhase, SessionState,
};

const MAX_SOURCES: usize = 3;
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

fn id(value: u8) -> PairingId {
    PairingId::new([value; 16])
}

fn source(value: u8) -> PairingSource {
    PairingSource::new([value; 16])
}

/// Session phases the controller's own operations can produce.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ModelPhase {
    Idle,
    Pairing,
    Ready,
}

impl ModelPhase {
    const fn as_session_phase(self) -> SessionPhase {
        match self {
            Self::Idle => SessionPhase::Idle,
            Self::Pairing => SessionPhase::Pairing,
            Self::Ready => SessionPhase::Ready,
        }
    }
}

/// Comparable model-side error. [`ModelError::Transition`] is unreachable
/// through the controller's own operations because `StartPairing` is only ever
/// applied from `Idle` and `SessionExpired` is valid from every phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ModelError {
    Record(PairingRecordError),
    Transition,
    ActiveInvitation,
    MissingInvitation,
    ZeroLifetime,
    ExpiryOverflow,
    InvalidSessionState(SessionPhase),
}

impl ModelError {
    fn from_real(error: PairingInvitationError) -> Self {
        match error {
            PairingInvitationError::Record(record) => Self::Record(record),
            PairingInvitationError::Transition(_) => Self::Transition,
            PairingInvitationError::ActiveInvitation => Self::ActiveInvitation,
            PairingInvitationError::MissingInvitation => Self::MissingInvitation,
            PairingInvitationError::ZeroLifetime => Self::ZeroLifetime,
            PairingInvitationError::ExpiryOverflow => Self::ExpiryOverflow,
            PairingInvitationError::InvalidSessionState(state) => {
                Self::InvalidSessionState(state.phase())
            }
        }
    }
}

/// Independent orchestration model. It owns a real [`PairingRecord`] as a
/// trusted primitive (the record's own behavior is proven separately) and a
/// compact session phase, and derives every controller rule from the documented
/// contract: `begin` refuses to replace an active invitation, requires an idle
/// session and a positive non-overflowing lifetime, and only then creates the
/// record; `observe` polls the record clock and coordinates the session after
/// it; `verify_proof` gates on the pairing phase before admitting a proof and
/// only establishes trust for a consumed proof; `expire` and `revoke` retire
/// the record and return the desktop to `Idle` when the record turns terminal.
struct ControllerModel<const MAX_SOURCES: usize> {
    record: Option<PairingRecord<MAX_SOURCES>>,
    phase: ModelPhase,
}

impl<const MAX_SOURCES: usize> ControllerModel<MAX_SOURCES> {
    fn new() -> Self {
        Self {
            record: None,
            phase: ModelPhase::Idle,
        }
    }

    fn record_state(&self) -> Option<PairingRecordState> {
        self.record.as_ref().map(PairingRecord::state)
    }

    fn begin(
        &mut self,
        pair_id: PairingId,
        now_ms: u64,
        lifetime_ms: u64,
        max_attempts_per_source: u8,
    ) -> Result<(), ModelError> {
        if self.record_state() == Some(PairingRecordState::Active) {
            return Err(ModelError::ActiveInvitation);
        }
        if self.phase != ModelPhase::Idle {
            return Err(ModelError::InvalidSessionState(
                self.phase.as_session_phase(),
            ));
        }
        if lifetime_ms == 0 {
            return Err(ModelError::ZeroLifetime);
        }
        let Some(expires_at_ms) = now_ms.checked_add(lifetime_ms) else {
            return Err(ModelError::ExpiryOverflow);
        };
        let mut record = PairingRecord::new(pair_id, expires_at_ms, max_attempts_per_source)
            .map_err(ModelError::Record)?;
        record.observe(now_ms).map_err(ModelError::Record)?;
        self.phase = ModelPhase::Pairing;
        self.record = Some(record);
        Ok(())
    }

    fn observe(&mut self, now_ms: u64) -> Result<PairingRecordState, ModelError> {
        let record = self.record.as_mut().ok_or(ModelError::MissingInvitation)?;
        let observed = record.observe(now_ms);
        self.synchronize()?;
        observed.map_err(ModelError::Record)
    }

    /// Predicts the outcome and whether the proof verifier is invoked.
    fn verify_proof(
        &mut self,
        now_ms: u64,
        proof_source: PairingSource,
        result: PairingProofVerification,
    ) -> (Result<PairingAttemptOutcome, ModelError>, bool) {
        if self.phase != ModelPhase::Pairing {
            return (
                Err(ModelError::InvalidSessionState(
                    self.phase.as_session_phase(),
                )),
                false,
            );
        }
        let Some(record) = self.record.as_mut() else {
            return (Err(ModelError::MissingInvitation), false);
        };
        let invoked = Cell::new(false);
        let outcome = record.verify_proof(now_ms, proof_source, || {
            invoked.set(true);
            result
        });
        let mapped = match outcome {
            Ok(PairingAttemptOutcome::Paired) => {
                self.phase = ModelPhase::Ready;
                Ok(PairingAttemptOutcome::Paired)
            }
            Ok(rejected @ PairingAttemptOutcome::Rejected { .. }) => Ok(rejected),
            Err(error) => match self.synchronize() {
                Ok(()) => Err(ModelError::Record(error)),
                Err(sync_error) => Err(sync_error),
            },
        };
        (mapped, invoked.get())
    }

    fn expire(&mut self) -> Result<PairingRecordState, ModelError> {
        let state = {
            let record = self.record.as_mut().ok_or(ModelError::MissingInvitation)?;
            record.expire();
            record.state()
        };
        self.synchronize()?;
        Ok(state)
    }

    fn revoke(&mut self) -> Result<PairingRecordState, ModelError> {
        let state = {
            let record = self.record.as_mut().ok_or(ModelError::MissingInvitation)?;
            record.revoke();
            record.state()
        };
        self.synchronize()?;
        Ok(state)
    }

    fn synchronize(&mut self) -> Result<(), ModelError> {
        match self.record_state() {
            Some(PairingRecordState::Active) if self.phase != ModelPhase::Pairing => Err(
                ModelError::InvalidSessionState(self.phase.as_session_phase()),
            ),
            Some(PairingRecordState::Expired | PairingRecordState::Revoked) => {
                self.phase = ModelPhase::Idle;
                Ok(())
            }
            Some(PairingRecordState::Active | PairingRecordState::Consumed) | None => Ok(()),
        }
    }
}

/// Branch counters accumulated across the seeded walks.
#[derive(Default)]
struct WalkCounters {
    begin: [usize; 6],
    observe: [usize; 7],
    verify: [usize; 8],
    expire: [usize; 5],
    revoke: [usize; 2],
    verifier_invoked: usize,
    verifier_skipped: usize,
}

impl WalkCounters {
    fn merge(&mut self, other: &Self) {
        for (slot, value) in self.begin.iter_mut().zip(other.begin) {
            *slot += value;
        }
        for (slot, value) in self.observe.iter_mut().zip(other.observe) {
            *slot += value;
        }
        for (slot, value) in self.verify.iter_mut().zip(other.verify) {
            *slot += value;
        }
        for (slot, value) in self.expire.iter_mut().zip(other.expire) {
            *slot += value;
        }
        for (slot, value) in self.revoke.iter_mut().zip(other.revoke) {
            *slot += value;
        }
        self.verifier_invoked += other.verifier_invoked;
        self.verifier_skipped += other.verifier_skipped;
    }
}

/// Deterministic walk parameters. Verification-result weights are out of 100.
struct WalkConfig {
    seed: u64,
    iterations: usize,
    regression_frequency: u32,
    clock_step_max: u64,
    lifetime_max: u64,
    max_attempts_max: u8,
    verified_weight: u32,
    rejected_weight: u32,
}

struct Walk<'a> {
    controller: PairingInvitationController<MAX_SOURCES>,
    session: SessionState,
    model: ControllerModel<MAX_SOURCES>,
    rng: XorShift64,
    config: &'a WalkConfig,
    counters: &'a mut WalkCounters,
    next_id: u8,
    now_ms: u64,
}

impl Walk<'_> {
    fn next_clock(&mut self) -> u64 {
        if self.config.regression_frequency != 0
            && self.rng.next_u32() % self.config.regression_frequency == 0
        {
            let step = u64::try_from(self.rng.next_usize(16)).expect("small bound") + 1;
            self.now_ms = self.now_ms.saturating_sub(step).max(1);
        } else {
            let bound = usize::try_from(self.config.clock_step_max).expect("small bound");
            let step = u64::try_from(self.rng.next_usize(bound)).expect("small bound") + 1;
            self.now_ms = self.now_ms.saturating_add(step);
        }
        self.now_ms
    }

    fn assert_coordinated(&self, context: &str) {
        assert_eq!(
            self.session.phase(),
            self.model.phase.as_session_phase(),
            "session phase drift after {context}"
        );
        assert_eq!(
            self.controller.state(),
            self.model.record_state(),
            "retained record state drift after {context}"
        );
        match self.controller.state() {
            Some(PairingRecordState::Active) => assert_eq!(
                self.session.phase(),
                SessionPhase::Pairing,
                "active invitation without a pairing session after {context}"
            ),
            Some(PairingRecordState::Expired | PairingRecordState::Revoked) => assert_eq!(
                self.session.phase(),
                SessionPhase::Idle,
                "terminal invitation without an idle session after {context}"
            ),
            Some(PairingRecordState::Consumed) => assert_ne!(
                self.session.phase(),
                SessionPhase::Pairing,
                "consumed invitation still in a pairing session after {context}"
            ),
            None => {}
        }
    }

    fn begin_params(&mut self) -> (PairingId, u64, u64, u8) {
        let pair_id = id(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        let now_ms = self.next_clock();
        let lifetime_roll = self.rng.next_u32() % 512;
        let lifetime_ms = if lifetime_roll < 16 {
            0
        } else if lifetime_roll < 32 {
            // Larger than `u64::MAX - now_ms`, so the clock addition overflows.
            u64::MAX - now_ms + 1
        } else {
            u64::from(self.rng.next_u32() % 512) % self.config.lifetime_max + 1
        };
        let attempts_roll = self.rng.next_u32() % 32;
        let max_attempts_per_source = if attempts_roll < 2 {
            0
        } else if attempts_roll < 4 {
            MAX_PAIRING_ATTEMPTS_PER_SOURCE + 1
        } else {
            u8::try_from(
                self.rng
                    .next_usize(usize::from(self.config.max_attempts_max)),
            )
            .expect("small bound")
                + 1
        };
        (pair_id, now_ms, lifetime_ms, max_attempts_per_source)
    }

    fn step_begin(&mut self) {
        let (pair_id, now_ms, lifetime_ms, max_attempts_per_source) = self.begin_params();
        let real = self.controller.begin(
            &mut self.session,
            pair_id,
            now_ms,
            lifetime_ms,
            max_attempts_per_source,
        );
        let expected = self
            .model
            .begin(pair_id, now_ms, lifetime_ms, max_attempts_per_source);
        assert_eq!(
            real.map_err(ModelError::from_real),
            expected,
            "begin at {now_ms} lifetime {lifetime_ms} attempts {max_attempts_per_source}"
        );
        self.assert_coordinated("begin");
        match real {
            Ok(()) => self.counters.begin[0] += 1,
            Err(PairingInvitationError::ActiveInvitation) => self.counters.begin[1] += 1,
            Err(PairingInvitationError::InvalidSessionState(_)) => self.counters.begin[2] += 1,
            Err(PairingInvitationError::ZeroLifetime) => self.counters.begin[3] += 1,
            Err(PairingInvitationError::ExpiryOverflow) => self.counters.begin[4] += 1,
            Err(PairingInvitationError::Record(PairingRecordError::InvalidAttemptLimit)) => {
                self.counters.begin[5] += 1;
            }
            Err(error) => panic!("begin reached an unexpected error: {error:?}"),
        }
    }

    fn step_observe(&mut self) {
        let now_ms = self.next_clock();
        let real = self.controller.observe(&mut self.session, now_ms);
        let expected = self.model.observe(now_ms);
        assert_eq!(
            real.map_err(ModelError::from_real),
            expected,
            "observe at {now_ms}"
        );
        self.assert_coordinated("observe");
        match real {
            Err(PairingInvitationError::MissingInvitation) => self.counters.observe[0] += 1,
            Ok(PairingRecordState::Active) => self.counters.observe[1] += 1,
            Ok(PairingRecordState::Expired) => self.counters.observe[2] += 1,
            Ok(PairingRecordState::Consumed) => self.counters.observe[3] += 1,
            Ok(PairingRecordState::Revoked) => self.counters.observe[4] += 1,
            Err(PairingInvitationError::Record(PairingRecordError::TimeRegressed)) => {
                self.counters.observe[5] += 1;
            }
            Err(PairingInvitationError::InvalidSessionState(_)) => self.counters.observe[6] += 1,
            Err(error) => panic!("observe reached an unexpected error: {error:?}"),
        }
    }

    fn step_verify(&mut self) {
        let now_ms = self.next_clock();
        let source_index = self.rng.next_usize(SOURCE_COUNT);
        let proof_source = source(u8::try_from(source_index).expect("small bound") + 1);
        let result_roll = self.rng.next_u32() % 100;
        let result = if result_roll < self.config.verified_weight {
            PairingProofVerification::Verified
        } else if result_roll < self.config.verified_weight + self.config.rejected_weight {
            PairingProofVerification::Rejected
        } else {
            PairingProofVerification::Failed
        };

        let verifier_calls = Cell::new(0_u32);
        let real = self
            .controller
            .verify_proof(&mut self.session, now_ms, proof_source, || {
                verifier_calls.set(verifier_calls.get() + 1);
                result
            });
        let (expected, verifier_should_run) = self.model.verify_proof(now_ms, proof_source, result);

        assert_eq!(
            real.map_err(ModelError::from_real),
            expected,
            "verify at {now_ms} from source {source_index}"
        );
        assert_eq!(
            verifier_calls.get(),
            u32::from(verifier_should_run),
            "verifier invocation mismatch at {now_ms} from source {source_index}"
        );
        self.assert_coordinated("verify");

        if verifier_calls.get() == 1 {
            self.counters.verifier_invoked += 1;
        } else {
            self.counters.verifier_skipped += 1;
        }

        match real {
            Ok(PairingAttemptOutcome::Paired) => self.counters.verify[0] += 1,
            Ok(PairingAttemptOutcome::Rejected { .. }) => self.counters.verify[1] += 1,
            Err(PairingInvitationError::InvalidSessionState(_)) => self.counters.verify[2] += 1,
            Err(PairingInvitationError::Record(PairingRecordError::TimeRegressed)) => {
                self.counters.verify[3] += 1;
            }
            Err(PairingInvitationError::Record(PairingRecordError::Expired)) => {
                self.counters.verify[4] += 1;
            }
            Err(PairingInvitationError::Record(PairingRecordError::SourceRateLimited)) => {
                self.counters.verify[5] += 1;
            }
            Err(PairingInvitationError::Record(PairingRecordError::SourceTableFull)) => {
                self.counters.verify[6] += 1;
            }
            Err(PairingInvitationError::Record(PairingRecordError::VerificationFailed)) => {
                self.counters.verify[7] += 1;
            }
            Err(error) => panic!("verify reached an unexpected error: {error:?}"),
        }
    }

    fn step_expire(&mut self) {
        let real = self.controller.expire(&mut self.session);
        let expected = self.model.expire();
        assert_eq!(real.map_err(ModelError::from_real), expected, "expire");
        self.assert_coordinated("expire");
        match real {
            Err(PairingInvitationError::MissingInvitation) => self.counters.expire[0] += 1,
            Ok(PairingRecordState::Expired) => self.counters.expire[1] += 1,
            Ok(PairingRecordState::Consumed) => self.counters.expire[2] += 1,
            Ok(PairingRecordState::Revoked) => self.counters.expire[3] += 1,
            Err(PairingInvitationError::InvalidSessionState(_)) => self.counters.expire[4] += 1,
            Ok(PairingRecordState::Active) | Err(_) => {
                panic!("expire reached an unexpected result: {real:?}");
            }
        }
    }

    fn step_revoke(&mut self) {
        let real = self.controller.revoke(&mut self.session);
        let expected = self.model.revoke();
        assert_eq!(real.map_err(ModelError::from_real), expected, "revoke");
        self.assert_coordinated("revoke");
        match real {
            Err(PairingInvitationError::MissingInvitation) => self.counters.revoke[0] += 1,
            Ok(PairingRecordState::Revoked) => self.counters.revoke[1] += 1,
            Ok(_) | Err(_) => panic!("revoke reached an unexpected result: {real:?}"),
        }
    }
}

fn run_walk(config: &WalkConfig) -> WalkCounters {
    let mut counters = WalkCounters::default();
    let mut walk = Walk {
        controller: PairingInvitationController::new(),
        session: SessionState::IDLE,
        model: ControllerModel::new(),
        rng: XorShift64(config.seed),
        config,
        counters: &mut counters,
        next_id: 1,
        now_ms: 1,
    };

    // Scripted prologue: with no invitation retained, poll and retire
    // operations must report a missing invitation and the verifier must never
    // run outside a pairing session.
    walk.step_observe();
    walk.step_expire();
    walk.step_revoke();
    walk.step_verify();

    for _ in 0..config.iterations {
        let operation = walk.rng.next_u32() % 100;
        if operation < 22 {
            walk.step_begin();
        } else if operation < 34 {
            walk.step_observe();
        } else if operation < 66 {
            walk.step_verify();
        } else if operation < 84 {
            walk.step_expire();
        } else {
            walk.step_revoke();
        }
    }
    counters
}

fn assert_required_coverage(counters: &WalkCounters) {
    for (index, name) in [
        (0, "begin_ok"),
        (1, "begin_active_invitation"),
        (2, "begin_invalid_session"),
        (3, "begin_zero_lifetime"),
        (4, "begin_expiry_overflow"),
        (5, "begin_invalid_attempt_limit"),
    ] {
        assert!(counters.begin[index] > 0, "{name} was never exercised");
    }
    for (index, name) in [
        (0, "observe_missing"),
        (1, "observe_active"),
        (2, "observe_expired"),
        (3, "observe_consumed"),
        (4, "observe_revoked"),
        (5, "observe_time_regressed"),
    ] {
        assert!(counters.observe[index] > 0, "{name} was never exercised");
    }
    for (index, name) in [
        (0, "verify_paired"),
        (1, "verify_rejected"),
        (2, "verify_invalid_session"),
        (3, "verify_time_regressed"),
        (4, "verify_expired"),
        (5, "verify_rate_limited"),
        (6, "verify_table_full"),
        (7, "verify_verification_failed"),
    ] {
        assert!(counters.verify[index] > 0, "{name} was never exercised");
    }
    for (index, name) in [
        (0, "expire_missing"),
        (1, "expire_expired"),
        (2, "expire_consumed"),
        (3, "expire_revoked"),
    ] {
        assert!(counters.expire[index] > 0, "{name} was never exercised");
    }
    assert!(counters.revoke[0] > 0, "revoke_missing was never exercised");
    assert!(counters.revoke[1] > 0, "revoke_ok was never exercised");
    assert!(counters.verifier_invoked > 0, "verifier was never invoked");
    assert!(
        counters.verifier_skipped > 0,
        "verifier skip path was never exercised"
    );
    // Documented orchestration invariants: an active invitation never coexists
    // with a non-pairing session through the controller's own operations, and
    // its two session events are always valid.
    assert_eq!(
        counters.observe[6], 0,
        "active record was observed outside a pairing session"
    );
    assert_eq!(
        counters.expire[4], 0,
        "expire reported an invalid session state"
    );
}

#[allow(clippy::similar_names)]
#[test]
fn seeded_orchestration_walks_match_a_spec_model() {
    let mut counters = WalkCounters::default();
    counters.merge(&run_walk(&WalkConfig {
        seed: 0xA11C_E202_6000_0011,
        iterations: 120_000,
        regression_frequency: 32,
        clock_step_max: 10,
        lifetime_max: 400,
        max_attempts_max: 3,
        verified_weight: 25,
        rejected_weight: 45,
    }));
    counters.merge(&run_walk(&WalkConfig {
        seed: 0xA11C_E202_6000_0012,
        iterations: 120_000,
        regression_frequency: 0,
        clock_step_max: 6,
        lifetime_max: 1_000,
        max_attempts_max: 2,
        verified_weight: 10,
        rejected_weight: 60,
    }));
    assert_required_coverage(&counters);
}
