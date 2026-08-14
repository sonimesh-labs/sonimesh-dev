//! Seeded property coverage for the plain deterministic scenario.
//!
//! Drives [`ScenarioConfig`]/`simulate` under random bounded profiles and
//! asserts the cross-layer invariants that hold for every deterministic run:
//! identical-seed determinism, every playout slot rendering decoded audio or
//! PLC, the network-level submission conservation, jitter-buffer duplicates
//! never exceeding network duplicate copies, and — when nothing is dropped or
//! duplicated — every missed slot's frame arriving exactly once afterward and
//! being counted late.

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
