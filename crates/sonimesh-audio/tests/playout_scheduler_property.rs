//! Deterministic property coverage for the target-gated playout scheduler.
//!
//! The scheduler is a bounded scheduling-domain decision: every tick is
//! exactly one gated wait or one played-out frame, the consecutive gate
//! streak never exceeds the configured deadline, a play below the target with
//! arrivals still pending is forced (and only then), and reset returns the
//! scheduler to a fresh-epoch state. This test drives seeded random water
//! levels, targets, and in-flight signals and asserts those invariants over a
//! long run.

use sonimesh_audio::{
    MAX_GATE_TICKS, PlayoutDecision, TargetGatedPlayout, TargetGatedPlayoutConfig,
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

/// Jitter-buffer capacity used to bound the buffered-frame inputs.
const BUFFER_CAPACITY: usize = 64;
/// Number of seeded decisions per property run.
const STEPS: usize = 20_000;

/// Runs one seeded walk and asserts every scheduler invariant after each step.
fn assert_walk_invariants(seed: u64, max_gate_ticks: u32) {
    let mut random = XorShift64(seed);
    let mut scheduler =
        TargetGatedPlayout::new(TargetGatedPlayoutConfig { max_gate_ticks }).unwrap();
    let mut played = 0_u64;
    let mut ticks = 0_u64;

    for _ in 0..STEPS {
        let buffered = random.next_usize(BUFFER_CAPACITY + 1);
        let target = random.next_usize(32) + 1;
        let arrivals_pending = random.next_u32() % 2 == 0;
        let below_target = buffered < target;

        let gated_before = scheduler.gated_in_a_row();
        let gated_ticks_before = scheduler.gated_ticks();
        let forced_before = scheduler.forced_playouts();

        let decision = scheduler.decide(buffered, target, arrivals_pending);
        ticks += 1;

        match decision {
            PlayoutDecision::Gate => {
                assert_eq!(
                    scheduler.gated_in_a_row(),
                    gated_before + 1,
                    "each gate grows the streak by one"
                );
                assert!(
                    scheduler.gated_in_a_row() <= max_gate_ticks,
                    "streak {} exceeded deadline {max_gate_ticks}",
                    scheduler.gated_in_a_row()
                );
                assert_eq!(
                    scheduler.gated_ticks(),
                    gated_ticks_before + 1,
                    "each gate grows the gated-tick counter"
                );
                assert_eq!(
                    scheduler.forced_playouts(),
                    forced_before,
                    "a gate never forces playout"
                );
                assert!(
                    below_target && arrivals_pending,
                    "a gate requires a pending, below-target stream"
                );
            }
            PlayoutDecision::Play => {
                assert_eq!(
                    scheduler.gated_in_a_row(),
                    0,
                    "a play resets the gate streak"
                );
                assert_eq!(
                    scheduler.gated_ticks(),
                    gated_ticks_before,
                    "a play never grows the gated-tick counter"
                );
                let forced_delta = u64::from(below_target && arrivals_pending);
                assert_eq!(
                    scheduler.forced_playouts(),
                    forced_before + forced_delta,
                    "a below-target play with pending arrivals is exactly the forced case"
                );
                played += 1;
            }
        }

        assert_eq!(
            scheduler.gated_ticks() + played,
            ticks,
            "each tick is exactly one gated wait or one played frame"
        );
        assert!(
            scheduler.forced_playouts() <= played,
            "forced playouts cannot exceed played frames"
        );
    }

    // A stream reset must return the scheduler to a fresh-epoch state.
    scheduler.reset();
    assert_eq!(scheduler.gated_in_a_row(), 0);
    assert_eq!(scheduler.gated_ticks(), 0);
    assert_eq!(scheduler.forced_playouts(), 0);
}

#[test]
fn every_seeded_walk_keeps_gate_deadline_and_counting_invariants() {
    for seed in [
        0x5EED_2026_0817_0001,
        0x0bad_c0de_00ff_0001,
        0xACE0_0000_0000_0001,
    ] {
        for max_gate_ticks in [1_u32, 2, 7, 16, 64] {
            assert_walk_invariants(seed ^ u64::from(max_gate_ticks), max_gate_ticks);
        }
    }
}

#[test]
fn a_zero_or_oversized_deadline_never_constructs() {
    assert!(TargetGatedPlayout::new(TargetGatedPlayoutConfig { max_gate_ticks: 0 }).is_err());
    assert!(
        TargetGatedPlayout::new(TargetGatedPlayoutConfig {
            max_gate_ticks: MAX_GATE_TICKS + 1
        })
        .is_err()
    );
    assert!(
        TargetGatedPlayout::new(TargetGatedPlayoutConfig {
            max_gate_ticks: MAX_GATE_TICKS
        })
        .is_ok()
    );
}
