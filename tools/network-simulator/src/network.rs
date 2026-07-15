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
}

/// Bounded deterministic virtual packet transport.
pub struct VirtualNetwork<T> {
    profile: NetworkProfile,
    random: Lcg,
    events: BTreeMap<u64, VecDeque<Delivery<T>>>,
    queued_copies: usize,
    burst_remaining: u16,
    stats: NetworkStats,
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
            burst_remaining: 0,
            stats: NetworkStats::default(),
        })
    }

    /// Applies configured impairments and schedules a payload.
    pub fn send(&mut self, now_ms: u64, payload: T) -> SendOutcome {
        let ordinal = self.stats.submitted_packets;
        self.stats.submitted_packets += 1;

        if self.should_drop_burst(ordinal) {
            self.stats.burst_loss_packets += 1;
            return SendOutcome::DroppedByLoss;
        }
        if self.random.chance(self.profile.loss_percent) {
            self.stats.random_loss_packets += 1;
            return SendOutcome::DroppedByLoss;
        }

        let delivery_ms = self.delivery_time(now_ms);
        let duplicate = self.random.chance(self.profile.duplicate_percent);
        let duplicate_payload = duplicate.then(|| payload.clone());
        if !self.schedule(now_ms, delivery_ms, payload) {
            return SendOutcome::DroppedByCapacity;
        }

        let mut copies = 1;
        if let Some(copy) = duplicate_payload {
            if self.schedule(now_ms, delivery_ms.saturating_add(1), copy) {
                self.stats.duplicate_copies += 1;
                copies += 1;
            }
        }
        SendOutcome::Scheduled { copies }
    }

    /// Removes every copy whose delivery time is no later than `now_ms`.
    pub fn drain_ready(&mut self, now_ms: u64) -> Vec<Delivery<T>> {
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
        self.stats.delivered_copies += u64::try_from(ready.len()).unwrap_or(u64::MAX);
        ready
    }

    /// Current number of bounded scheduled copies.
    #[must_use]
    pub const fn queued_copies(&self) -> usize {
        self.queued_copies
    }

    /// Snapshot of deterministic counters.
    #[must_use]
    pub const fn stats(&self) -> NetworkStats {
        self.stats
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

    fn delivery_time(&mut self, now_ms: u64) -> u64 {
        let jitter_span = self.profile.jitter_ms.saturating_mul(2).saturating_add(1);
        let offset = self.random.below_u64(jitter_span);
        let mut delay = self
            .profile
            .base_delay_ms
            .saturating_add(offset)
            .saturating_sub(self.profile.jitter_ms);
        if self.random.chance(self.profile.reorder_percent) {
            delay = delay.saturating_add(self.profile.reorder_delay_ms);
            self.stats.reordered_packets += 1;
        }
        now_ms.saturating_add(delay)
    }

    fn schedule(&mut self, sent_at_ms: u64, delivered_at_ms: u64, payload: T) -> bool {
        if self.queued_copies >= self.profile.max_in_flight_packets {
            self.stats.capacity_dropped_copies += 1;
            return false;
        }
        self.events
            .entry(delivered_at_ms)
            .or_default()
            .push_back(Delivery {
                payload,
                sent_at_ms,
                delivered_at_ms,
            });
        self.queued_copies += 1;
        self.stats.scheduled_copies += 1;
        true
    }
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

        network.send(0, 1_u32);
        network.send(0, 2_u32);

        assert_eq!(network.queued_copies(), 2);
        assert!(network.stats().capacity_dropped_copies > 0);
    }

    #[test]
    fn polls_all_events_before_requested_time() {
        let mut profile = NetworkProfile::stable(1);
        profile.base_delay_ms = 10;
        profile.jitter_ms = 0;
        let mut network = VirtualNetwork::new(profile).unwrap();
        network.send(0, 1_u32);
        network.send(5, 2_u32);

        let ready = network.drain_ready(20);
        assert_eq!(ready.len(), 2);
        assert_eq!(network.queued_copies(), 0);
    }
}
