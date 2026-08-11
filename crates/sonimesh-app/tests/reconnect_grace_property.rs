//! Deterministic property coverage for the reconnect-grace window lifecycle.
//!
//! The reconnect-grace window bounds how long a lost control session may stay
//! in the reconnecting lifecycle: it must either reconnect with fresh session
//! material before the deadline or expire, and a clock regression must fail
//! closed by force-expiring the window. The lifecycle is monotonic, so three
//! seeded walks over a fresh controller are compared step by step against an
//! independent spec model; terminal and bounded-remaining invariants are
//! checked after every operation.

use sonimesh_app::{
    ReconnectGraceController, ReconnectGraceError, ReconnectGraceObservation, ReconnectGraceState,
};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ModelState {
    Idle,
    InGrace,
}

/// Independent spec-level model of the grace window. It uses a single
/// transition rule per operation rather than the implementation's field
/// update order, so the two cannot share a structural bug by construction.
struct GraceModel {
    state: ModelState,
    deadline_ms: u64,
    last_observed_ms: u64,
}

impl GraceModel {
    fn new() -> Self {
        Self {
            state: ModelState::Idle,
            deadline_ms: 0,
            last_observed_ms: 0,
        }
    }

    fn begin(&mut self, now_ms: u64, duration_ms: u64) -> Result<(), ReconnectGraceError> {
        if self.state == ModelState::InGrace {
            return Err(ReconnectGraceError::AlreadyInGrace);
        }
        if duration_ms == 0 {
            return Err(ReconnectGraceError::ZeroDuration);
        }
        let Some(deadline_ms) = now_ms.checked_add(duration_ms) else {
            return Err(ReconnectGraceError::ExpiryOverflow);
        };
        self.state = ModelState::InGrace;
        self.deadline_ms = deadline_ms;
        self.last_observed_ms = now_ms;
        Ok(())
    }

    fn observe(&mut self, now_ms: u64) -> Result<ReconnectGraceObservation, ReconnectGraceError> {
        if self.state == ModelState::Idle {
            return Ok(ReconnectGraceObservation::Idle);
        }
        if now_ms < self.last_observed_ms {
            self.reset();
            return Err(ReconnectGraceError::TimeRegressed);
        }
        self.last_observed_ms = now_ms;
        if now_ms >= self.deadline_ms {
            self.reset();
            return Ok(ReconnectGraceObservation::Expired);
        }
        Ok(ReconnectGraceObservation::InGrace {
            remaining_ms: self.deadline_ms - now_ms,
        })
    }

    fn reconnected(&mut self) -> bool {
        let was_active = self.state == ModelState::InGrace;
        self.reset();
        was_active
    }

    fn reset(&mut self) {
        self.state = ModelState::Idle;
        self.deadline_ms = 0;
        self.last_observed_ms = 0;
    }
}

/// Branch and invariant counters accumulated across the seeded walks.
#[derive(Default)]
struct WalkCounters {
    begin: [usize; 4],
    observe: [usize; 4],
    states: [bool; 2],
    expirations: usize,
}

fn count_begin_outcome(real: Result<(), ReconnectGraceError>, counters: &mut WalkCounters) {
    match real {
        Ok(()) => counters.begin[0] += 1,
        Err(ReconnectGraceError::AlreadyInGrace) => counters.begin[1] += 1,
        Err(ReconnectGraceError::ZeroDuration) => counters.begin[2] += 1,
        Err(ReconnectGraceError::ExpiryOverflow) => counters.begin[3] += 1,
        Err(ReconnectGraceError::TimeRegressed) => {
            panic!("begin must never report a clock regression")
        }
    }
}

fn count_observe_outcome(
    real: Result<ReconnectGraceObservation, ReconnectGraceError>,
    counters: &mut WalkCounters,
) {
    match real {
        Ok(ReconnectGraceObservation::Idle) => counters.observe[0] += 1,
        Ok(ReconnectGraceObservation::InGrace { .. }) => counters.observe[1] += 1,
        Ok(ReconnectGraceObservation::Expired) => {
            counters.observe[2] += 1;
            counters.expirations += 1;
        }
        Err(ReconnectGraceError::TimeRegressed) => counters.observe[3] += 1,
        Err(error) => panic!("observe returned an unexpected error: {error:?}"),
    }
}

/// One deterministic walk over a fresh controller. Weights are out of ten for
/// operations.
struct WalkConfig {
    seed: u64,
    iterations: usize,
    begin_weight: u32,
    observe_weight: u32,
    max_forward_step: u64,
    regression_frequency: u32,
}

fn run_walk(config: &WalkConfig, counters: &mut WalkCounters) {
    let mut rng = XorShift64(config.seed);
    let mut grace = ReconnectGraceController::new();
    let mut model = GraceModel::new();
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
        if operation < config.begin_weight {
            // Occasionally exercise the zero-duration and overflow rejections
            // so every begin branch is covered by the walk.
            let duration_roll = rng.next_u32() % 100;
            let duration_ms = if duration_roll < 2 {
                0
            } else if duration_roll < 4 {
                u64::MAX
            } else {
                u64::try_from(rng.next_usize(10)).expect("small bound") + 1
            };
            let real = grace.begin(now_ms, duration_ms);
            let expected = model.begin(now_ms, duration_ms);
            assert_eq!(real, expected, "begin at {now_ms} duration {duration_ms}");
            count_begin_outcome(real, counters);
        } else if operation < config.begin_weight + config.observe_weight {
            let real = grace.observe(now_ms);
            let expected = model.observe(now_ms);
            assert_eq!(real, expected, "observe at {now_ms}");
            if let Ok(ReconnectGraceObservation::InGrace { remaining_ms }) = real {
                assert!(
                    remaining_ms >= 1,
                    "remaining window must stay positive at {now_ms}"
                );
            }
            count_observe_outcome(real, counters);
        } else if operation < config.begin_weight + config.observe_weight + 1 {
            let real = grace.reconnected();
            let expected = model.reconnected();
            assert_eq!(real, expected, "reconnected at {now_ms}");
        } else {
            let real = grace.cancel();
            let expected = model.reconnected();
            assert_eq!(real, expected, "cancel at {now_ms}");
        }

        assert_eq!(
            grace.state(),
            match model.state {
                ModelState::Idle => ReconnectGraceState::Idle,
                ModelState::InGrace => ReconnectGraceState::InGrace,
            },
            "state drift at {now_ms}"
        );
        match grace.state() {
            ReconnectGraceState::Idle => counters.states[0] = true,
            ReconnectGraceState::InGrace => counters.states[1] = true,
        }
    }
}

/// `model` and `grace` are short local names, not a design smell, so the noisy
/// `similar_names` lint is allowed here.
#[allow(clippy::similar_names)]
#[test]
fn seeded_lifecycle_walks_match_a_spec_model() {
    let mut counters = WalkCounters::default();

    let configs = [
        // Observe-heavy with frequent clock regressions.
        WalkConfig {
            seed: 0x5EED_0001,
            iterations: 40_000,
            begin_weight: 3,
            observe_weight: 6,
            max_forward_step: 64,
            regression_frequency: 25,
        },
        // Begin-heavy with a tight forward clock, forcing early expiration.
        WalkConfig {
            seed: 0x5EED_0002,
            iterations: 40_000,
            begin_weight: 6,
            observe_weight: 3,
            max_forward_step: 3,
            regression_frequency: 0,
        },
        // Balanced walk with rare regressions and long forward jumps.
        WalkConfig {
            seed: 0x5EED_0003,
            iterations: 40_000,
            begin_weight: 4,
            observe_weight: 4,
            max_forward_step: 4_096,
            regression_frequency: 97,
        },
    ];

    for config in &configs {
        run_walk(config, &mut counters);
    }

    // Every branch was exercised across the walks.
    assert_ne!(counters.begin[0], 0, "begin success branch never hit");
    assert_ne!(
        counters.begin[1], 0,
        "begin already-in-grace branch never hit"
    );
    assert_ne!(counters.begin[2], 0, "begin zero-duration branch never hit");
    assert_ne!(counters.begin[3], 0, "begin overflow branch never hit");
    assert_ne!(counters.observe[0], 0, "observe idle branch never hit");
    assert_ne!(counters.observe[1], 0, "observe in-grace branch never hit");
    assert_ne!(counters.observe[2], 0, "observe expired branch never hit");
    assert_ne!(
        counters.observe[3], 0,
        "observe regression branch never hit"
    );
    assert!(counters.expirations > 0, "expiration path never exercised");
    assert!(
        counters.states[0] && counters.states[1],
        "state space incomplete"
    );
}
