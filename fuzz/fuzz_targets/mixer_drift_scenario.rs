#![no_main]

//! Fuzz the multi-stream mixer-drift scenario closed loop.
//!
//! Arbitrary bounded scenario configurations drive `simulate_mixer_drift` end
//! to end at the Phase 2 four-stream upper bound: four independent
//! integerized occupancy plants, each with its own bounded drift controller,
//! rounding, and resynchronization bookkeeping. An accepted configuration must
//! keep every stream's occupancy observation within the bounded PCM queue,
//! keep every final requested ratio within the configured cap, keep the
//! resynchronization bookkeeping consistent (each hard resync is exactly one
//! low- or high-side reset), keep every stream's report equal to the
//! single-stream `simulate_drift` result for that stream's own configuration,
//! and stay deterministic; arbitrary raw scenario fields must also run through
//! validation without panicking, and an accepted raw configuration must
//! satisfy the same invariants.

use libfuzzer_sys::fuzz_target;
use sonimesh_audio::{
    DriftConfig, MAX_DRIFT_ADJUSTMENT_PPM, MAX_DRIFT_INTEGRAL_BLOCK_TICKS, MAX_PCM_QUEUE_BLOCKS,
};
use sonimesh_network_simulator::{
    MAX_MIXER_DRIFT_STREAMS, MixerDriftScenarioConfig, MixerDriftStreamConfig, simulate_drift,
    simulate_mixer_drift,
};

/// The model's synthetic sender-rate bound (private in the scenario module).
const MAX_SOURCE_RATE_PPM: i32 = 10_000;
/// Scenario tick bound kept small so every fuzz run is bounded.
const MAX_FUZZ_TICKS: u32 = 10_000;
/// Stream count exercised by this target: the Phase 2 upper bound.
const STREAMS: usize = MAX_MIXER_DRIFT_STREAMS;

/// Asserts the closed-loop invariants that hold for every accepted report.
fn assert_stream_invariants(
    report: &sonimesh_network_simulator::DriftSimulationReport,
    stream: MixerDriftStreamConfig,
    controller: &DriftConfig,
) {
    let max_blocks = u16::try_from(MAX_PCM_QUEUE_BLOCKS).unwrap();
    assert!(
        report.minimum_occupancy_blocks <= stream.initial_occupancy_blocks,
        "minimum occupancy can never exceed the initial snapshot"
    );
    assert!(
        stream.initial_occupancy_blocks <= report.maximum_occupancy_blocks,
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
        report.final_adjustment_ppm.abs() <= controller.max_adjustment_ppm,
        "the requested ratio must stay within the configured cap"
    );

    if stream.source_rate_ppm == 0 && stream.initial_occupancy_blocks == controller.target_blocks {
        assert_eq!(
            report.hard_resynchronizations, 0,
            "a clean no-drift stream must not request a resynchronization"
        );
        assert_eq!(
            report.final_adjustment_ppm, 0,
            "a clean no-drift stream must stay at the nominal ratio"
        );
        assert_eq!(
            report.final_occupancy_blocks, controller.target_blocks,
            "a clean no-drift stream must stay at the target occupancy"
        );
    }
}

fn derived_stream(data: &[u8], index: usize) -> MixerDriftStreamConfig {
    let initial = data.get(16 + index * 2).copied().unwrap_or(0);
    let rate_residue = u32::from(u16::from_le_bytes([
        data.get(17 + index * 2).copied().unwrap_or(0),
        data.get(18 + index * 2).copied().unwrap_or(0),
    ])) % (2 * MAX_SOURCE_RATE_PPM as u32 + 1);
    MixerDriftStreamConfig {
        initial_occupancy_blocks: u16::from(initial) % (MAX_PCM_QUEUE_BLOCKS as u16 + 1),
        source_rate_ppm: i32::try_from(rate_residue).unwrap_or(0) - MAX_SOURCE_RATE_PPM,
    }
}

fuzz_target!(|data: &[u8]| {
    // Derived fields are chosen so `MixerDriftScenarioConfig` validation always
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

    let streams = [
        derived_stream(data, 0),
        derived_stream(data, 1),
        derived_stream(data, 2),
        derived_stream(data, 3),
    ];
    let controller = DriftConfig {
        target_blocks,
        low_resync_blocks,
        high_resync_blocks,
        proportional_ppm_per_block,
        integral_ppm_per_block_tick,
        max_adjustment_ppm,
        max_step_ppm,
        max_integral_block_ticks,
    };
    let config = MixerDriftScenarioConfig {
        ticks,
        controller,
        streams,
    };
    let first = simulate_mixer_drift(config).expect("the derived configuration must be valid");
    let second = simulate_mixer_drift(config).expect("the derived configuration must be valid");
    assert_eq!(first, second, "the multi-stream loop must be deterministic");
    for (index, stream) in streams.iter().enumerate() {
        assert_stream_invariants(&first.streams[index], *stream, &controller);
        let single = simulate_drift(sonimesh_network_simulator::DriftScenarioConfig {
            ticks,
            initial_occupancy_blocks: stream.initial_occupancy_blocks,
            source_rate_ppm: stream.source_rate_ppm,
            controller,
        })
        .expect("the derived single-stream configuration must be valid");
        assert_eq!(
            first.streams[index], single,
            "stream {index} must match the single-stream loop for its own configuration"
        );
    }

    // Arbitrary raw scenario fields must not panic validation; accepted ones
    // must satisfy the same closed-loop invariants.
    if data.len() >= 24 + STREAMS * 4 {
        let raw_streams = [
            MixerDriftStreamConfig {
                initial_occupancy_blocks: u16::from_le_bytes([data[24], data[25]]),
                source_rate_ppm: i32::from_le_bytes([data[26], data[27], data[28], data[29]]),
            },
            MixerDriftStreamConfig {
                initial_occupancy_blocks: u16::from_le_bytes([data[30], data[31]]),
                source_rate_ppm: i32::from_le_bytes([data[32], data[33], data[34], data[35]]),
            },
            MixerDriftStreamConfig {
                initial_occupancy_blocks: u16::from_le_bytes([data[36], data[37]]),
                source_rate_ppm: i32::from_le_bytes([data[38], data[39], data[40], data[41]]),
            },
            MixerDriftStreamConfig {
                initial_occupancy_blocks: u16::from_le_bytes([data[42], data[43]]),
                source_rate_ppm: i32::from_le_bytes([data[44], data[45], data[46], data[47]]),
            },
        ];
        let raw = MixerDriftScenarioConfig {
            ticks: 1,
            controller: DriftConfig {
                target_blocks: u16::from_le_bytes([data[0], data[1]]),
                low_resync_blocks: u16::from_le_bytes([data[2], data[3]]),
                high_resync_blocks: u16::from_le_bytes([data[4], data[5]]),
                proportional_ppm_per_block: i32::from(data[6]),
                integral_ppm_per_block_tick: i32::from(data[7]),
                max_adjustment_ppm: i32::from(data[8]),
                max_step_ppm: i32::from(data[9]),
                max_integral_block_ticks: 1,
            },
            streams: raw_streams,
        };
        if let Ok(report) = simulate_mixer_drift(raw) {
            for (index, stream) in raw_streams.iter().enumerate() {
                assert_stream_invariants(&report.streams[index], *stream, &raw.controller);
            }
        }
    }
});
