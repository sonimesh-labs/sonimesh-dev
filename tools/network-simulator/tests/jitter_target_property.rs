//! Deterministic property coverage for the single-stream adaptive-jitter-target
//! simulation.
//!
//! The scenario must complete and stay bounded for any valid configuration:
//! every frame is played or concealed exactly once, the controller observes
//! once per frame, the adaptive target stays inside its `[min, max]` water
//! levels, step counters never exceed the observation count, the buffered
//! frame count never exceeds the jitter capacity, and exactly one packet is
//! submitted per frame. This test drives seeded random network profiles and
//! controller modes across the full valid initial-target range and asserts
//! those invariants on every report, mirroring the deterministic property
//! pattern used for the multi-stream adaptive-target scenario.

use sonimesh_audio::{JitterMode, JitterTargetConfig};
use sonimesh_network_simulator::{
    JitterTargetScenarioConfig, JitterTargetSimulationReport, NetworkProfile,
    simulate_jitter_target,
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

/// Jitter-buffer capacity shared by the scenario and its target preset.
const JITTER_BUFFER_CAPACITY: usize = 64;
/// Number of seeded configurations per property run.
const STEPS: usize = 128;

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

/// Builds one valid configuration from the seeded random stream.
fn random_config(random: &mut XorShift64) -> JitterTargetScenarioConfig {
    let frame_duration_ms: u64 = if random.next_u32() % 2 == 0 { 10 } else { 20 };
    let frame_duration = u32::try_from(frame_duration_ms).unwrap();
    JitterTargetScenarioConfig {
        frames: random.next_in(30, 201),
        frame_duration_ms,
        // The scenario pre-arms playout at the negotiated start sequence, so
        // any valid initial target is accepted: a playout before the first
        // frame arrives conceals instead of rejecting.
        target_buffer_ms: u64::from(random.next_in(0, 1_001)),
        media_datagram_bytes: 256,
        network: random_profile(random),
        target: JitterTargetConfig::preset(
            random_mode(random),
            frame_duration,
            JITTER_BUFFER_CAPACITY,
        ),
    }
}

/// Asserts every single-stream invariant on one completed report.
fn assert_invariants(config: &JitterTargetScenarioConfig, report: &JitterTargetSimulationReport) {
    let consumed = report.playout.playout_frames + report.playout.plc_frames;
    assert_eq!(
        consumed, config.frames,
        "every configured frame must be played or concealed"
    );
    assert_eq!(
        report.observations, config.frames,
        "the controller must observe once per configured frame"
    );
    assert_eq!(
        report.network.submitted_packets,
        u64::from(config.frames),
        "exactly one packet must be submitted per frame"
    );
    assert!(
        (report.min_frames..=report.max_frames).contains(&report.final_target_frames),
        "target must stay inside the bounded range"
    );
    assert!(
        (report.min_frames..=report.max_frames).contains(&report.base_frames),
        "mode base must stay inside the bounded range"
    );
    assert!(
        report.raised_steps + report.lowered_steps <= report.observations,
        "steps must not exceed observations"
    );
    assert!(
        report.final_buffered_frames <= JITTER_BUFFER_CAPACITY,
        "buffered frames must not exceed the jitter capacity"
    );
}

#[test]
fn every_configuration_completes_with_bounded_target() {
    for seed in [0x5EED_2026_0814_0001, 0xABCD_EF01_2345_6791] {
        let mut random = XorShift64(seed);
        for _ in 0..STEPS {
            let config = random_config(&mut random);
            let report = simulate_jitter_target(config)
                .unwrap_or_else(|error| panic!("configuration {config:?} failed: {error}"));
            assert_invariants(&config, &report);
        }
    }
}

#[test]
fn identical_configs_produce_identical_reports() {
    let mut random = XorShift64(0xDEAD_BEEF_2026_0814);
    for _ in 0..16 {
        let config = random_config(&mut random);
        let first = simulate_jitter_target(config).unwrap();
        let second = simulate_jitter_target(config).unwrap();
        assert_eq!(first, second);
    }
}
