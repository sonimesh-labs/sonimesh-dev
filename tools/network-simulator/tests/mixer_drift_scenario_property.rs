//! Deterministic property coverage for the multi-stream mixer-drift scenario.
//!
//! The per-stream closed-loop occupancy model must complete and stay bounded
//! for every valid configuration, and the multi-stream model must preserve the
//! single-stream semantics exactly: every occupancy observation stays within
//! the bounded PCM queue, the final requested ratio stays within the
//! configured cap, the resynchronization bookkeeping stays consistent (each
//! hard resync is exactly one low- or high-side reset), every stream's report
//! equals the single-stream `simulate_drift` result for that stream's own
//! configuration, a clean no-drift stream never requests a resynchronization
//! even while uncorrectable sibling streams do, and identical configurations
//! produce identical reports. This test drives seeded random tick counts,
//! per-stream source rates and initial occupancies, and shared controller
//! parameters over a long run and asserts those invariants on every report,
//! mirroring the deterministic property pattern used for the sibling scenarios
//! and the `mixer_drift_scenario` fuzz target.

use sonimesh_audio::{
    DriftConfig, MAX_DRIFT_ADJUSTMENT_PPM, MAX_DRIFT_INTEGRAL_BLOCK_TICKS, MAX_PCM_QUEUE_BLOCKS,
};
use sonimesh_network_simulator::{
    MAX_MIXER_DRIFT_STREAMS, MIN_MIXER_DRIFT_STREAMS, MixerDriftScenarioConfig,
    MixerDriftStreamConfig, simulate_drift, simulate_mixer_drift,
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

    /// Returns a value in `low..high_exclusive` for `low < high_exclusive`.
    fn next_in(&mut self, low: u32, high_exclusive: u32) -> u32 {
        low + self.next_u32() % (high_exclusive - low)
    }
}

/// Number of seeded configurations per property run.
const STEPS: usize = 256;
/// Scenario tick bound shared with the `mixer_drift_scenario` fuzz target.
const MAX_PROPERTY_TICKS: u32 = 10_000;
/// The model's synthetic sender-rate bound (private in the scenario module).
const MAX_SOURCE_RATE_PPM: i32 = 10_000;

/// Builds a validated shared controller configuration within the model bounds.
///
/// The ranges mirror the `mixer_drift_scenario` fuzz target so the seeded
/// property run locally covers the same configuration space the LibFuzzer
/// smoke exercises on CI.
fn random_controller(random: &mut XorShift64) -> DriftConfig {
    let max_blocks = u32::try_from(MAX_PCM_QUEUE_BLOCKS).unwrap();
    let target_blocks = u16::try_from(random.next_in(1, max_blocks)).unwrap();
    let max_adjustment_ppm =
        i32::try_from(random.next_in(1, u32::try_from(MAX_DRIFT_ADJUSTMENT_PPM).unwrap() + 1))
            .unwrap();
    DriftConfig {
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
    }
}

/// Builds one validated per-stream source configuration within the model bounds.
fn random_stream(random: &mut XorShift64) -> MixerDriftStreamConfig {
    let source_rate_residue = random
        .next_u32()
        .checked_rem(2 * u32::try_from(MAX_SOURCE_RATE_PPM).unwrap() + 1)
        .unwrap_or(0);
    MixerDriftStreamConfig {
        initial_occupancy_blocks: u16::try_from(
            random.next_in(0, u32::try_from(MAX_PCM_QUEUE_BLOCKS).unwrap() + 1),
        )
        .unwrap(),
        source_rate_ppm: i32::try_from(source_rate_residue).unwrap_or(0) - MAX_SOURCE_RATE_PPM,
    }
}

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

    // A nominal-rate stream starting exactly at target cannot leave target and
    // must never request a resynchronization.
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

#[test]
fn seeded_multi_stream_runs_preserve_single_stream_semantics_and_isolation() {
    let mut random = XorShift64(0x9E37_79B9_7F4A_7C15);
    for step in 0..STEPS {
        let streams_count = MIN_MIXER_DRIFT_STREAMS
            + usize::try_from(
                random.next_in(0, u32::try_from(MAX_MIXER_DRIFT_STREAMS).unwrap() - 1),
            )
            .unwrap();
        let controller = random_controller(&mut random);
        let ticks = random.next_in(1, MAX_PROPERTY_TICKS + 1);
        let stream_configs: Vec<MixerDriftStreamConfig> = (0..streams_count)
            .map(|_| random_stream(&mut random))
            .collect();
        let mut streams = [MixerDriftStreamConfig {
            initial_occupancy_blocks: 0,
            source_rate_ppm: 0,
        }; MAX_MIXER_DRIFT_STREAMS];
        streams[..streams_count].copy_from_slice(&stream_configs);
        let config: MixerDriftScenarioConfig<MAX_MIXER_DRIFT_STREAMS> = MixerDriftScenarioConfig {
            ticks,
            controller,
            streams,
        };
        let report = simulate_mixer_drift(config)
            .unwrap_or_else(|error| panic!("step {step}: derived config must be valid: {error}"));
        let again = simulate_mixer_drift(config).unwrap();
        assert_eq!(report, again, "the multi-stream loop must be deterministic");

        for (index, stream) in stream_configs.iter().enumerate() {
            assert_stream_invariants(&report.streams[index], *stream, &controller);

            let single = simulate_drift(sonimesh_network_simulator::DriftScenarioConfig {
                ticks,
                initial_occupancy_blocks: stream.initial_occupancy_blocks,
                source_rate_ppm: stream.source_rate_ppm,
                controller,
            })
            .unwrap();
            assert_eq!(
                report.streams[index], single,
                "step {step}: stream {index} must match the single-stream loop for its own configuration"
            );
        }
    }
}

#[test]
fn seeded_multi_stream_runs_keep_clean_streams_isolated_from_uncorrectable_siblings() {
    let mut random = XorShift64(0xD1B5_4A32_D192_ED03);
    for step in 0..STEPS {
        let controller = random_controller(&mut random);
        let ticks = random.next_in(1, MAX_PROPERTY_TICKS + 1);
        // A clean stream sits exactly at target; its siblings get arbitrary
        // valid source rates so at least the model's uncorrectable range is
        // exercised on some seeds.
        let clean = MixerDriftStreamConfig {
            initial_occupancy_blocks: controller.target_blocks,
            source_rate_ppm: 0,
        };
        let streams = [
            clean,
            random_stream(&mut random),
            random_stream(&mut random),
        ];
        let config = MixerDriftScenarioConfig {
            ticks,
            controller,
            streams,
        };
        let report = simulate_mixer_drift(config)
            .unwrap_or_else(|error| panic!("step {step}: derived config must be valid: {error}"));
        assert_eq!(
            report.streams[0].hard_resynchronizations, 0,
            "step {step}: a clean stream must not resynchronize while siblings do"
        );
        assert_eq!(report.streams[0].final_adjustment_ppm, 0);
        assert_eq!(
            report.streams[0].final_occupancy_blocks, controller.target_blocks,
            "step {step}: a clean stream must stay at the target occupancy"
        );
        assert_stream_invariants(&report.streams[1], streams[1], &controller);
        assert_stream_invariants(&report.streams[2], streams[2], &controller);
    }
}
