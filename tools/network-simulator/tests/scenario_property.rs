//! Seeded property coverage for the deterministic scenario.
//!
//! Drives [`ScenarioConfig`]/`simulate` under random bounded profiles and
//! asserts the cross-layer invariants that hold for every deterministic run:
//! identical-seed determinism, every playout slot rendering decoded audio or
//! PLC, the network-level submission conservation, jitter-buffer duplicates
//! never exceeding network duplicate copies, and — when nothing is dropped or
//! duplicated — every missed slot's frame arriving exactly once afterward and
//! being counted late. A second walk adds a finite serial link and sender-side
//! socket buffer, cross-checking sender backpressure against an independent
//! byte-credit model and pinning the serialization and submission accounting.

use sonimesh_network_simulator::{NetworkProfile, ScenarioConfig, simulate};
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

    fn percent(&mut self) -> u8 {
        u8::try_from(self.next_usize(101)).expect("percent bounded by 100")
    }
}

fn random_config(random: &mut XorShift64) -> ScenarioConfig {
    let mut network = NetworkProfile::stable(0x5C3A_2026);
    network.loss_percent = random.percent().min(20);
    network.duplicate_percent = random.percent().min(25);
    network.reorder_percent = random.percent().min(40);
    network.base_delay_ms = u64::try_from(random.next_usize(61)).unwrap();
    network.jitter_ms = u64::try_from(random.next_usize(31)).unwrap();
    network.reorder_delay_ms = u64::try_from(random.next_usize(41)).unwrap();
    // No serial-link shaping or sender-side socket buffer: the walk isolates
    // transport loss/reorder/duplication against the jitter buffer.
    network.bandwidth_bytes_per_second = 0;
    network.send_buffer_bytes = None;
    network.burst_interval_packets = 0;
    network.burst_length_packets = 0;
    ScenarioConfig {
        frames: u32::try_from(random.next_usize(161) + 40).unwrap(),
        frame_duration_ms: if random.next_usize(2) == 0 { 10 } else { 20 },
        target_buffer_ms: u64::try_from(random.next_usize(201)).unwrap(),
        media_datagram_bytes: u16::try_from(random.next_usize(MAX_MEDIA_DATAGRAM_LEN) + 1).unwrap(),
        network,
    }
}

#[test]
fn scenario_walk_preserves_cross_layer_invariants() {
    let mut random = XorShift64(0xA11C_E202);

    for _ in 0..80 {
        let config = random_config(&mut random);
        let first = simulate(config).expect("valid bounded scenario runs");
        let second = simulate(config).expect("valid bounded scenario runs");

        assert_eq!(first, second, "identical seed stays deterministic");
        assert_eq!(
            first.playout_frames + first.plc_frames,
            config.frames,
            "every playout slot renders decoded audio or PLC"
        );
        assert_eq!(
            first.network.capacity_dropped_copies, 0,
            "bounded delays never fill the in-flight queue"
        );
        assert_eq!(
            first.resynchronizations, 0,
            "bounded occupancy never forces a jitter reset"
        );
        // With no send-buffer and no capacity drops, every submission is
        // either lost or scheduled: scheduled originals equal submitted minus
        // losses, and scheduled copies add only admitted duplicates.
        assert_eq!(
            first.network.submitted_packets,
            first.network.burst_loss_packets
                + first.network.random_loss_packets
                + first.network.scheduled_copies
                - first.network.duplicate_copies,
            "network-level submission conservation"
        );
        assert!(
            u64::from(first.duplicate_frames) <= first.network.duplicate_copies,
            "jitter duplicates are a subset of network duplicate copies"
        );
        // Late is not a subset of PLC: a duplicate copy arriving after its
        // frame's slot played counts as Late even when that slot rendered the
        // original frame. The excess over PLC is bounded by duplicate copies.
        assert!(
            u64::from(first.late_frames)
                <= u64::from(first.plc_frames) + first.network.duplicate_copies,
            "late arrivals beyond PLC come only from duplicate copies"
        );
        assert!(
            first.network.delivered_copies <= first.network.scheduled_copies,
            "delivered copies never exceed scheduled copies"
        );
        if first.network.random_loss_packets == 0
            && first.network.burst_loss_packets == 0
            && first.network.duplicate_copies == 0
        {
            assert_eq!(
                first.late_frames, first.plc_frames,
                "no loss or duplication: every missed frame arrives once and is counted late"
            );
            assert_eq!(
                first.late_frames + first.playout_frames,
                config.frames,
                "late arrivals complete the playout accounting"
            );
        }
    }
}

/// Independent byte-credit model of the sender-side socket buffer.
///
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

fn random_shaped_config(random: &mut XorShift64) -> ScenarioConfig {
    let mut network = NetworkProfile::stable(0x5B1F_2026);
    network.loss_percent = random.percent().min(20);
    network.duplicate_percent = random.percent().min(25);
    network.reorder_percent = random.percent().min(40);
    network.base_delay_ms = u64::try_from(random.next_usize(61)).unwrap();
    network.jitter_ms = u64::try_from(random.next_usize(31)).unwrap();
    network.reorder_delay_ms = u64::try_from(random.next_usize(41)).unwrap();
    // Occasional unshaped profiles pin the "no finite link means no sender
    // backpressure" semantics; slow links guarantee the walk exercises both
    // backpressure and in-flight capacity pressure.
    let shaped = random.percent() >= 10;
    network.bandwidth_bytes_per_second = if shaped && random.percent() < 30 {
        u64::try_from(random.next_usize(4_801) + 200).unwrap() // 200..=5_000 bytes/s
    } else if shaped {
        u64::try_from(random.next_usize(1_995_001) + 5_000).unwrap() // 5_000..=2_000_000 bytes/s
    } else {
        0
    };
    network.burst_interval_packets = u32::try_from(random.next_usize(46) + 5).unwrap();
    network.burst_length_packets = u16::try_from(
        random.next_usize(usize::try_from(network.burst_interval_packets).unwrap()) + 1,
    )
    .unwrap();
    network.max_in_flight_packets = random.next_usize(15) + 2;
    let frames = u32::try_from(random.next_usize(161) + 40).unwrap();
    let frame_duration_ms = if random.next_usize(2) == 0 { 10 } else { 20 };
    let target_buffer_ms = u64::try_from(random.next_usize(201)).unwrap();
    let media_datagram_bytes =
        u16::try_from(random.next_usize(MAX_MEDIA_DATAGRAM_LEN) + 1).unwrap();
    // Mix bufferless, tight, and roomy sender-side socket buffers so the walk
    // covers all three regimes while the credit model cross-checks each one.
    network.send_buffer_bytes = if !shaped || random.percent() < 30 {
        None
    } else if random.percent() < 50 {
        Some(u64::try_from(random.next_usize(usize::from(media_datagram_bytes) * 8) + 1).unwrap())
    } else {
        Some(u64::try_from(random.next_usize(2_000_000) + 1).unwrap())
    };
    ScenarioConfig {
        frames,
        frame_duration_ms,
        target_buffer_ms,
        media_datagram_bytes,
        network,
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn shaped_walk_preserves_sender_side_and_conservation_invariants() {
    let mut random = XorShift64(0x0BAA_D2026);
    let mut shaped_runs = 0_u32;
    let mut backpressure_runs = 0_u32;
    let mut capacity_drop_runs = 0_u32;

    for _ in 0..80 {
        let config = random_shaped_config(&mut random);
        let first = simulate(config).expect("valid bounded scenario runs");
        let second = simulate(config).expect("valid bounded scenario runs");

        assert_eq!(first, second, "identical seed stays deterministic");
        assert_eq!(
            first.network.submitted_packets,
            u64::from(config.frames),
            "every sender frame is submitted exactly once"
        );
        assert_eq!(
            first.playout_frames + first.plc_frames,
            config.frames,
            "every playout slot renders decoded audio or PLC"
        );
        assert!(
            first.network.delivered_copies <= first.network.scheduled_copies,
            "delivered copies never exceed scheduled copies"
        );
        assert!(
            u64::from(first.duplicate_frames) <= first.network.duplicate_copies,
            "jitter duplicates are a subset of network duplicate copies"
        );
        // Late is not a subset of PLC: a duplicate copy arriving after its
        // frame's slot played counts as Late even when that slot rendered the
        // original frame. The excess over PLC is bounded by duplicate copies.
        assert!(
            u64::from(first.late_frames)
                <= u64::from(first.plc_frames) + first.network.duplicate_copies,
            "late arrivals beyond PLC come only from duplicate copies"
        );
        let bandwidth = config.network.bandwidth_bytes_per_second;
        if bandwidth == 0 {
            assert_eq!(first.network.bandwidth_shaped_copies, 0);
            assert_eq!(first.network.bandwidth_serialized_bytes, 0);
        } else {
            shaped_runs += 1;
            assert_eq!(
                first.network.bandwidth_shaped_copies, first.network.scheduled_copies,
                "every scheduled copy is serialized on the finite link"
            );
            assert_eq!(
                first.network.bandwidth_serialized_bytes,
                first.network.scheduled_copies * u64::from(config.media_datagram_bytes),
                "serialized bytes equal one datagram per scheduled copy"
            );
        }

        // Without a finite link or without a socket buffer the sender never
        // reports backpressure, by construction of the model.
        if config.network.send_buffer_bytes.is_none() || bandwidth == 0 {
            assert_eq!(
                first.network.send_buffer_full_packets, 0,
                "no finite link or no socket buffer means no backpressure"
            );
        } else {
            let capacity = config.network.send_buffer_bytes.expect("checked Some");
            let mut model = SendBufferCreditModel::new(capacity);
            let last_playout_ms = config.target_buffer_ms.saturating_add(
                u64::from(config.frames.saturating_sub(1)).saturating_mul(config.frame_duration_ms),
            );
            let end_ms = last_playout_ms
                .saturating_add(config.network.base_delay_ms)
                .saturating_add(config.network.jitter_ms)
                .saturating_add(config.network.reorder_delay_ms)
                .saturating_add(2);
            let mut next_send_sequence = 0_u32;
            let mut next_send_ms = 0_u64;
            for now_ms in 0..=end_ms {
                if next_send_sequence < config.frames && now_ms == next_send_ms {
                    model.try_enqueue(now_ms, u64::from(config.media_datagram_bytes), bandwidth);
                    next_send_sequence += 1;
                    next_send_ms = next_send_ms.saturating_add(config.frame_duration_ms);
                }
                model.drain_to(now_ms, bandwidth);
            }
            assert_eq!(
                model.full_packets, first.network.send_buffer_full_packets,
                "independent byte-credit model reproduces sender backpressure"
            );
            assert_eq!(
                model.queued_bytes, first.network.send_buffer_queued_bytes,
                "independent byte-credit model reproduces the final sender-side queued bytes"
            );
        }

        if first.network.send_buffer_full_packets > 0 {
            backpressure_runs += 1;
        }
        if first.network.capacity_dropped_copies > 0 {
            capacity_drop_runs += 1;
        }

        // Every submission is either lost, backpressured, capacity-dropped, or
        // scheduled as an original. Capacity drops may also consume duplicate
        // copies, so the equality is exact only when none occurred.
        assert!(
            first.network.submitted_packets
                >= first.network.burst_loss_packets
                    + first.network.random_loss_packets
                    + first.network.send_buffer_full_packets
                    + (first.network.scheduled_copies - first.network.duplicate_copies),
            "every non-scheduled outcome is counted without double counting"
        );
        if first.network.capacity_dropped_copies == 0 {
            assert_eq!(
                first.network.submitted_packets,
                first.network.burst_loss_packets
                    + first.network.random_loss_packets
                    + first.network.send_buffer_full_packets
                    + (first.network.scheduled_copies - first.network.duplicate_copies),
                "without capacity drops every submission is lost, backpressured, or scheduled"
            );
        }
    }

    assert!(shaped_runs > 0, "walk must exercise the finite-link path");
    assert!(
        backpressure_runs > 0,
        "walk must exercise sender-side backpressure"
    );
    assert!(
        capacity_drop_runs > 0,
        "walk must exercise in-flight capacity drops"
    );
}
