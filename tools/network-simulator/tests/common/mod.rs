//! Shared test support for the deterministic network-simulator property walks.
//!
//! The byte-credit model here is deliberately independent of the library: it
//! replays a scenario's submission timeline with its own drain math so the
//! walks can cross-check the production sender-side backpressure counters
//! without reaching into private state. If the two diverge, the production
//! path is wrong.

use sonimesh_network_simulator::{NetworkProfile, NetworkStats};

/// Independent byte-credit model of the sender-side socket buffer.
///
/// Mirrors the production drain semantics (bytes drain at
/// `bandwidth * elapsed_ms / 1000` whenever the clock advances) so the shaped
/// walk can cross-check backpressure counts without reaching into the
/// library's private state. It is deliberately a separate, simpler
/// implementation: if the two diverge, the production path is wrong.
pub struct SendBufferCreditModel {
    capacity_bytes: u64,
    queued_bytes: u64,
    last_drain_ms: u64,
    full_packets: u64,
}

impl SendBufferCreditModel {
    pub fn new(capacity_bytes: u64) -> Self {
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

    pub fn try_enqueue(&mut self, now_ms: u64, bytes: u64, bytes_per_second: u64) -> bool {
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
pub struct SubmissionTimeline {
    pub media_datagram_bytes: u16,
    pub frames: u32,
    pub frame_duration_ms: u64,
    pub end_ms: u64,
    pub streams: usize,
}

/// Cross-checks the report's sender-side backpressure counters against an
/// independent byte-credit model that replays the scenario's submission
/// timeline exactly, draining at the link bandwidth whenever the virtual clock
/// advances.
pub fn cross_check_send_buffer_credit(
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
