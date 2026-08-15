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
//!
//! Profiles also vary the media datagram size and occasionally configure a
//! finite serial link and a bounded sender-side socket buffer shared by all
//! streams, so the walk pins the bandwidth-serialization accounting (every
//! scheduled copy is serialized as one datagram) and the no-link/no-buffer
//! backpressure semantics.

use sonimesh_audio::{JitterMode, JitterTargetConfig};
use sonimesh_network_simulator::{
    MultiStreamJitterTargetScenarioConfig, MultiStreamJitterTargetSimulationReport, NetworkProfile,
    simulate_multi_stream_jitter_target,
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

/// Jitter-buffer capacity shared by the scenario and its target presets.
const JITTER_BUFFER_CAPACITY: usize = 64;
/// Number of seeded configurations per property run.
const STEPS: usize = 64;
/// Streams in each scenario run.
const STREAMS: usize = 3;

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

/// Builds one valid multi-stream configuration from the seeded random stream.
fn random_config(random: &mut XorShift64) -> MultiStreamJitterTargetScenarioConfig<STREAMS> {
    let frame_duration_ms: u64 = if random.next_u32() % 2 == 0 { 10 } else { 20 };
    let frame_duration = u32::try_from(frame_duration_ms).unwrap();
    let media_datagram_bytes =
        u16::try_from(random.next_usize(MAX_MEDIA_DATAGRAM_LEN) + 1).unwrap();
    MultiStreamJitterTargetScenarioConfig {
        frames: random.next_in(30, 201),
        frame_duration_ms,
        target_buffer_ms: u64::from(random.next_in(10, 301)),
        media_datagram_bytes,
        network: random_profile(random, media_datagram_bytes),
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
                streams: STREAMS,
            },
        );
    }
}

#[test]
fn every_configuration_completes_with_bounded_per_stream_targets() {
    let mut shaped_runs = 0_u32;
    let mut backpressure_runs = 0_u32;
    for seed in [0x5EED_2026_0813_0002, 0xABCD_EF01_2345_6790] {
        let mut random = XorShift64(seed);
        for _ in 0..STEPS {
            let config = random_config(&mut random);
            if config.network.bandwidth_bytes_per_second > 0 {
                shaped_runs += 1;
            }
            let report = simulate_multi_stream_jitter_target::<STREAMS>(config)
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
    let mut random = XorShift64(0xDEAD_BEEF_2026_0813);
    for _ in 0..16 {
        let config = random_config(&mut random);
        let first = simulate_multi_stream_jitter_target::<STREAMS>(config).unwrap();
        let second = simulate_multi_stream_jitter_target::<STREAMS>(config).unwrap();
        assert_eq!(first, second);
    }
}
