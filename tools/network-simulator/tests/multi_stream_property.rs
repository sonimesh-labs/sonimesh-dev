//! Deterministic property coverage for the multi-stream shared-link scenario.
//!
//! Two to four source streams traverse one shared virtual link and profile, so
//! correlated loss and bandwidth serialization are observed against independent
//! per-stream jitter buffers and playout deadlines. Every accepted report must
//! be deterministic, must submit exactly one packet per stream per frame over
//! the shared link, and must account for every playout decision: each stream
//! plays out or conceals exactly its configured frame count. This test drives
//! seeded random profiles, frame durations, and initial playout targets across
//! the full Phase 2 stream range (two, three, and four independent sources) and
//! asserts those invariants on every accepted report, mirroring the
//! deterministic property pattern used for the sibling scenarios.
//!
//! Profiles also vary the media datagram size and occasionally configure a
//! finite serial link and a bounded sender-side socket buffer shared by all
//! streams, so the walk pins the bandwidth-serialization accounting (every
//! scheduled copy is serialized as one datagram) and the no-link/no-buffer
//! backpressure semantics.

use sonimesh_network_simulator::{
    MAX_MULTI_STREAM_STREAMS, MIN_MULTI_STREAM_STREAMS, MultiStreamScenarioConfig,
    MultiStreamSimulationReport, NetworkProfile, NetworkStats, simulate_multi_stream,
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

/// Number of seeded configurations per property run.
const STEPS: usize = 256;
/// Sender frames per stream stay small so every property step is bounded.
const MAX_FRAMES: u32 = 200;

/// Mirrors the production drain semantics (bytes drain at
/// `bandwidth * elapsed_ms / 1000` whenever the clock advances) so the shaped
/// walk can cross-check backpressure counts without reaching into the
/// library's private state. It is deliberately a separate, simpler
/// implementation: if the two diverge, the production path is wrong.
struct SendBufferCreditModel {
    capacity_bytes: u64,
    queued_bytes: u64,
    last_drain_ms: u64,
    full_packets: u64,
}

impl SendBufferCreditModel {
    fn new(capacity_bytes: u64) -> Self {
        Self {
            capacity_bytes,
            queued_bytes: 0,
            last_drain_ms: 0,
            full_packets: 0,
        }
    }

    fn drain_to(&mut self, now_ms: u64, bytes_per_second: u64) {
        if now_ms <= self.last_drain_ms {
            return;
        }
        let elapsed_ms = now_ms - self.last_drain_ms;
        self.last_drain_ms = now_ms;
        let drained = self
            .queued_bytes
            .min(bytes_per_second.saturating_mul(elapsed_ms) / 1_000);
        self.queued_bytes -= drained;
    }

    fn try_enqueue(&mut self, now_ms: u64, bytes: u64, bytes_per_second: u64) -> bool {
        if bytes_per_second == 0 {
            return true;
        }
        self.drain_to(now_ms, bytes_per_second);
        if self.queued_bytes.saturating_add(bytes) > self.capacity_bytes {
            self.full_packets += 1;
            return false;
        }
        self.queued_bytes += bytes;
        true
    }
}

/// Replays the scenario's exact submission timeline for the credit model:
/// every stream submits one datagram every `frame_duration_ms` from time zero
/// on the shared link until the inclusive last virtual tick the production
/// loop processes.
struct SubmissionTimeline {
    media_datagram_bytes: u16,
    frames: u32,
    frame_duration_ms: u64,
    end_ms: u64,
    streams: usize,
}

/// Cross-checks the report's sender-side backpressure counters against an
/// independent byte-credit model that replays the scenario's submission
/// timeline exactly, draining at the link bandwidth whenever the virtual clock
/// advances.
fn cross_check_send_buffer_credit(
    stats: &NetworkStats,
    profile: &NetworkProfile,
    timeline: &SubmissionTimeline,
) {
    let capacity = profile.send_buffer_bytes.expect("checked Some");
    let bandwidth = profile.bandwidth_bytes_per_second;
    let mut model = SendBufferCreditModel::new(capacity);
    let mut next_send_sequence = 0_u32;
    let mut next_send_ms = 0_u64;
    for now_ms in 0..=timeline.end_ms {
        if next_send_sequence < timeline.frames && now_ms == next_send_ms {
            for _ in 0..timeline.streams {
                model.try_enqueue(now_ms, u64::from(timeline.media_datagram_bytes), bandwidth);
            }
            next_send_sequence += 1;
            next_send_ms = next_send_ms.saturating_add(timeline.frame_duration_ms);
        }
        model.drain_to(now_ms, bandwidth);
    }
    assert_eq!(
        model.full_packets, stats.send_buffer_full_packets,
        "independent byte-credit model reproduces sender backpressure"
    );
    assert_eq!(
        model.queued_bytes, stats.send_buffer_queued_bytes,
        "independent byte-credit model reproduces the final sender-side queued bytes"
    );
    assert!(
        stats.send_buffer_queued_bytes <= capacity,
        "sender-side queued bytes never exceed the configured socket-buffer capacity"
    );
}

/// Builds a validated network profile within the scenario impairment range.
///
/// The ranges mirror the mixer scenario property tests and fuzz targets so the
/// seeded run covers the same shared-link configuration space. The media
/// datagram size is passed in so a tight sender-side socket buffer can be sized
/// relative to the datagram the scenario actually transmits.
fn random_profile(random: &mut XorShift64, media_datagram_bytes: u16) -> NetworkProfile {
    let mut network = NetworkProfile::stable(u64::from(random.next_u32()));
    network.base_delay_ms = u64::from(random.next_in(0, 101));
    network.jitter_ms = u64::from(random.next_in(0, 41));
    network.reorder_delay_ms = u64::from(random.next_in(0, 101));
    network.loss_percent = u8::try_from(random.next_in(0, 101)).unwrap();
    network.duplicate_percent = u8::try_from(random.next_in(0, 101)).unwrap();
    network.reorder_percent = u8::try_from(random.next_in(0, 101)).unwrap();
    // A zero interval disables bursts; otherwise the burst never overlaps its
    // interval so `NetworkProfile::validate` always accepts the pair.
    let burst_interval_packets = match random.next_usize(8) {
        0 => 0_u32,
        1 => 2,
        2 => 4,
        3 => 8,
        4 => 12,
        5 => 16,
        6 => 20,
        _ => 32,
    };
    network.burst_interval_packets = burst_interval_packets;
    network.burst_length_packets = if burst_interval_packets == 0 {
        0
    } else {
        u16::try_from(random.next_in(1, burst_interval_packets + 1)).unwrap()
    };
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

/// Builds a validated multi-stream configuration whose initial playout target
/// covers the worst-case first-arrival delay, so an intact first packet is
/// always in the jitter buffer by the first playout deadline.
fn random_config(random: &mut XorShift64) -> MultiStreamScenarioConfig {
    let frame_duration_ms = if random.next_usize(2) == 0 {
        10_u64
    } else {
        20_u64
    };
    let media_datagram_bytes =
        u16::try_from(random.next_in(1, u32::try_from(MAX_MEDIA_DATAGRAM_LEN).unwrap() + 1))
            .unwrap();
    let network = random_profile(random, media_datagram_bytes);
    let worst_case_first_arrival_ms =
        network.base_delay_ms + network.jitter_ms + network.reorder_delay_ms;
    let target_buffer_ms = worst_case_first_arrival_ms
        + u64::from(random.next_in(
            0,
            1_001 - u32::try_from(worst_case_first_arrival_ms).unwrap(),
        ));
    MultiStreamScenarioConfig {
        frames: random.next_in(1, MAX_FRAMES + 1),
        frame_duration_ms,
        target_buffer_ms,
        media_datagram_bytes,
        network,
    }
}

/// Asserts the shared-link invariants that hold for every accepted report.
fn assert_scenario_invariants<const STREAMS: usize>(
    report: &MultiStreamSimulationReport<STREAMS>,
    config: &MultiStreamScenarioConfig,
) {
    assert_eq!(
        report.streams.len(),
        STREAMS,
        "one report per source stream"
    );
    assert_eq!(
        report.network.submitted_packets,
        u64::from(config.frames) * u64::try_from(STREAMS).unwrap(),
        "each stream must submit exactly one packet per frame on the shared link"
    );
    for stream in &report.streams {
        assert_eq!(
            stream.playout_frames + stream.plc_frames,
            config.frames,
            "every playout decision must play out or conceal exactly one frame"
        );
        assert!(
            stream.playout_frames <= config.frames,
            "a stream cannot play out more frames than it sends"
        );
        assert!(
            stream.plc_frames <= config.frames,
            "a stream cannot conceal more frames than it sends"
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

/// Drives `STEPS` seeded configurations at a fixed stream count, asserts the
/// scenario invariants on every accepted report, and returns how many accepted
/// reports exercised the finite-link and sender-side backpressure paths.
fn check_stream_count<const S: usize>() -> (u32, u32) {
    let mut random = XorShift64(0x5A17_2026);
    let mut shaped_runs = 0_u32;
    let mut backpressure_runs = 0_u32;
    for _ in 0..STEPS {
        let config = random_config(&mut random);
        // Playout is pre-armed at the negotiated start sequence, so a lost or
        // late first packet conceals via PLC rather than reporting NotReady;
        // accepted reports must satisfy the invariants.
        if let Ok(first) = simulate_multi_stream::<S>(config) {
            if config.network.bandwidth_bytes_per_second > 0 {
                shaped_runs += 1;
            }
            if first.network.send_buffer_full_packets > 0 {
                backpressure_runs += 1;
            }
            let second = simulate_multi_stream::<S>(config)
                .expect("an accepted configuration must stay accepted");
            assert_eq!(
                first, second,
                "the shared-link simulation must be deterministic"
            );
            assert_scenario_invariants(&first, &config);
        }
    }
    (shaped_runs, backpressure_runs)
}

#[test]
fn every_accepted_configuration_stays_bounded_deterministic_and_accounted() {
    // Phase 2 admits two to four independent sources; verify every stream count
    // in the range over the same seeded profiles.
    let mut shaped_runs = 0_u32;
    let mut backpressure_runs = 0_u32;
    for streams in MIN_MULTI_STREAM_STREAMS..=MAX_MULTI_STREAM_STREAMS {
        let (shaped, backpressure) = match streams {
            2 => check_stream_count::<2>(),
            3 => check_stream_count::<3>(),
            _ => check_stream_count::<4>(),
        };
        shaped_runs += shaped;
        backpressure_runs += backpressure;
    }
    assert!(shaped_runs > 0, "walk must exercise the finite-link path");
    assert!(
        backpressure_runs > 0,
        "walk must exercise sender-side backpressure"
    );
}
