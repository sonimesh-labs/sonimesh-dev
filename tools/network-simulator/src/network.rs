use std::collections::{BTreeMap, VecDeque};

use crate::{NetworkProfile, ProfileError};

/// One payload delivered by the virtual network.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Delivery<T> {
    /// Application payload.
    pub payload: T,
    /// Virtual send time.
    pub sent_at_ms: u64,
    /// Scheduled virtual arrival time.
    pub delivered_at_ms: u64,
}

/// Result of submitting one original packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SendOutcome {
    /// Original packet was scheduled, optionally with a duplicate.
    Scheduled {
        /// Number of copies admitted to the bounded in-flight queue.
        copies: u8,
    },
    /// Original packet matched random or burst loss.
    DroppedByLoss,
    /// In-flight queue had no capacity for the original packet.
    DroppedByCapacity,
    /// Sender-side socket buffer had no capacity; the packet was not
    /// scheduled and the caller applies its own backpressure policy.
    SendBufferFull,
}

/// Accumulated deterministic network counters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NetworkStats {
    /// Original packets submitted by the sender.
    pub submitted_packets: u64,
    /// Original and duplicate copies admitted to the event queue.
    pub scheduled_copies: u64,
    /// Copies removed by a receiver poll.
    pub delivered_copies: u64,
    /// Original packets removed by independent random loss.
    pub random_loss_packets: u64,
    /// Original packets removed by a deterministic loss burst.
    pub burst_loss_packets: u64,
    /// Original or duplicate copies rejected by the queue bound.
    pub capacity_dropped_copies: u64,
    /// Duplicate copies admitted to the queue.
    pub duplicate_copies: u64,
    /// Packets receiving additional reorder delay.
    pub reordered_packets: u64,
    /// Copies whose transfer completion was shaped by a finite link bandwidth.
    pub bandwidth_shaped_copies: u64,
    /// Bytes placed on the finite virtual link.
    pub bandwidth_serialized_bytes: u64,
    /// Total sender-side serialization wait before transfer began.
    pub bandwidth_queue_delay_ms: u64,
    /// Original packets rejected because the sender-side socket buffer was full.
    pub send_buffer_full_packets: u64,
    /// Bytes still reserved in the bounded sender-side socket buffer.
    pub send_buffer_queued_bytes: u64,
}

/// Bounded deterministic virtual packet transport.
pub struct VirtualNetwork<T> {
    profile: NetworkProfile,
    random: Lcg,
    events: BTreeMap<u64, VecDeque<Delivery<T>>>,
    queued_copies: usize,
    next_transmit_available_ms: u64,
    burst_remaining: u16,
    stats: NetworkStats,
    send_buffer: Option<SendBufferState>,
}

impl<T: Clone> VirtualNetwork<T> {
    /// Creates an empty virtual network.
    ///
    /// # Errors
    ///
    /// Returns [`ProfileError`] when `profile` violates a hard bound.
    pub fn new(profile: NetworkProfile) -> Result<Self, ProfileError> {
        profile.validate()?;
        Ok(Self {
            profile,
            random: Lcg::new(profile.seed),
            events: BTreeMap::new(),
            queued_copies: 0,
            next_transmit_available_ms: 0,
            burst_remaining: 0,
            stats: NetworkStats::default(),
            send_buffer: profile.send_buffer_bytes.map(SendBufferState::new),
        })
    }

    /// Applies configured impairments and schedules a payload.
    pub fn send(&mut self, now_ms: u64, payload: T, payload_bytes: u64) -> SendOutcome {
        let ordinal = self.stats.submitted_packets;
        saturating_increment(&mut self.stats.submitted_packets);

        let bandwidth_bytes_per_second = self.profile.bandwidth_bytes_per_second;
        if let Some(buffer) = &mut self.send_buffer {
            if !buffer.try_enqueue(now_ms, payload_bytes, bandwidth_bytes_per_second) {
                saturating_increment(&mut self.stats.send_buffer_full_packets);
                return SendOutcome::SendBufferFull;
            }
        }

        if self.should_drop_burst(ordinal) {
            saturating_increment(&mut self.stats.burst_loss_packets);
            return SendOutcome::DroppedByLoss;
        }
        if self.random.chance(self.profile.loss_percent) {
            saturating_increment(&mut self.stats.random_loss_packets);
            return SendOutcome::DroppedByLoss;
        }

        if !self.has_capacity() {
            self.record_capacity_drop();
            return SendOutcome::DroppedByCapacity;
        }
        let delay_ms = self.impairment_delay();
        let delivery_ms = self.delivery_time(now_ms, delay_ms, payload_bytes);
        let duplicate = self.random.chance(self.profile.duplicate_percent);
        let duplicate_payload = duplicate.then(|| payload.clone());
        self.schedule(now_ms, delivery_ms, payload);

        let mut copies = 1;
        if let Some(copy) = duplicate_payload {
            if self.has_capacity() {
                let duplicate_delivery_ms = self
                    .delivery_time(now_ms, delay_ms, payload_bytes)
                    .max(delivery_ms.saturating_add(1));
                self.schedule(now_ms, duplicate_delivery_ms, copy);
                saturating_increment(&mut self.stats.duplicate_copies);
                copies += 1;
            } else {
                self.record_capacity_drop();
            }
        }
        SendOutcome::Scheduled { copies }
    }

    /// Removes every copy whose delivery time is no later than `now_ms`.
    pub fn drain_ready(&mut self, now_ms: u64) -> Vec<Delivery<T>> {
        if let Some(buffer) = &mut self.send_buffer {
            buffer.drain_to(now_ms, self.profile.bandwidth_bytes_per_second);
        }
        let mut ready = Vec::new();
        while self
            .events
            .first_key_value()
            .is_some_and(|(delivery_ms, _)| *delivery_ms <= now_ms)
        {
            let Some((_, mut deliveries)) = self.events.pop_first() else {
                break;
            };
            self.queued_copies -= deliveries.len();
            ready.extend(deliveries.drain(..));
        }
        saturating_add(
            &mut self.stats.delivered_copies,
            u64::try_from(ready.len()).unwrap_or(u64::MAX),
        );
        ready
    }

    /// Current number of bounded scheduled copies.
    #[must_use]
    pub const fn queued_copies(&self) -> usize {
        self.queued_copies
    }

    /// Snapshot of deterministic counters.
    #[must_use]
    pub fn stats(&self) -> NetworkStats {
        let mut stats = self.stats;
        if let Some(buffer) = &self.send_buffer {
            stats.send_buffer_queued_bytes = buffer.queued_bytes();
        }
        stats
    }

    /// Bytes currently reserved in the bounded sender-side socket buffer.
    #[must_use]
    pub fn send_buffer_queued_bytes(&self) -> u64 {
        match &self.send_buffer {
            Some(buffer) => buffer.queued_bytes(),
            None => 0,
        }
    }

    fn should_drop_burst(&mut self, ordinal: u64) -> bool {
        if self.burst_remaining > 0 {
            self.burst_remaining -= 1;
            return true;
        }
        let interval = u64::from(self.profile.burst_interval_packets);
        if interval > 0 && ordinal > 0 && ordinal % interval == 0 {
            self.burst_remaining = self.profile.burst_length_packets.saturating_sub(1);
            return self.profile.burst_length_packets > 0;
        }
        false
    }

    fn impairment_delay(&mut self) -> u64 {
        let jitter_span = self.profile.jitter_ms.saturating_mul(2).saturating_add(1);
        let offset = self.random.below_u64(jitter_span);
        let mut delay = self
            .profile
            .base_delay_ms
            .saturating_add(offset)
            .saturating_sub(self.profile.jitter_ms);
        if self.random.chance(self.profile.reorder_percent) {
            delay = delay.saturating_add(self.profile.reorder_delay_ms);
            saturating_increment(&mut self.stats.reordered_packets);
        }
        delay
    }

    fn delivery_time(&mut self, now_ms: u64, delay_ms: u64, payload_bytes: u64) -> u64 {
        if self.profile.bandwidth_bytes_per_second == 0 {
            return now_ms.saturating_add(delay_ms);
        }

        let transmit_start_ms = now_ms.max(self.next_transmit_available_ms);
        saturating_increment(&mut self.stats.bandwidth_shaped_copies);
        self.stats.bandwidth_serialized_bytes = self
            .stats
            .bandwidth_serialized_bytes
            .saturating_add(payload_bytes);
        self.stats.bandwidth_queue_delay_ms = self
            .stats
            .bandwidth_queue_delay_ms
            .saturating_add(transmit_start_ms.saturating_sub(now_ms));
        let transfer_ms =
            serialized_duration_ms(payload_bytes, self.profile.bandwidth_bytes_per_second);
        let transmit_complete_ms = transmit_start_ms.saturating_add(transfer_ms);
        self.next_transmit_available_ms = transmit_complete_ms;
        transmit_complete_ms.saturating_add(delay_ms)
    }

    fn has_capacity(&self) -> bool {
        self.queued_copies < self.profile.max_in_flight_packets
    }

    fn record_capacity_drop(&mut self) {
        saturating_increment(&mut self.stats.capacity_dropped_copies);
    }

    fn schedule(&mut self, sent_at_ms: u64, delivered_at_ms: u64, payload: T) {
        self.events
            .entry(delivered_at_ms)
            .or_default()
            .push_back(Delivery {
                payload,
                sent_at_ms,
                delivered_at_ms,
            });
        self.queued_copies += 1;
        saturating_increment(&mut self.stats.scheduled_copies);
    }
}

/// Bounded sender-side socket buffer modeled as a byte credit.
///
/// Bytes admitted by [`VirtualNetwork::send`] stay reserved until the virtual
/// clock advances and the configured link bandwidth drains them; a full buffer
/// reports backpressure instead of scheduling the packet. A packet rejected by
/// loss or in-flight capacity keeps its reservation until the next drain, a
/// conservative approximation of a socket buffer that frees bytes only as the
/// link transmits.
struct SendBufferState {
    capacity_bytes: u64,
    queued_bytes: u64,
    last_drain_ms: u64,
}

impl SendBufferState {
    fn new(capacity_bytes: u64) -> Self {
        Self {
            capacity_bytes,
            queued_bytes: 0,
            last_drain_ms: 0,
        }
    }

    fn try_enqueue(&mut self, now_ms: u64, bytes: u64, bytes_per_second: u64) -> bool {
        if bytes_per_second == 0 {
            // Without a finite link the socket buffer drains instantly and
            // never reports backpressure.
            return true;
        }
        self.drain_to(now_ms, bytes_per_second);
        if self.queued_bytes.saturating_add(bytes) > self.capacity_bytes {
            return false;
        }
        self.queued_bytes = self.queued_bytes.saturating_add(bytes);
        true
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
        self.queued_bytes = self.queued_bytes.saturating_sub(drained);
    }

    fn queued_bytes(&self) -> u64 {
        self.queued_bytes
    }
}

fn saturating_increment(counter: &mut u64) {
    saturating_add(counter, 1);
}

fn saturating_add(counter: &mut u64, amount: u64) {
    *counter = counter.saturating_add(amount);
}

fn serialized_duration_ms(payload_bytes: u64, bandwidth_bytes_per_second: u64) -> u64 {
    let whole_seconds = payload_bytes / bandwidth_bytes_per_second;
    let remainder = payload_bytes % bandwidth_bytes_per_second;
    let partial_millis = remainder
        .saturating_mul(1_000)
        .saturating_add(bandwidth_bytes_per_second.saturating_sub(1))
        / bandwidth_bytes_per_second;
    whole_seconds
        .saturating_mul(1_000)
        .saturating_add(partial_millis)
}

struct Lcg(u64);

impl Lcg {
    const fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn chance(&mut self, percent: u8) -> bool {
        self.below_u64(100) < u64::from(percent)
    }

    fn below_u64(&mut self, upper: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        if upper == 0 {
            return 0;
        }
        (self.0 >> 1) % upper
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_is_never_exceeded() {
        let mut profile = NetworkProfile::stable(1);
        profile.max_in_flight_packets = 2;
        profile.duplicate_percent = 100;
        let mut network = VirtualNetwork::new(profile).unwrap();

        network.send(0, 1_u32, u64::from(u32::BITS / 8));
        network.send(0, 2_u32, u64::from(u32::BITS / 8));

        assert_eq!(network.queued_copies(), 2);
        assert!(network.stats().capacity_dropped_copies > 0);
    }

    #[test]
    fn polls_all_events_before_requested_time() {
        let mut profile = NetworkProfile::stable(1);
        profile.base_delay_ms = 10;
        profile.jitter_ms = 0;
        let mut network = VirtualNetwork::new(profile).unwrap();
        network.send(0, 1_u32, u64::from(u32::BITS / 8));
        network.send(5, 2_u32, u64::from(u32::BITS / 8));

        let ready = network.drain_ready(20);
        assert_eq!(ready.len(), 2);
        assert_eq!(network.queued_copies(), 0);
    }

    #[test]
    fn bandwidth_serializes_back_to_back_packets() {
        let mut profile = NetworkProfile::stable(1);
        profile.base_delay_ms = 0;
        profile.jitter_ms = 0;
        profile.bandwidth_bytes_per_second = 1_000;
        let mut network = VirtualNetwork::new(profile).unwrap();

        network.send(0, 1_u32, 100);
        network.send(0, 2_u32, 100);

        assert!(network.drain_ready(99).is_empty());
        let first = network.drain_ready(100);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].payload, 1);
        let second = network.drain_ready(200);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].payload, 2);
        assert_eq!(network.stats().bandwidth_shaped_copies, 2);
        assert_eq!(network.stats().bandwidth_serialized_bytes, 200);
        assert_eq!(network.stats().bandwidth_queue_delay_ms, 100);
    }

    #[test]
    fn duplicate_copy_is_dropped_independently_when_capacity_is_exhausted() {
        let mut profile = NetworkProfile::stable(1);
        profile.duplicate_percent = 100;
        profile.max_in_flight_packets = 1;
        let mut network = VirtualNetwork::new(profile).unwrap();

        assert_eq!(
            network.send(0, 7_u32, 100),
            SendOutcome::Scheduled { copies: 1 }
        );
        assert_eq!(network.queued_copies(), 1);
        assert_eq!(network.stats().scheduled_copies, 1);
        assert_eq!(network.stats().duplicate_copies, 0);
        assert_eq!(network.stats().capacity_dropped_copies, 1);

        let delivered = network.drain_ready(u64::MAX);
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].payload, 7);
    }
    #[test]
    fn capacity_drop_does_not_consume_bandwidth_time() {
        let mut profile = NetworkProfile::stable(1);
        profile.base_delay_ms = 0;
        profile.jitter_ms = 0;
        profile.bandwidth_bytes_per_second = 1_000;
        profile.max_in_flight_packets = 1;
        let mut network = VirtualNetwork::new(profile).unwrap();

        assert_eq!(
            network.send(0, 1_u32, 100),
            SendOutcome::Scheduled { copies: 1 }
        );
        assert_eq!(network.send(0, 2_u32, 100), SendOutcome::DroppedByCapacity);
        assert_eq!(network.drain_ready(100).len(), 1);
        assert_eq!(
            network.send(100, 3_u32, 100),
            SendOutcome::Scheduled { copies: 1 }
        );

        let ready = network.drain_ready(200);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].payload, 3);
        assert_eq!(network.stats().bandwidth_serialized_bytes, 200);
    }

    #[test]
    fn cumulative_stats_saturate_instead_of_wrapping() {
        let mut profile = NetworkProfile::stable(1);
        profile.base_delay_ms = 0;
        profile.jitter_ms = 0;
        profile.bandwidth_bytes_per_second = 1;
        profile.duplicate_percent = 100;
        profile.reorder_percent = 100;
        profile.max_in_flight_packets = 4;
        let mut network = VirtualNetwork::new(profile).unwrap();
        network.stats.submitted_packets = u64::MAX - 1;
        network.stats.scheduled_copies = u64::MAX - 1;
        network.stats.delivered_copies = u64::MAX - 1;
        network.stats.duplicate_copies = u64::MAX - 1;
        network.stats.reordered_packets = u64::MAX - 1;
        network.stats.bandwidth_shaped_copies = u64::MAX - 1;
        network.stats.bandwidth_serialized_bytes = u64::MAX - 1;
        network.stats.bandwidth_queue_delay_ms = u64::MAX - 1;
        network.stats.capacity_dropped_copies = u64::MAX - 1;
        network.stats.random_loss_packets = u64::MAX - 1;
        network.stats.burst_loss_packets = u64::MAX - 1;

        assert_eq!(
            network.send(0, 1_u32, 1),
            SendOutcome::Scheduled { copies: 2 }
        );
        let ready = network.drain_ready(u64::MAX);
        assert_eq!(ready.len(), 2);
        network.record_capacity_drop();

        let snapshot = network.stats();
        assert_eq!(snapshot.submitted_packets, u64::MAX);
        assert_eq!(snapshot.scheduled_copies, u64::MAX);
        assert_eq!(snapshot.delivered_copies, u64::MAX);
        assert_eq!(snapshot.duplicate_copies, u64::MAX);
        assert_eq!(snapshot.reordered_packets, u64::MAX);
        assert_eq!(snapshot.bandwidth_shaped_copies, u64::MAX);
        assert_eq!(snapshot.bandwidth_serialized_bytes, u64::MAX);
        assert_eq!(snapshot.bandwidth_queue_delay_ms, u64::MAX);
        assert_eq!(snapshot.capacity_dropped_copies, u64::MAX);

        network.stats.random_loss_packets = u64::MAX - 1;
        network.profile.loss_percent = 100;
        assert_eq!(network.send(0, 2_u32, 1), SendOutcome::DroppedByLoss);
        assert_eq!(network.stats.random_loss_packets, u64::MAX);

        network.stats.burst_loss_packets = u64::MAX - 1;
        network.profile.loss_percent = 0;
        network.profile.burst_interval_packets = 1;
        network.profile.burst_length_packets = 1;
        assert_eq!(network.send(0, 3_u32, 1), SendOutcome::DroppedByLoss);
        assert_eq!(network.stats.burst_loss_packets, u64::MAX);
    }

    #[test]
    fn default_profile_never_reports_send_buffer_backpressure() {
        let profile = NetworkProfile::stable(1);
        let mut network = VirtualNetwork::new(profile).unwrap();

        for _ in 0..16 {
            assert_ne!(network.send(0, 0_u32, 1_024), SendOutcome::SendBufferFull);
        }
        assert_eq!(network.stats().send_buffer_full_packets, 0);
        assert_eq!(network.send_buffer_queued_bytes(), 0);
    }

    #[test]
    fn full_send_buffer_reports_backpressure_without_scheduling() {
        let mut profile = NetworkProfile::stable(1);
        profile.send_buffer_bytes = Some(100);
        profile.bandwidth_bytes_per_second = 1_000; // one byte per millisecond
        let mut network = VirtualNetwork::new(profile).unwrap();

        assert_eq!(
            network.send(0, 1_u32, 100),
            SendOutcome::Scheduled { copies: 1 }
        );
        assert_eq!(network.send(0, 2_u32, 1), SendOutcome::SendBufferFull);
        assert_eq!(network.stats().send_buffer_full_packets, 1);
        assert_eq!(network.send_buffer_queued_bytes(), 100);
        assert!(network.drain_ready(0).is_empty());
        assert_eq!(network.drain_ready(1_000).len(), 1);
        assert_eq!(network.send_buffer_queued_bytes(), 0);
    }

    #[test]
    fn send_buffer_drains_at_link_bandwidth_over_virtual_time() {
        let mut profile = NetworkProfile::stable(1);
        profile.send_buffer_bytes = Some(100);
        profile.bandwidth_bytes_per_second = 1_000; // one byte per millisecond
        let mut network = VirtualNetwork::new(profile).unwrap();

        assert_eq!(
            network.send(0, 1_u32, 100),
            SendOutcome::Scheduled { copies: 1 }
        );
        assert_eq!(network.send(0, 2_u32, 1), SendOutcome::SendBufferFull);

        // 50 ms later the link drained 50 reserved bytes, so 51 bytes still
        // exceed the one-hundred-byte buffer.
        assert_eq!(network.send(50, 3_u32, 51), SendOutcome::SendBufferFull);
        // After 100 ms the reservation is fully drained and 100 bytes fit.
        assert_eq!(
            network.send(100, 4_u32, 100),
            SendOutcome::Scheduled { copies: 1 }
        );
    }

    #[test]
    fn unshaped_link_never_backpressures_the_send_buffer() {
        let mut profile = NetworkProfile::stable(1);
        profile.send_buffer_bytes = Some(100);
        profile.bandwidth_bytes_per_second = 0;
        let mut network = VirtualNetwork::new(profile).unwrap();

        for _ in 0..16 {
            assert_ne!(network.send(0, 0_u32, 100), SendOutcome::SendBufferFull);
        }
        assert_eq!(network.stats().send_buffer_full_packets, 0);
    }

    #[test]
    fn send_buffer_full_counter_saturates_instead_of_wrapping() {
        let mut profile = NetworkProfile::stable(1);
        profile.send_buffer_bytes = Some(1);
        profile.bandwidth_bytes_per_second = 1;
        let mut network = VirtualNetwork::new(profile).unwrap();
        network.stats.send_buffer_full_packets = u64::MAX - 1;

        assert_eq!(
            network.send(0, 1_u32, 1),
            SendOutcome::Scheduled { copies: 1 }
        );
        assert_eq!(network.send(0, 2_u32, 1), SendOutcome::SendBufferFull);
        assert_eq!(network.stats().send_buffer_full_packets, u64::MAX);
    }

    #[test]
    fn send_buffer_reservation_is_kept_by_loss_drops() {
        let mut profile = NetworkProfile::stable(1);
        profile.send_buffer_bytes = Some(100);
        profile.bandwidth_bytes_per_second = 1_000; // one byte per millisecond
        profile.loss_percent = 100;
        let mut network = VirtualNetwork::new(profile).unwrap();

        // The sender reserves bytes in the socket buffer before loss applies,
        // so a loss-dropped packet keeps its reservation until the next drain.
        assert_eq!(network.send(0, 1_u32, 100), SendOutcome::DroppedByLoss);
        assert_eq!(network.send_buffer_queued_bytes(), 100);
        assert_eq!(network.stats().send_buffer_full_packets, 0);

        // The stale reservation can still backpressure the next submission.
        assert_eq!(network.send(0, 2_u32, 1), SendOutcome::SendBufferFull);
        assert_eq!(network.stats().send_buffer_full_packets, 1);

        // Advancing virtual time drains the reservation at link bandwidth.
        assert!(network.drain_ready(1_000).is_empty());
        assert_eq!(network.send_buffer_queued_bytes(), 0);
    }

    #[test]
    fn send_buffer_backpressure_precedes_duplication() {
        let mut profile = NetworkProfile::stable(1);
        profile.send_buffer_bytes = Some(100);
        profile.bandwidth_bytes_per_second = 1_000; // one byte per millisecond
        profile.duplicate_percent = 100;
        let mut network = VirtualNetwork::new(profile).unwrap();

        // The first submission schedules original plus duplicate while the
        // buffer holds one hundred reserved bytes.
        assert_eq!(
            network.send(0, 1_u32, 100),
            SendOutcome::Scheduled { copies: 2 }
        );
        assert_eq!(network.stats().scheduled_copies, 2);
        assert_eq!(network.stats().duplicate_copies, 1);

        // Backpressure rejects the next submission before any duplicate copy
        // can be admitted.
        assert_eq!(network.send(0, 2_u32, 1), SendOutcome::SendBufferFull);
        assert_eq!(network.stats().send_buffer_full_packets, 1);
        assert_eq!(network.stats().scheduled_copies, 2);
        assert_eq!(network.stats().duplicate_copies, 1);
    }

    #[test]
    fn send_buffer_reservation_is_kept_by_capacity_drops() {
        let mut profile = NetworkProfile::stable(1);
        profile.send_buffer_bytes = Some(1_000);
        profile.bandwidth_bytes_per_second = 1_000; // one byte per millisecond
        profile.max_in_flight_packets = 1;
        let mut network = VirtualNetwork::new(profile).unwrap();

        assert_eq!(
            network.send(0, 1_u32, 100),
            SendOutcome::Scheduled { copies: 1 }
        );
        assert_eq!(network.send(0, 2_u32, 100), SendOutcome::DroppedByCapacity);
        assert_eq!(network.send_buffer_queued_bytes(), 200);

        // The in-flight queue stays full, so further submissions keep being
        // capacity-dropped while their reservations accumulate.
        assert_eq!(network.send(0, 3_u32, 100), SendOutcome::DroppedByCapacity);
        assert_eq!(network.send_buffer_queued_bytes(), 300);
        assert_eq!(network.stats().send_buffer_full_packets, 0);

        // Virtual time drains the reservation at link bandwidth, but a still
        // full in-flight queue keeps dropping submissions with new reservations.
        assert!(network.drain_ready(50).is_empty());
        assert_eq!(network.send_buffer_queued_bytes(), 250);
        assert_eq!(network.send(50, 4_u32, 100), SendOutcome::DroppedByCapacity);
        assert_eq!(network.send_buffer_queued_bytes(), 350);
        assert_eq!(network.stats().capacity_dropped_copies, 3);
    }
}
