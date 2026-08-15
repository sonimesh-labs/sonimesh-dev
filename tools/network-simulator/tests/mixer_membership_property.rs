//! Deterministic property coverage for the fixed-slot mixer membership
//! simulation over a shared virtual link.
//!
//! ADR 0015's fixed-slot `CommandDrivenMixer` is driven from a multi-stream
//! shared-link run: every stream's playout is decoded to a finite PCM block
//! and pushed into its slot, the callback renders the bus every tick, and a
//! mid-session remove/reinsert exercises membership churn. Every accepted
//! report must consume every frame through playout or PLC, submit exactly one
//! packet per stream per frame, keep both streams active in the fixed-slot
//! table after the reinsert, publish the two starts plus one reinsert and one
//! remove, and render a finite bus. This test drives seeded random network
//! profiles over a long run and asserts those invariants on every report,
//! mirroring the deterministic property pattern used for the other mixer
//! scenarios.
//!
//! Profiles also vary the media datagram size and occasionally configure a
//! finite serial link and a bounded sender-side socket buffer shared by both
//! streams, so the walk pins the bandwidth-serialization accounting (every
//! scheduled copy is serialized as one datagram) and the no-link/no-buffer
//! backpressure semantics.

use sonimesh_network_simulator::{
    MixerMembershipScenarioConfig, MixerMembershipSimulationReport, NetworkProfile, NetworkStats,
    simulate_mixer_membership,
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
const STEPS: usize = 128;
/// Two source streams share one link and the mixer table.
const STREAMS: usize = 2;
/// Fixed slot count of the mixer membership table.
const SLOTS: usize = 4;
/// Output-format PCM block size used by the mixer callback.
const PCM_BLOCK: usize = 64;

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

/// Builds one valid configuration from the seeded random stream.
fn random_config(random: &mut XorShift64) -> MixerMembershipScenarioConfig<STREAMS> {
    let media_datagram_bytes =
        u16::try_from(random.next_usize(MAX_MEDIA_DATAGRAM_LEN) + 1).unwrap();
    MixerMembershipScenarioConfig {
        frames: random.next_in(30, 201),
        frame_duration_ms: if random.next_u32() % 2 == 0 { 10 } else { 20 },
        target_buffer_ms: u64::from(random.next_in(10, 301)),
        media_datagram_bytes,
        network: random_profile(random, media_datagram_bytes),
    }
}

/// Asserts every mixer-membership invariant on one completed report.
fn assert_invariants(
    config: &MixerMembershipScenarioConfig<STREAMS>,
    report: &MixerMembershipSimulationReport<STREAMS, SLOTS>,
) {
    assert_eq!(
        report.network.submitted_packets,
        u64::from(config.frames) * u64::try_from(STREAMS).unwrap(),
        "each stream must submit exactly one packet per frame"
    );
    for (index, stream) in report.streams.iter().enumerate() {
        assert_eq!(
            stream.playout_frames + stream.plc_frames,
            config.frames,
            "stream {index} must play or conceal every configured frame"
        );
    }
    assert_eq!(
        report.mixer.slots, SLOTS,
        "the report must expose the fixed slot count"
    );
    assert_eq!(
        report.mixer.active_streams, STREAMS,
        "both streams must be active after the reinsert"
    );
    assert_eq!(
        report.mixer.published_inserts, 3,
        "two starts plus the mid-session reinsert"
    );
    assert_eq!(report.mixer.published_removes, 1, "the mid-session remove");
    assert!(report.mixer.renders > 0, "the callback must render the bus");
    assert!(report.mixer.output_finite, "the bus must stay finite");

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
fn every_configuration_completes_with_bounded_mixer_bus() {
    let mut shaped_runs = 0_u32;
    let mut backpressure_runs = 0_u32;
    for seed in [0x5EED_2026_0814_0002, 0xABCD_EF01_2345_6792] {
        let mut random = XorShift64(seed);
        for _ in 0..STEPS {
            let config = random_config(&mut random);
            if config.network.bandwidth_bytes_per_second > 0 {
                shaped_runs += 1;
            }
            let report = simulate_mixer_membership::<STREAMS, SLOTS, PCM_BLOCK>(config)
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
        let first = simulate_mixer_membership::<STREAMS, SLOTS, PCM_BLOCK>(config).unwrap();
        let second = simulate_mixer_membership::<STREAMS, SLOTS, PCM_BLOCK>(config).unwrap();
        assert_eq!(first, second);
    }
}
