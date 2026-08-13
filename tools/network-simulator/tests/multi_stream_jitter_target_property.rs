//! Deterministic property coverage for the multi-stream adaptive-target
//! simulation.
//!
//! The shared-link scenario must complete and stay bounded for any valid
//! configuration: every stream plays or conceals exactly its configured frame
//! count, each stream's adaptive target remains inside its `[min, max]` water
//! levels, step counters never exceed the observation count, and a stream-local
//! jitter resynchronization never corrupts another stream's accounting. This
//! test drives seeded random network profiles and independent per-stream modes
//! over a long run and asserts those invariants on every report, mirroring the
//! deterministic property pattern used for the single-stream scheduler.

use sonimesh_audio::{JitterMode, JitterTargetConfig};
use sonimesh_network_simulator::{
    MultiStreamJitterTargetScenarioConfig, MultiStreamJitterTargetSimulationReport, NetworkProfile,
    simulate_multi_stream_jitter_target,
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

    /// Returns a value in `low..high_exclusive` for `low < high_exclusive`.
    fn next_in(&mut self, low: u32, high_exclusive: u32) -> u32 {
        low + self.next_u32() % (high_exclusive - low)
    }
}

/// Jitter-buffer capacity shared by the scenario and its target presets.
const JITTER_BUFFER_CAPACITY: usize = 64;
/// Number of seeded configurations per property run.
const STEPS: usize = 64;
/// Streams in each scenario run.
const STREAMS: usize = 3;

/// Builds a validated network profile within the scenario impairment range.
fn random_profile(random: &mut XorShift64) -> NetworkProfile {
    let mut network = NetworkProfile::stable(u64::from(random.next_u32()));
    network.loss_percent = u8::try_from(random.next_in(0, 16)).unwrap();
    network.duplicate_percent = u8::try_from(random.next_in(0, 11)).unwrap();
    network.reorder_percent = u8::try_from(random.next_in(0, 9)).unwrap();
    network.base_delay_ms = u64::from(random.next_in(1, 61));
    network.jitter_ms = u64::from(random.next_in(0, 121));
    network.reorder_delay_ms = u64::from(random.next_in(0, 41));
    if random.next_u32() % 3 == 0 {
        let interval = random.next_in(20, 61);
        network.burst_interval_packets = interval;
        network.burst_length_packets =
            u16::try_from(random.next_in(1, (interval - 1).min(6) + 1)).unwrap();
    }
    network
}

fn random_mode(random: &mut XorShift64) -> JitterMode {
    match random.next_usize(3) {
        0 => JitterMode::LowLatency,
        1 => JitterMode::Balanced,
        _ => JitterMode::Stable,
    }
}

/// Builds one valid multi-stream configuration from the seeded random stream.
fn random_config(random: &mut XorShift64) -> MultiStreamJitterTargetScenarioConfig<STREAMS> {
    let frame_duration_ms: u64 = if random.next_u32() % 2 == 0 { 10 } else { 20 };
    let frame_duration = u32::try_from(frame_duration_ms).unwrap();
    MultiStreamJitterTargetScenarioConfig {
        frames: random.next_in(30, 201),
        frame_duration_ms,
        target_buffer_ms: u64::from(random.next_in(10, 301)),
        media_datagram_bytes: 256,
        network: random_profile(random),
        targets: std::array::from_fn(|_| {
            JitterTargetConfig::preset(random_mode(random), frame_duration, JITTER_BUFFER_CAPACITY)
        }),
    }
}

/// Asserts every multi-stream invariant on one completed report.
fn assert_invariants(
    config: &MultiStreamJitterTargetScenarioConfig<STREAMS>,
    report: &MultiStreamJitterTargetSimulationReport<STREAMS>,
) {
    for (index, stream) in report.streams.iter().enumerate() {
        let consumed = stream.playout.playout_frames + stream.playout.plc_frames;
        assert_eq!(
            consumed, config.frames,
            "stream {index} must play or conceal every configured frame"
        );
        assert_eq!(
            stream.observations, config.frames,
            "stream {index} must observe once per configured frame"
        );
        assert!(
            (stream.min_frames..=stream.max_frames).contains(&stream.final_target_frames),
            "stream {index} target must stay inside the bounded range"
        );
        assert!(
            (stream.min_frames..=stream.max_frames).contains(&stream.base_frames),
            "stream {index} mode base must stay inside the bounded range"
        );
        assert!(
            stream.raised_steps + stream.lowered_steps <= stream.observations,
            "stream {index} steps must not exceed observations"
        );
        assert!(
            stream.final_buffered_frames <= JITTER_BUFFER_CAPACITY,
            "stream {index} buffered frames must not exceed the jitter capacity"
        );
    }
}

#[test]
fn every_configuration_completes_with_bounded_per_stream_targets() {
    for seed in [0x5EED_2026_0813_0002, 0xABCD_EF01_2345_6790] {
        let mut random = XorShift64(seed);
        for _ in 0..STEPS {
            let config = random_config(&mut random);
            let report = simulate_multi_stream_jitter_target::<STREAMS>(config)
                .unwrap_or_else(|error| panic!("configuration {config:?} failed: {error}"));
            assert_invariants(&config, &report);
        }
    }
}

#[test]
fn identical_configs_produce_identical_reports() {
    let mut random = XorShift64(0xDEAD_BEEF_2026_0813);
    for _ in 0..16 {
        let config = random_config(&mut random);
        let first = simulate_multi_stream_jitter_target::<STREAMS>(config).unwrap();
        let second = simulate_multi_stream_jitter_target::<STREAMS>(config).unwrap();
        assert_eq!(first, second);
    }
}
