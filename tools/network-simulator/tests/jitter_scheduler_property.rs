//! Deterministic property coverage for the target-gated playout scheduler.
//!
//! The scheduler scenario must always complete for any valid configuration:
//! every configured frame is played or concealed exactly once, each scheduler
//! tick is either one gated wait or one consumed frame, the adaptive target
//! stays inside its bounded `[min, max]` range, and completion never exceeds
//! the explicit hard deadline. This test drives seeded random network
//! profiles, controller modes, and gate deadlines over a long run and asserts
//! those invariants on every report.

use sonimesh_audio::{JitterMode, JitterTargetConfig};
use sonimesh_network_simulator::{
    JitterSchedulerScenarioConfig, JitterSchedulerSimulationReport, NetworkProfile,
    simulate_jitter_scheduler,
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
const STEPS: usize = 128;

/// Builds a validated network profile within the scheduler impairment range.
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

/// Builds one valid scheduler configuration from the seeded random stream.
fn random_config(random: &mut XorShift64) -> JitterSchedulerScenarioConfig {
    let frame_duration_ms: u64 = if random.next_u32() % 2 == 0 { 10 } else { 20 };
    let mode = match random.next_usize(3) {
        0 => JitterMode::LowLatency,
        1 => JitterMode::Balanced,
        _ => JitterMode::Stable,
    };
    JitterSchedulerScenarioConfig {
        frames: random.next_in(30, 201),
        frame_duration_ms,
        initial_playout_ms: u64::from(random.next_in(10, 301)),
        media_datagram_bytes: 256,
        network: random_profile(random),
        target: JitterTargetConfig::preset(
            mode,
            u32::try_from(frame_duration_ms).unwrap(),
            JITTER_BUFFER_CAPACITY,
        ),
        max_gate_ticks: random.next_in(1, 17),
    }
}

/// Asserts every scheduler invariant on one completed report.
fn assert_invariants(
    config: JitterSchedulerScenarioConfig,
    report: &JitterSchedulerSimulationReport,
) {
    let consumed = report.playout.playout_frames + report.playout.plc_frames;
    assert_eq!(
        consumed, config.frames,
        "every configured frame must be played or concealed"
    );
    assert_eq!(
        report.gated_ticks + consumed,
        report.observations,
        "each scheduler tick is one gated wait or one consumed frame"
    );
    assert!(
        (report.min_frames..=report.max_frames).contains(&report.final_target_frames),
        "final target must stay inside the bounded range"
    );
    assert!(
        report.forced_playouts <= config.frames,
        "forced playouts must not exceed the configured frame count"
    );
    assert!(
        report.final_buffered_frames <= JITTER_BUFFER_CAPACITY,
        "final buffered frames must not exceed the jitter-buffer capacity"
    );

    // Completion latency is bounded by the scenario's explicit hard deadline:
    // every frame may wait at most (max_gate_ticks + 2) scheduler periods plus
    // the worst-case one-way network delay before a forced playout.
    let hard_slack_ms = u64::from(config.frames)
        .saturating_mul(u64::from(config.max_gate_ticks + 2))
        .saturating_mul(config.frame_duration_ms)
        .saturating_add(config.network.base_delay_ms)
        .saturating_add(config.network.jitter_ms)
        .saturating_add(config.network.reorder_delay_ms)
        .saturating_add(2);
    assert!(
        report.excess_latency_ms <= hard_slack_ms,
        "completion must never exceed the hard deadline"
    );
}

#[test]
fn every_configuration_completes_with_bounded_target_and_latency() {
    for seed in [0x5EED_2026_0813_0001, 0xABCD_EF01_2345_6789] {
        let mut random = XorShift64(seed);
        for _ in 0..STEPS {
            let config = random_config(&mut random);
            let report = simulate_jitter_scheduler(config)
                .unwrap_or_else(|error| panic!("configuration {config:?} failed: {error}"));
            assert_invariants(config, &report);
        }
    }
}

#[test]
fn identical_configs_produce_identical_reports() {
    let mut random = XorShift64(0xDEAD_BEEF_2026_0813);
    for _ in 0..16 {
        let config = random_config(&mut random);
        let first = simulate_jitter_scheduler(config).unwrap();
        let second = simulate_jitter_scheduler(config).unwrap();
        assert_eq!(first, second);
    }
}
