#![no_main]

//! Fuzz the integerized drift scenario closed loop.
//!
//! Arbitrary bounded scenario configurations drive `simulate_drift` end to
//! end: a synthetic occupancy plant, the bounded per-stream drift controller,
//! rounding, and resynchronization bookkeeping. An accepted configuration
//! must keep every occupancy observation within the bounded PCM queue, keep
//! the final requested ratio within the configured cap, keep the
//! resynchronization bookkeeping consistent (each hard resync is exactly one
//! low- or high-side reset), and stay deterministic; arbitrary raw scenario
//! fields must also run through validation without panicking, and an accepted
//! raw configuration must satisfy the same invariants.

use libfuzzer_sys::fuzz_target;
use sonimesh_audio::{
    DriftConfig, MAX_DRIFT_ADJUSTMENT_PPM, MAX_DRIFT_INTEGRAL_BLOCK_TICKS, MAX_PCM_QUEUE_BLOCKS,
};
use sonimesh_network_simulator::{DriftScenarioConfig, DriftSimulationReport, simulate_drift};

/// The model's synthetic sender-rate bound (private in the scenario module).
const MAX_SOURCE_RATE_PPM: i32 = 10_000;
/// Scenario tick bound kept small so every fuzz run is bounded.
const MAX_FUZZ_TICKS: u32 = 10_000;

fuzz_target!(|data: &[u8]| {
    // Derived fields are chosen so `DriftScenarioConfig` validation always
    // accepts: the target sits strictly between the emergency thresholds, the
    // high threshold stays within the bounded PCM queue, and every gain and
    // cap is positive and ordered.
    let ticks = 1 + u32::from(u16::from_le_bytes([
        data.first().copied().unwrap_or(0),
        data.get(1).copied().unwrap_or(0),
    ])) % MAX_FUZZ_TICKS;
    let target_blocks = 1 + u16::from_le_bytes([
        data.get(2).copied().unwrap_or(0),
        data.get(3).copied().unwrap_or(0),
    ]) % (MAX_PCM_QUEUE_BLOCKS - 1) as u16;
    let low_resync_blocks = u16::from(data.get(4).copied().unwrap_or(0)) % target_blocks;
    let high_resync_blocks = target_blocks
        + 1
        + u16::from(data.get(5).copied().unwrap_or(0))
            % (MAX_PCM_QUEUE_BLOCKS as u16 - target_blocks);
    let proportional_ppm_per_block = 1 + i32::from(u16::from_le_bytes([
        data.get(6).copied().unwrap_or(0),
        data.get(7).copied().unwrap_or(0),
    ])) % 500;
    let integral_ppm_per_block_tick = 1 + i32::from(u16::from_le_bytes([
        data.get(8).copied().unwrap_or(0),
        data.get(9).copied().unwrap_or(0),
    ])) % 100;
    let max_adjustment_ppm = 1 + i32::from(u16::from_le_bytes([
        data.get(10).copied().unwrap_or(0),
        data.get(11).copied().unwrap_or(0),
    ])) % MAX_DRIFT_ADJUSTMENT_PPM;
    let max_step_ppm = 1 + i32::from(u16::from_le_bytes([
        data.get(12).copied().unwrap_or(0),
        data.get(13).copied().unwrap_or(0),
    ])) % max_adjustment_ppm;
    let max_integral_block_ticks = 1 + i32::from(u16::from_le_bytes([
        data.get(14).copied().unwrap_or(0),
        data.get(15).copied().unwrap_or(0),
    ])) % MAX_DRIFT_INTEGRAL_BLOCK_TICKS;
    let initial_occupancy_blocks =
        u16::from(data.get(16).copied().unwrap_or(0)) % (MAX_PCM_QUEUE_BLOCKS as u16 + 1);
    // Map two bytes to a signed offset that always stays inside the model's
    // synthetic sender-rate bound.
    let source_rate_residue = u32::from(u16::from_le_bytes([
        data.get(17).copied().unwrap_or(0),
        data.get(18).copied().unwrap_or(0),
    ])) % (2 * MAX_SOURCE_RATE_PPM as u32 + 1);
    let source_rate_ppm = i32::try_from(source_rate_residue).unwrap_or(0) - MAX_SOURCE_RATE_PPM;

    let config = DriftScenarioConfig {
        ticks,
        initial_occupancy_blocks,
        source_rate_ppm,
        controller: DriftConfig {
            target_blocks,
            low_resync_blocks,
            high_resync_blocks,
            proportional_ppm_per_block,
            integral_ppm_per_block_tick,
            max_adjustment_ppm,
            max_step_ppm,
            max_integral_block_ticks,
        },
    };
    let first = simulate_drift(config).expect("the derived configuration must be valid");
    let second = simulate_drift(config).expect("the derived configuration must be valid");
    assert_eq!(first, second, "the closed loop must be deterministic");
    assert_scenario_invariants(&first, &config);

    // Arbitrary raw scenario fields must not panic validation; accepted ones
    // must satisfy the same closed-loop invariants.
    if data.len() >= 32 {
        let raw = DriftScenarioConfig {
            ticks: 1,
            initial_occupancy_blocks: u16::from_le_bytes([data[16], data[17]]),
            source_rate_ppm: i32::from_le_bytes([data[18], data[19], data[20], data[21]]),
            controller: DriftConfig {
                target_blocks: u16::from_le_bytes([data[22], data[23]]),
                low_resync_blocks: u16::from_le_bytes([data[24], data[25]]),
                high_resync_blocks: u16::from_le_bytes([data[26], data[27]]),
                proportional_ppm_per_block: i32::from(data[28]),
                integral_ppm_per_block_tick: i32::from(data[29]),
                max_adjustment_ppm: i32::from(data[30]),
                max_step_ppm: i32::from(data[31]),
                max_integral_block_ticks: 1,
            },
        };
        if let Ok(report) = simulate_drift(raw) {
            assert_scenario_invariants(&report, &raw);
        }
    }
});

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
