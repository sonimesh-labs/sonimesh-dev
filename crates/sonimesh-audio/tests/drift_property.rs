//! Deterministic property coverage for the bounded PCM occupancy controller.
//!
//! ADR 0007 commits the drift controller to hard boundedness guarantees:
//! every requested adjustment stays within `max_adjustment_ppm`, consecutive
//! adjustments move by at most `max_step_ppm`, and emergency resynchronization
//! fires only when occupancy actually crosses a threshold. This test drives a
//! fixed pseudo-random occupancy sequence (the full `u16` range so emergency
//! thresholds are crossed regularly) and checks every decision against those
//! invariants, mirroring the deterministic property pattern used for the
//! protocol replay window.

use sonimesh_audio::{DriftConfig, DriftController, DriftDecision, DriftResyncDirection};

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
}

fn config(
    proportional_ppm_per_block: i32,
    integral_ppm_per_block_tick: i32,
    max_adjustment_ppm: i32,
    max_step_ppm: i32,
    max_integral_block_ticks: i32,
) -> DriftConfig {
    DriftConfig {
        target_blocks: 4,
        low_resync_blocks: 1,
        high_resync_blocks: 8,
        proportional_ppm_per_block,
        integral_ppm_per_block_tick,
        max_adjustment_ppm,
        max_step_ppm,
        max_integral_block_ticks,
    }
}

fn assert_bounded_sequence(config: DriftConfig, occupancies: impl IntoIterator<Item = u16>) {
    let mut controller = DriftController::new(config).expect("property config must validate");
    // Nominal output at construction; a resynchronization resets it to zero.
    let mut previous_ppm = 0_i32;

    for occupancy in occupancies {
        match controller.update(occupancy) {
            DriftDecision::HardResync { direction } => {
                let expected = if occupancy <= config.low_resync_blocks {
                    DriftResyncDirection::Low
                } else {
                    DriftResyncDirection::High
                };
                assert_eq!(
                    direction, expected,
                    "resync direction must match the crossed boundary"
                );
                assert_eq!(
                    controller.adjustment_ppm(),
                    0,
                    "resynchronization must clear adjustment history"
                );
                previous_ppm = 0;
            }
            DriftDecision::Adjust { ppm } => {
                assert!(
                    ppm.abs() <= config.max_adjustment_ppm,
                    "adjustment {ppm} exceeds the {0} ppm ratio cap",
                    config.max_adjustment_ppm
                );
                assert!(
                    (ppm - previous_ppm).abs() <= config.max_step_ppm,
                    "adjustment moved from {previous_ppm} to {ppm}, exceeding the {0} ppm step cap",
                    config.max_step_ppm
                );
                previous_ppm = ppm;
            }
        }
    }
}

/// Occupancy is drawn from the low 16 RNG bits; truncation is intentional.
#[allow(clippy::cast_possible_truncation)]
#[test]
fn adjustments_stay_within_ratio_and_slew_bounds_over_arbitrary_occupancy() {
    let cases = [
        (config(100, 10, 500, 50, 16), 0xD011_2026_0000_0001),
        (config(100, 100, 500, 500, 16), 0xD011_2026_0000_0002),
        (config(100, 10, 100, 100, 16), 0xD011_2026_0000_0003),
        (config(25, 5, 200, 20, 4), 0xD011_2026_0000_0004),
    ];

    for (config, seed) in cases {
        let mut rng = XorShift64(seed);
        let occupancies = (0..50_000).map(|_| rng.next_u32() as u16);
        assert_bounded_sequence(config, occupancies);
    }
}

/// Occupancy is drawn from the low 16 RNG bits; truncation is intentional.
#[allow(clippy::cast_possible_truncation)]
#[test]
fn in_range_occupancy_never_resynchronizes_and_stays_bounded() {
    let config = config(100, 10, 500, 50, 16);
    let mut rng = XorShift64(0xB0B1_2026_0000_0005);
    let mut controller = DriftController::new(config).expect("property config must validate");
    let mut previous_ppm = 0_i32;
    let mut resyncs = 0_u32;
    let range = config.high_resync_blocks - config.low_resync_blocks - 1;

    for _ in 0..100_000 {
        let occupancy = config.low_resync_blocks + 1 + (rng.next_u32() % u32::from(range)) as u16;
        match controller.update(occupancy) {
            DriftDecision::HardResync { .. } => resyncs += 1,
            DriftDecision::Adjust { ppm } => {
                assert!(ppm.abs() <= config.max_adjustment_ppm);
                assert!((ppm - previous_ppm).abs() <= config.max_step_ppm);
                previous_ppm = ppm;
            }
        }
    }

    assert_eq!(resyncs, 0, "in-range occupancy must never trigger a resync");
}
