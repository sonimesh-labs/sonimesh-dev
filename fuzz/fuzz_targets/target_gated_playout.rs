#![no_main]

//! Fuzz the bounded target-gated playout primitive (ADR 0016).
//!
//! Arbitrary gate deadlines and per-tick buffered, target, and
//! pending-arrival inputs drive `TargetGatedPlayout` through multi-tick
//! histories and resets. The target proves adversarial timing cannot panic the
//! scheduler, push the gate streak past the configured deadline, let the
//! cumulative gate count fall below the live streak, or count a forced playout
//! without an expired gate deadline. Rejected deadlines must return a clean
//! config error.

use libfuzzer_sys::fuzz_target;
use sonimesh_audio::{
    MAX_GATE_TICKS, PlayoutDecision, TargetGatedPlayout, TargetGatedPlayoutConfig,
};

fuzz_target!(|data: &[u8]| {
    // The first byte selects one deadline candidate for the whole scheduler
    // epoch. Each later five-byte chunk is one playout tick: buffered and
    // target water levels as little-endian u16s, then flags (bit 0 = arrivals
    // pending, bit 1 = reset before this tick).
    let deadline = u32::from(data.first().copied().unwrap_or(0)) % (MAX_GATE_TICKS + 2);
    let config = TargetGatedPlayoutConfig {
        max_gate_ticks: deadline,
    };
    let validated = config.validate();
    let mut scheduler = match TargetGatedPlayout::new(config) {
        Ok(scheduler) => {
            assert!(validated.is_ok(), "an accepted deadline must validate");
            scheduler
        }
        Err(_) => {
            assert!(validated.is_err(), "a rejected deadline must not validate");
            return;
        }
    };

    let mut expected_streak = 0_u32;
    let mut expected_gated_ticks = 0_u64;
    let mut expected_forced_playouts = 0_u64;

    for chunk in data.get(1..).unwrap_or_default().chunks(5) {
        let buffered = usize::from(u16::from_le_bytes([
            chunk.first().copied().unwrap_or(0),
            chunk.get(1).copied().unwrap_or(0),
        ]));
        let target = usize::from(u16::from_le_bytes([
            chunk.get(2).copied().unwrap_or(0),
            chunk.get(3).copied().unwrap_or(0),
        ]));
        let flags = chunk.get(4).copied().unwrap_or(0);
        let arrivals_pending = flags & 1 != 0;
        let reset = flags & 2 != 0;

        if reset {
            scheduler.reset();
            expected_streak = 0;
            expected_gated_ticks = 0;
            expected_forced_playouts = 0;
        }

        let below_target = buffered < target;
        let expected_decision =
            if below_target && arrivals_pending && expected_streak < config.max_gate_ticks {
                PlayoutDecision::Gate
            } else {
                PlayoutDecision::Play
            };
        let decision = scheduler.decide(buffered, target, arrivals_pending);
        assert_eq!(
            decision, expected_decision,
            "the decision matches the bounded model"
        );

        match decision {
            PlayoutDecision::Gate => {
                expected_streak = expected_streak.saturating_add(1);
                expected_gated_ticks = expected_gated_ticks.saturating_add(1);
            }
            PlayoutDecision::Play => {
                if below_target && arrivals_pending {
                    expected_forced_playouts = expected_forced_playouts.saturating_add(1);
                }
                expected_streak = 0;
            }
        }

        assert_eq!(
            scheduler.gated_in_a_row(),
            expected_streak,
            "the live streak matches the model"
        );
        assert_eq!(
            scheduler.gated_ticks(),
            expected_gated_ticks,
            "the cumulative gated tick count matches the model"
        );
        assert_eq!(
            scheduler.forced_playouts(),
            expected_forced_playouts,
            "the forced playout count matches the model"
        );
        assert!(
            scheduler.gated_in_a_row() <= config.max_gate_ticks,
            "the gate streak must stay within the configured deadline"
        );
        assert!(
            scheduler.gated_ticks() >= u64::from(scheduler.gated_in_a_row()),
            "the cumulative gate count must never fall below the live streak"
        );
        assert!(
            scheduler.forced_playouts() <= scheduler.gated_ticks(),
            "every forced playout must follow at least one gated tick"
        );
    }
});
