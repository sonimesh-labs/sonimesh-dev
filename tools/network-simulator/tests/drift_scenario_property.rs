//! Deterministic property coverage for the integerized drift scenario.
//!
//! The closed-loop occupancy model must complete and stay bounded for any
//! valid configuration: every occupancy observation stays within the bounded
//! PCM queue, the final requested ratio stays within the configured cap, the
//! resynchronization bookkeeping stays consistent (each hard resync is exactly
//! one low- or high-side reset), the same configuration always produces the
//! same report, and a clean no-drift plant (nominal source rate and target
//! occupancy) never requests a resynchronization. This test drives seeded
//! random tick counts, source rates, initial occupancies, and controller
//! parameters over a long run and asserts those invariants on every report,
//! mirroring the deterministic property pattern used for the sibling scenarios
//! and the `drift_scenario` fuzz target.

use sonimesh_audio::{
    DriftConfig, MAX_DRIFT_ADJUSTMENT_PPM, MAX_DRIFT_INTEGRAL_BLOCK_TICKS, MAX_PCM_QUEUE_BLOCKS,
};
use sonimesh_network_simulator::{DriftScenarioConfig, DriftSimulationReport, simulate_drift};

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

    /// Returns a value in `low..high_exclusive` for `low < high_exclusive`.
    fn next_in(&mut self, low: u32, high_exclusive: u32) -> u32 {
        low + self.next_u32() % (high_exclusive - low)
    }
}

/// Number of seeded configurations per property run.
const STEPS: usize = 256;
/// Scenario tick bound shared with the `drift_scenario` fuzz target.
const MAX_PROPERTY_TICKS: u32 = 10_000;
/// The model's synthetic sender-rate bound (private in the scenario module).
const MAX_SOURCE_RATE_PPM: i32 = 10_000;

/// Builds a validated drift scenario configuration within the model bounds.
///
/// The ranges mirror the `drift_scenario` fuzz target so the seeded property
/// run locally covers the same configuration space the LibFuzzer smoke
/// exercises on CI.
fn random_config(random: &mut XorShift64) -> DriftScenarioConfig {
    // The target sits strictly between the low and high emergency thresholds,
    // and the high threshold cannot exceed the bounded PCM queue.
    let target_blocks =
        u16::try_from(random.next_in(1, u32::try_from(MAX_PCM_QUEUE_BLOCKS).unwrap())).unwrap();
    let max_blocks = u32::try_from(MAX_PCM_QUEUE_BLOCKS).unwrap();
    let max_adjustment_ppm =
        i32::try_from(random.next_in(1, u32::try_from(MAX_DRIFT_ADJUSTMENT_PPM).unwrap() + 1))
            .unwrap();
    let source_rate_residue = random
        .next_u32()
        .checked_rem(2 * u32::try_from(MAX_SOURCE_RATE_PPM).unwrap() + 1)
        .unwrap_or(0);
    DriftScenarioConfig {
        ticks: random.next_in(1, MAX_PROPERTY_TICKS + 1),
        initial_occupancy_blocks: u16::try_from(random.next_in(0, max_blocks + 1)).unwrap(),
        source_rate_ppm: i32::try_from(source_rate_residue).unwrap_or(0) - MAX_SOURCE_RATE_PPM,
        controller: DriftConfig {
            target_blocks,
            low_resync_blocks: u16::try_from(random.next_in(0, u32::from(target_blocks))).unwrap(),
            high_resync_blocks: u16::try_from(
                random.next_in(u32::from(target_blocks) + 1, max_blocks + 1),
            )
            .unwrap(),
            proportional_ppm_per_block: i32::try_from(random.next_in(1, 501)).unwrap(),
            integral_ppm_per_block_tick: i32::try_from(random.next_in(1, 101)).unwrap(),
            max_adjustment_ppm,
            max_step_ppm: i32::try_from(
                random.next_in(1, u32::try_from(max_adjustment_ppm).unwrap() + 1),
            )
            .unwrap(),
            max_integral_block_ticks: i32::try_from(random.next_in(
                1,
                u32::try_from(MAX_DRIFT_INTEGRAL_BLOCK_TICKS).unwrap() + 1,
            ))
            .unwrap(),
        },
    }
}

/// Asserts the closed-loop invariants that hold for every accepted report.
fn assert_scenario_invariants(report: &DriftSimulationReport, config: &DriftScenarioConfig) {
    let max_blocks = u16::try_from(MAX_PCM_QUEUE_BLOCKS).unwrap();
    assert!(
        report.minimum_occupancy_blocks <= config.initial_occupancy_blocks,
        "minimum occupancy can never exceed the initial snapshot"
    );
    assert!(
        config.initial_occupancy_blocks <= report.maximum_occupancy_blocks,
        "maximum occupancy can never drop below the initial snapshot"
    );
    assert!(
        report.minimum_occupancy_blocks <= report.maximum_occupancy_blocks,
        "the occupancy range must stay ordered"
    );
    assert!(
        report.minimum_occupancy_blocks <= max_blocks,
        "minimum occupancy must stay within the bounded PCM queue"
    );
    assert!(
        report.maximum_occupancy_blocks <= max_blocks,
        "maximum occupancy must stay within the bounded PCM queue"
    );
    assert!(
        report.final_occupancy_blocks <= max_blocks,
        "final occupancy must stay within the bounded PCM queue"
    );
    assert_eq!(
        report.hard_resynchronizations,
        report.low_resynchronizations + report.high_resynchronizations,
        "each hard resync must be exactly one low- or high-side reset"
    );
    assert!(
        report.final_adjustment_ppm.abs() <= config.controller.max_adjustment_ppm,
        "the requested ratio must stay within the configured cap"
    );

    // A nominal-rate plant starting exactly at target cannot leave target and
    // must never request a resynchronization.
    if config.source_rate_ppm == 0
        && config.initial_occupancy_blocks == config.controller.target_blocks
    {
        assert_eq!(
            report.hard_resynchronizations, 0,
            "a clean no-drift plant must not request a resynchronization"
        );
        assert_eq!(
            report.final_adjustment_ppm, 0,
            "a clean no-drift plant must stay at the nominal ratio"
        );
        assert_eq!(
            report.final_occupancy_blocks, config.controller.target_blocks,
            "a clean no-drift plant must stay at the target occupancy"
        );
    }
}

#[test]
fn every_valid_configuration_stays_bounded_and_deterministic() {
    let mut random = XorShift64(0x5A17_2026);
    for _ in 0..STEPS {
        let config = random_config(&mut random);
        let first = simulate_drift(config).expect("the seeded configuration must be valid");
        let second = simulate_drift(config).expect("the seeded configuration must be valid");
        assert_eq!(first, second, "the closed loop must be deterministic");
        assert_scenario_invariants(&first, &config);
    }
}

#[test]
fn nominal_plant_stays_at_target_without_resynchronization() {
    let config = DriftScenarioConfig {
        ticks: 20_000,
        initial_occupancy_blocks: 4,
        source_rate_ppm: 0,
        controller: DriftConfig {
            target_blocks: 4,
            low_resync_blocks: 0,
            high_resync_blocks: 8,
            proportional_ppm_per_block: 25,
            integral_ppm_per_block_tick: 1,
            max_adjustment_ppm: 500,
            max_step_ppm: 25,
            max_integral_block_ticks: 256,
        },
    };

    let report = simulate_drift(config).expect("the nominal configuration must be valid");
    assert_eq!(report.hard_resynchronizations, 0);
    assert_eq!(report.low_resynchronizations, 0);
    assert_eq!(report.high_resynchronizations, 0);
    assert_eq!(report.final_adjustment_ppm, 0);
    assert_eq!(report.final_occupancy_blocks, 4);
    assert_eq!(report.minimum_occupancy_blocks, 4);
    assert_eq!(report.maximum_occupancy_blocks, 4);
}
