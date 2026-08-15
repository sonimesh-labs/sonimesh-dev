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
//!
//! Profiles also vary the media datagram size and occasionally configure a
//! finite serial link and a bounded sender-side socket buffer, so the walk
//! pins the bandwidth-serialization accounting (every scheduled copy is
//! serialized as one datagram) and the no-link/no-buffer backpressure
//! semantics.

use sonimesh_audio::{JitterMode, JitterTargetConfig};
use sonimesh_network_simulator::{
    JitterTargetScenarioConfig, JitterTargetSimulationReport, NetworkProfile,
    simulate_jitter_target,
};
use sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN;

mod common;
use common::{SubmissionTimeline, cross_check_send_buffer_credit};

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

/// Jitter-buffer capacity shared by the scenario and its target preset.
const JITTER_BUFFER_CAPACITY: usize = 64;
/// Number of seeded configurations per property run.
const STEPS: usize = 128;

/// Builds a validated network profile within the scenario impairment range.
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
    let media_datagram_bytes =
        u16::try_from(random.next_usize(MAX_MEDIA_DATAGRAM_LEN) + 1).unwrap();
    JitterTargetScenarioConfig {
        frames: random.next_in(30, 201),
        frame_duration_ms,
        // The scenario pre-arms playout at the negotiated start sequence, so
        // any valid initial target is accepted: a playout before the first
        // frame arrives conceals instead of rejecting.
        target_buffer_ms: u64::from(random.next_in(0, 1_001)),
        media_datagram_bytes,
        network: random_profile(random, media_datagram_bytes),
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
        cross_check_send_buffer_credit(
            &report.network,
            &config.network,
            &SubmissionTimeline {
                media_datagram_bytes: config.media_datagram_bytes,
                frames: config.frames,
                frame_duration_ms: config.frame_duration_ms,
                end_ms: config
                    .target_buffer_ms
                    .saturating_add(
                        u64::from(config.frames.saturating_sub(1))
                            .saturating_mul(config.frame_duration_ms),
                    )
                    .saturating_add(config.network.base_delay_ms)
                    .saturating_add(config.network.jitter_ms)
                    .saturating_add(config.network.reorder_delay_ms)
                    .saturating_add(2),
                streams: 1,
            },
        );
    }
}

#[test]
fn every_configuration_completes_with_bounded_target() {
    let mut shaped_runs = 0_u32;
    let mut backpressure_runs = 0_u32;
    for seed in [0x5EED_2026_0814_0001, 0xABCD_EF01_2345_6791] {
        let mut random = XorShift64(seed);
        for _ in 0..STEPS {
            let config = random_config(&mut random);
            if config.network.bandwidth_bytes_per_second > 0 {
                shaped_runs += 1;
            }
            let report = simulate_jitter_target(config)
                .unwrap_or_else(|error| panic!("configuration {config:?} failed: {error}"));
            if report.network.send_buffer_full_packets > 0 {
                backpressure_runs += 1;
            }
            assert_invariants(&config, &report);
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
    let mut random = XorShift64(0xDEAD_BEEF_2026_0814);
    for _ in 0..16 {
        let config = random_config(&mut random);
        let first = simulate_jitter_target(config).unwrap();
        let second = simulate_jitter_target(config).unwrap();
        assert_eq!(first, second);
    }
}
