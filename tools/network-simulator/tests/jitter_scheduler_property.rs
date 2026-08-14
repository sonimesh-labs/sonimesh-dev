//! Deterministic property coverage for the target-gated playout scheduler.
//!
//! The scheduler scenario must always complete for any valid configuration:
//! every configured frame is played or concealed exactly once, each scheduler
//! tick is either one gated wait or one consumed frame, the adaptive target
//! stays inside its bounded `[min, max]` range, and completion never exceeds
//! the explicit hard deadline. This test drives seeded random network
//! profiles, controller modes, and gate deadlines over a long run and asserts
//! those invariants on every report.
//!
//! Profiles also vary the media datagram size and occasionally configure a
//! finite serial link and a bounded sender-side socket buffer, so the walk
//! pins the bandwidth-serialization accounting (every scheduled copy is
//! serialized as one datagram) and the no-link/no-buffer backpressure
//! semantics.

use sonimesh_audio::{JitterMode, JitterTargetConfig};
use sonimesh_network_simulator::{
    JitterSchedulerScenarioConfig, JitterSchedulerSimulationReport, NetworkProfile,
    simulate_jitter_scheduler,
};
use sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN;

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

    /// Returns a value in `0..=100`.
    fn percent(&mut self) -> usize {
        self.next_usize(101)
    }
}

/// Jitter-buffer capacity shared by the scenario and its target presets.
const JITTER_BUFFER_CAPACITY: usize = 64;
/// Number of seeded configurations per property run.
const STEPS: usize = 128;

/// Builds a validated network profile within the scheduler impairment range.
///
/// The media datagram size is passed in so a tight sender-side socket buffer
/// can be sized relative to the datagram the scenario actually transmits.
fn random_profile(random: &mut XorShift64, media_datagram_bytes: u16) -> NetworkProfile {
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
    // Occasionally shape the link and bound the sender-side socket buffer so
    // the walk exercises finite-link serialization and backpressure. Slow
    // links guarantee shaping pressure; tight buffers guarantee backpressure.
    let shaped = random.percent() >= 10;
    network.bandwidth_bytes_per_second = if shaped && random.percent() < 30 {
        u64::try_from(random.next_usize(4_801) + 200).unwrap() // 200..=5_000 bytes/s
    } else if shaped {
        u64::try_from(random.next_usize(1_995_001) + 5_000).unwrap() // 5_000..=2_000_000 bytes/s
    } else {
        0
    };
    network.max_in_flight_packets = random.next_usize(15) + 2;
    // Mix bufferless, tight, and roomy sender-side socket buffers.
    network.send_buffer_bytes = if !shaped || random.percent() < 30 {
        None
    } else if random.percent() < 50 {
        Some(u64::try_from(random.next_usize(usize::from(media_datagram_bytes) * 8) + 1).unwrap())
    } else {
        Some(u64::try_from(random.next_usize(2_000_000) + 1).unwrap())
    };
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
    let media_datagram_bytes =
        u16::try_from(random.next_usize(MAX_MEDIA_DATAGRAM_LEN) + 1).unwrap();
    JitterSchedulerScenarioConfig {
        frames: random.next_in(30, 201),
        frame_duration_ms,
        initial_playout_ms: u64::from(random.next_in(10, 301)),
        media_datagram_bytes,
        network: random_profile(random, media_datagram_bytes),
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

    // Finite-link serialization accounting: every scheduled copy (original or
    // duplicate) is serialized as exactly one media datagram, and an unshaped
    // link serializes nothing.
    let bandwidth = config.network.bandwidth_bytes_per_second;
    if bandwidth == 0 {
        assert_eq!(
            report.network.bandwidth_shaped_copies, 0,
            "an unshaped link must not serialize any copy"
        );
        assert_eq!(
            report.network.bandwidth_serialized_bytes, 0,
            "an unshaped link must not place bytes on a link"
        );
    } else {
        assert_eq!(
            report.network.bandwidth_shaped_copies, report.network.scheduled_copies,
            "every scheduled copy is serialized on the finite link"
        );
        assert_eq!(
            report.network.bandwidth_serialized_bytes,
            report.network.scheduled_copies * u64::from(config.media_datagram_bytes),
            "serialized bytes equal one datagram per scheduled copy"
        );
    }

    // Without a finite link or without a socket buffer the sender never
    // reports backpressure, by construction of the model.
    if config.network.send_buffer_bytes.is_none() || bandwidth == 0 {
        assert_eq!(
            report.network.send_buffer_full_packets, 0,
            "no finite link or no socket buffer means no backpressure"
        );
        assert_eq!(
            report.network.send_buffer_queued_bytes, 0,
            "no finite link or no socket buffer means no queued bytes"
        );
    } else {
        let capacity = config.network.send_buffer_bytes.expect("checked Some");
        assert!(
            report.network.send_buffer_queued_bytes <= capacity,
            "sender-side queued bytes never exceed the configured socket-buffer capacity"
        );
    }
}

#[test]
fn every_configuration_completes_with_bounded_target_and_latency() {
    let mut shaped_runs = 0_u32;
    let mut backpressure_runs = 0_u32;
    for seed in [0x5EED_2026_0813_0001, 0xABCD_EF01_2345_6789] {
        let mut random = XorShift64(seed);
        for _ in 0..STEPS {
            let config = random_config(&mut random);
            if config.network.bandwidth_bytes_per_second > 0 {
                shaped_runs += 1;
            }
            let report = simulate_jitter_scheduler(config)
                .unwrap_or_else(|error| panic!("configuration {config:?} failed: {error}"));
            if report.network.send_buffer_full_packets > 0 {
                backpressure_runs += 1;
            }
            assert_invariants(config, &report);
        }
    }
    assert!(shaped_runs > 0, "walk must exercise the finite-link path");
    assert!(
        backpressure_runs > 0,
        "walk must exercise sender-side backpressure"
    );
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
