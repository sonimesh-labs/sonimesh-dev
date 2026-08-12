//! Deterministic property coverage for the bounded virtual packet transport.
//!
//! `VirtualNetwork` publishes a small observable surface: a bounded in-flight
//! queue, per-impairment counters, and time-stamped deliveries, all driven by
//! a deterministic LCG. This test drives real networks under seeded send/drain
//! walks against independent models: a plain accounting model that predicts
//! the burst-loss pattern, the loss/drop split, the finite-link serialization
//! state, and every published counter, plus a per-payload delivery tracker
//! that checks send time, delivery time, ordering, and duplicate invariants.
//! Identical-seed determinism and the delivery-time boundary are covered by
//! separate targeted tests.

use std::collections::HashMap;

use sonimesh_network_simulator::{Delivery, NetworkProfile, SendOutcome, VirtualNetwork};

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

    fn next_u64(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            return 0;
        }
        let hi = u64::from(self.next_u32());
        let lo = u64::from(self.next_u32());
        ((hi << 32) | lo) % bound
    }

    fn percent(&mut self) -> u8 {
        u8::try_from(self.next_usize(101)).expect("percent bounded by 100")
    }
}

/// Predicts the deterministic burst-loss pattern from the submission ordinal.
///
/// A burst opens at every positive multiple of `burst_interval_packets` and
/// covers the next `burst_length_packets` packets. Because validation requires
/// `burst_length <= burst_interval`, bursts never overlap.
fn is_burst_drop(ordinal: u64, profile: &NetworkProfile) -> bool {
    let interval = u64::from(profile.burst_interval_packets);
    if interval == 0 {
        return false;
    }
    let burst = ordinal / interval;
    burst >= 1 && ordinal - burst * interval < u64::from(profile.burst_length_packets)
}

/// Ceil-division serialization duration used by the independent shaping model.
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

/// Independent accounting model of the network's observable counters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct NetworkModel {
    submitted: u64,
    scheduled_copies: u64,
    delivered_copies: u64,
    burst_loss: u64,
    random_loss: u64,
    original_capacity_drops: u64,
    known_duplicate_drops: u64,
    duplicate_copies: u64,
    bandwidth_serialized_bytes: u64,
    bandwidth_queue_delay_ms: u64,
    next_transmit_available_ms: u64,
}

impl NetworkModel {
    fn on_send(
        &mut self,
        profile: &NetworkProfile,
        now_ms: u64,
        payload_bytes: u64,
        outcome: SendOutcome,
    ) {
        let ordinal = self.submitted;
        self.submitted = self.submitted.saturating_add(1);

        match outcome {
            SendOutcome::DroppedByLoss => {
                if is_burst_drop(ordinal, profile) {
                    self.burst_loss = self.burst_loss.saturating_add(1);
                } else {
                    self.random_loss = self.random_loss.saturating_add(1);
                }
            }
            SendOutcome::DroppedByCapacity => {
                self.original_capacity_drops = self.original_capacity_drops.saturating_add(1);
            }
            SendOutcome::Scheduled { copies } => {
                self.scheduled_copies = self.scheduled_copies.saturating_add(u64::from(copies));
                self.duplicate_copies = self
                    .duplicate_copies
                    .saturating_add(u64::from(copies.saturating_sub(1)));
                // With duplication at 100%, every admitted original must have
                // rolled a duplicate, so a single-copy outcome can only mean
                // the duplicate copy was rejected by the queue bound.
                if profile.duplicate_percent == 100 && copies == 1 {
                    self.known_duplicate_drops = self.known_duplicate_drops.saturating_add(1);
                }
                if profile.bandwidth_bytes_per_second > 0 {
                    for _ in 0..copies {
                        self.shape(now_ms, payload_bytes, profile.bandwidth_bytes_per_second);
                    }
                }
            }
        }
    }

    fn shape(&mut self, now_ms: u64, payload_bytes: u64, bandwidth_bytes_per_second: u64) {
        let transmit_start = now_ms.max(self.next_transmit_available_ms);
        self.bandwidth_queue_delay_ms = self
            .bandwidth_queue_delay_ms
            .saturating_add(transmit_start.saturating_sub(now_ms));
        self.bandwidth_serialized_bytes = self
            .bandwidth_serialized_bytes
            .saturating_add(payload_bytes);
        let transfer = serialized_duration_ms(payload_bytes, bandwidth_bytes_per_second);
        self.next_transmit_available_ms = transmit_start.saturating_add(transfer);
    }

    fn on_drain(&mut self, delivered: usize) {
        self.delivered_copies = self
            .delivered_copies
            .saturating_add(u64::try_from(delivered).expect("drain length stays tiny"));
    }
}

/// Tracks every scheduled payload id through the delivery stream.
#[derive(Default)]
struct DeliveryTracker {
    sent_at_by_id: HashMap<u64, u64>,
    expected_copies_by_id: HashMap<u64, u8>,
    seen_counts: HashMap<u64, u8>,
    last_delivered_at: HashMap<u64, u64>,
}

impl DeliveryTracker {
    fn register(&mut self, id: u64, sent_at_ms: u64, copies: u8) {
        self.sent_at_by_id.insert(id, sent_at_ms);
        self.expected_copies_by_id.insert(id, copies);
    }

    fn check_delivery(&mut self, delivery: &Delivery<u64>, drain_now_ms: u64) {
        let id = delivery.payload;
        let expected = *self
            .expected_copies_by_id
            .get(&id)
            .expect("every delivery carries a scheduled id");
        let seen = self.seen_counts.get(&id).copied().unwrap_or(0);
        assert!(
            seen < expected,
            "id {id} must not deliver more copies than were scheduled"
        );
        assert_eq!(
            delivery.sent_at_ms,
            *self
                .sent_at_by_id
                .get(&id)
                .expect("scheduled id records a send time"),
            "id {id} delivers its send time"
        );
        assert!(
            delivery.delivered_at_ms >= delivery.sent_at_ms,
            "id {id} never arrives before it was sent"
        );
        assert!(
            delivery.delivered_at_ms <= drain_now_ms,
            "id {id} drain never returns future deliveries"
        );
        if seen > 0 {
            let last = *self
                .last_delivered_at
                .get(&id)
                .expect("prior copy records a delivery time");
            assert!(
                delivery.delivered_at_ms > last,
                "id {id} duplicate arrives strictly after its original"
            );
        }
        self.last_delivered_at.insert(id, delivery.delivered_at_ms);
        self.seen_counts.insert(id, seen + 1);
    }

    fn assert_all_delivered(&self) {
        for (id, expected) in &self.expected_copies_by_id {
            let seen = self.seen_counts.get(id).copied().unwrap_or(0);
            assert_eq!(
                seen, *expected,
                "id {id} must deliver exactly its scheduled copies"
            );
        }
    }
}

/// Covers every impairment path at least once across the curated profiles.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct WalkCoverage(u8);

impl WalkCoverage {
    const BURST_LOSS: u8 = 1 << 0;
    const RANDOM_LOSS: u8 = 1 << 1;
    const DUPLICATE: u8 = 1 << 2;
    const DUPLICATE_DROP: u8 = 1 << 3;
    const ORIGINAL_DROP: u8 = 1 << 4;
    const SHAPED: u8 = 1 << 5;

    fn mark(&mut self, flag: u8) {
        self.0 |= flag;
    }

    fn covered(self, flag: u8) -> bool {
        self.0 & flag != 0
    }
}

fn curated_profiles() -> Vec<NetworkProfile> {
    let mut profiles = Vec::new();

    let mut heavy = NetworkProfile::stable(0x4E47_0000_2026_0001);
    heavy.loss_percent = 30;
    heavy.duplicate_percent = 40;
    heavy.reorder_percent = 50;
    heavy.base_delay_ms = 10;
    heavy.jitter_ms = 20;
    heavy.reorder_delay_ms = 40;
    heavy.max_in_flight_packets = 64;
    profiles.push(heavy);

    let mut shaped = NetworkProfile::stable(0x4E47_0000_2026_0002);
    shaped.loss_percent = 10;
    shaped.duplicate_percent = 90;
    shaped.base_delay_ms = 5;
    shaped.jitter_ms = 5;
    shaped.bandwidth_bytes_per_second = 2_000;
    shaped.max_in_flight_packets = 8;
    profiles.push(shaped);

    let mut bursts = NetworkProfile::stable(0x4E47_0000_2026_0003);
    bursts.duplicate_percent = 100;
    bursts.reorder_percent = 100;
    bursts.base_delay_ms = 1;
    bursts.jitter_ms = 0;
    bursts.reorder_delay_ms = 5;
    bursts.burst_interval_packets = 7;
    bursts.burst_length_packets = 3;
    bursts.max_in_flight_packets = 2;
    profiles.push(bursts);

    let mut everything = NetworkProfile::stable(0x4E47_0000_2026_0004);
    everything.loss_percent = 25;
    everything.duplicate_percent = 50;
    everything.reorder_percent = 75;
    everything.base_delay_ms = 20;
    everything.jitter_ms = 10;
    everything.reorder_delay_ms = 20;
    everything.bandwidth_bytes_per_second = 1_000;
    everything.burst_interval_packets = 5;
    everything.burst_length_packets = 2;
    everything.max_in_flight_packets = 4;
    profiles.push(everything);

    // Duplication is certain and the queue holds one copy, so every scheduled
    // original must reject its duplicate copy by capacity: this pins the
    // capacity-dropped accounting exactly.
    let mut pinned = NetworkProfile::stable(0x4E47_0000_2026_0005);
    pinned.duplicate_percent = 100;
    pinned.base_delay_ms = 5;
    pinned.jitter_ms = 5;
    pinned.max_in_flight_packets = 1;
    profiles.push(pinned);

    profiles
}

fn random_profile(rng: &mut XorShift64) -> NetworkProfile {
    let mut profile = NetworkProfile::stable(rng.next_u64(u64::MAX));
    profile.loss_percent = rng.percent();
    profile.duplicate_percent = rng.percent();
    profile.reorder_percent = rng.percent();
    profile.base_delay_ms = rng.next_u64(201);
    profile.jitter_ms = rng.next_u64(201);
    profile.reorder_delay_ms = rng.next_u64(201);
    if rng.next_usize(4) != 0 {
        profile.bandwidth_bytes_per_second = 100 + rng.next_u64(10_000);
    }
    if rng.next_usize(3) != 0 {
        let interval = 2 + rng.next_u32() % 49;
        profile.burst_interval_packets = interval;
        profile.burst_length_packets =
            u16::try_from(1 + rng.next_u32() % interval).expect("burst length bounded by 50");
    }
    profile.max_in_flight_packets = 1 + rng.next_usize(64);
    profile
}

fn check_invariants(network: &VirtualNetwork<u64>, model: &NetworkModel, profile: &NetworkProfile) {
    let stats = network.stats();
    assert_eq!(
        stats.submitted_packets, model.submitted,
        "submitted packets"
    );
    assert_eq!(
        stats.scheduled_copies, model.scheduled_copies,
        "scheduled copies"
    );
    assert_eq!(
        stats.delivered_copies, model.delivered_copies,
        "delivered copies"
    );
    assert_eq!(
        stats.burst_loss_packets, model.burst_loss,
        "burst loss packets"
    );
    assert_eq!(
        stats.random_loss_packets, model.random_loss,
        "random loss packets"
    );
    assert_eq!(
        stats.duplicate_copies, model.duplicate_copies,
        "duplicate copies"
    );
    assert_eq!(
        stats.bandwidth_serialized_bytes, model.bandwidth_serialized_bytes,
        "bandwidth serialized bytes"
    );
    assert_eq!(
        stats.bandwidth_queue_delay_ms, model.bandwidth_queue_delay_ms,
        "bandwidth queue delay"
    );
    if profile.bandwidth_bytes_per_second > 0 {
        assert_eq!(
            stats.bandwidth_shaped_copies, stats.scheduled_copies,
            "every scheduled copy is shaped on a finite link"
        );
    } else {
        assert_eq!(
            stats.bandwidth_shaped_copies, 0,
            "no shaping without a finite link"
        );
    }

    // Every submitted packet is accounted for exactly once across burst loss,
    // random loss, an original capacity drop, or a scheduled copy; admitted
    // duplicates appear only inside `scheduled_copies`.
    let accounted = stats
        .burst_loss_packets
        .saturating_add(stats.random_loss_packets)
        .saturating_add(model.original_capacity_drops)
        .saturating_add(
            stats
                .scheduled_copies
                .saturating_sub(stats.duplicate_copies),
        );
    assert_eq!(
        stats.submitted_packets, accounted,
        "submission conservation across all outcomes"
    );

    if profile.duplicate_percent == 100 {
        assert_eq!(
            stats.capacity_dropped_copies,
            model
                .original_capacity_drops
                .saturating_add(model.known_duplicate_drops),
            "capacity accounting is exact when duplicates are certain"
        );
    } else {
        assert!(
            stats.capacity_dropped_copies >= model.original_capacity_drops,
            "capacity drops cover every original drop plus any duplicate drops"
        );
    }

    let queued = network.queued_copies();
    assert_eq!(
        u64::try_from(queued).expect("queued copies stay tiny"),
        stats
            .scheduled_copies
            .saturating_sub(stats.delivered_copies),
        "queued copies equal scheduled minus delivered"
    );
    assert!(
        queued <= profile.max_in_flight_packets,
        "in-flight bound must never be exceeded"
    );
}

fn drain_and_check(
    network: &mut VirtualNetwork<u64>,
    model: &mut NetworkModel,
    tracker: &mut DeliveryTracker,
    profile: &NetworkProfile,
    now_ms: u64,
) {
    let delivered = network.drain_ready(now_ms);
    model.on_drain(delivered.len());
    for delivery in &delivered {
        tracker.check_delivery(delivery, now_ms);
    }
    check_invariants(network, model, profile);
}

fn run_walk(
    rng: &mut XorShift64,
    profile: &NetworkProfile,
    steps: usize,
    observed: &mut WalkCoverage,
) {
    let mut network = VirtualNetwork::new(*profile).expect("profile validates");
    let mut model = NetworkModel::default();
    let mut tracker = DeliveryTracker::default();
    let mut now_ms = 0_u64;
    let mut next_id = 0_u64;

    for _ in 0..steps {
        // Advance the monotonic clock; keep a back-to-back arm so shaping and
        // capacity pressure see simultaneous submissions.
        let step_ms = if rng.next_usize(4) == 0 {
            rng.next_u64(5)
        } else {
            rng.next_u64(1_000)
        };
        now_ms = now_ms.saturating_add(step_ms);

        if rng.next_usize(5) == 0 {
            drain_and_check(&mut network, &mut model, &mut tracker, profile, now_ms);
        }

        let id = next_id;
        next_id = next_id.wrapping_add(1);
        let payload_bytes = rng.next_u64(513);
        let outcome = network.send(now_ms, id, payload_bytes);
        let ordinal = model.submitted;
        model.on_send(profile, now_ms, payload_bytes, outcome);
        match outcome {
            SendOutcome::DroppedByLoss => {
                if is_burst_drop(ordinal, profile) {
                    observed.mark(WalkCoverage::BURST_LOSS);
                } else {
                    observed.mark(WalkCoverage::RANDOM_LOSS);
                }
            }
            SendOutcome::DroppedByCapacity => observed.mark(WalkCoverage::ORIGINAL_DROP),
            SendOutcome::Scheduled { copies } => {
                tracker.register(id, now_ms, copies);
                if copies == 2 {
                    observed.mark(WalkCoverage::DUPLICATE);
                }
                if copies == 1 && profile.duplicate_percent == 100 {
                    observed.mark(WalkCoverage::DUPLICATE_DROP);
                }
                if profile.bandwidth_bytes_per_second > 0 {
                    observed.mark(WalkCoverage::SHAPED);
                }
            }
        }
        check_invariants(&network, &model, profile);
    }

    // Exhaust the queue and confirm every scheduled copy arrives exactly once.
    drain_and_check(&mut network, &mut model, &mut tracker, profile, u64::MAX);
    assert_eq!(network.queued_copies(), 0, "full drain exhausts the queue");
    let stats = network.stats();
    assert_eq!(
        stats.delivered_copies, stats.scheduled_copies,
        "full drain delivers every scheduled copy"
    );
    tracker.assert_all_delivered();
}

/// Drives curated and randomly generated valid profiles through seeded
/// send/drain walks against the independent accounting and delivery models.
#[test]
fn randomized_send_drain_walks_match_an_independent_network_model() {
    const CURATED_STEPS: usize = 3_000;
    const RANDOM_PROFILES: usize = 12;
    const RANDOM_STEPS: usize = 800;

    let mut rng = XorShift64(0x4E47_0000_2026_0018);
    let mut observed = WalkCoverage::default();

    for profile in curated_profiles() {
        run_walk(&mut rng, &profile, CURATED_STEPS, &mut observed);
    }
    for _ in 0..RANDOM_PROFILES {
        let profile = random_profile(&mut rng);
        profile.validate().expect("generated profile must validate");
        run_walk(&mut rng, &profile, RANDOM_STEPS, &mut observed);
    }

    assert!(
        observed.covered(WalkCoverage::BURST_LOSS),
        "walk must drop packets by burst"
    );
    assert!(
        observed.covered(WalkCoverage::RANDOM_LOSS),
        "walk must drop packets by random loss"
    );
    assert!(
        observed.covered(WalkCoverage::DUPLICATE),
        "walk must admit duplicate copies"
    );
    assert!(
        observed.covered(WalkCoverage::DUPLICATE_DROP),
        "walk must reject duplicate copies by capacity"
    );
    assert!(
        observed.covered(WalkCoverage::ORIGINAL_DROP),
        "walk must drop originals by capacity"
    );
    assert!(
        observed.covered(WalkCoverage::SHAPED),
        "walk must exercise finite-link shaping"
    );
}

/// Two networks created from the same profile and driven by the same script
/// must produce identical outcomes, delivery streams, and counter snapshots.
#[test]
fn identically_seeded_networks_produce_identical_delivery_streams() {
    let mut profile = NetworkProfile::stable(0x4E47_0000_2026_0011);
    profile.loss_percent = 20;
    profile.duplicate_percent = 60;
    profile.reorder_percent = 40;
    profile.base_delay_ms = 8;
    profile.jitter_ms = 12;
    profile.reorder_delay_ms = 25;
    profile.bandwidth_bytes_per_second = 1_500;
    profile.burst_interval_packets = 6;
    profile.burst_length_packets = 2;
    profile.max_in_flight_packets = 16;

    let mut left = VirtualNetwork::new(profile).expect("profile validates");
    let mut right = VirtualNetwork::new(profile).expect("profile validates");
    let mut rng = XorShift64(0x4E47_0000_2026_0012);
    let mut now_ms = 0_u64;
    let mut next_id = 0_u64;

    for step in 0..2_000 {
        now_ms = now_ms.saturating_add(rng.next_u64(200));
        let payload_bytes = rng.next_u64(400);
        let outcome_left = left.send(now_ms, next_id, payload_bytes);
        let outcome_right = right.send(now_ms, next_id, payload_bytes);
        assert_eq!(outcome_left, outcome_right, "send outcome at step {step}");
        next_id = next_id.wrapping_add(1);
        if rng.next_usize(4) == 0 {
            let drained_left = left.drain_ready(now_ms);
            let drained_right = right.drain_ready(now_ms);
            assert_eq!(drained_left, drained_right, "drain contents at step {step}");
        }
        assert_eq!(left.stats(), right.stats(), "stats at step {step}");
        assert_eq!(
            left.queued_copies(),
            right.queued_copies(),
            "queued at step {step}"
        );
    }

    assert_eq!(
        left.drain_ready(u64::MAX),
        right.drain_ready(u64::MAX),
        "final drain contents"
    );
    assert_eq!(left.stats(), right.stats(), "final stats");
    assert_eq!(left.queued_copies(), 0, "left queue is exhausted");
    assert_eq!(right.queued_copies(), 0, "right queue is exhausted");
}

/// A delivery becomes visible exactly when the virtual clock reaches its
/// scheduled arrival time, and a full drain is exhaustive.
#[test]
fn drain_respects_the_delivery_boundary_and_full_drain_is_exhaustive() {
    let mut profile = NetworkProfile::stable(0x4E47_0000_2026_0013);
    profile.base_delay_ms = 10;
    profile.jitter_ms = 0;
    profile.reorder_delay_ms = 0;
    profile.reorder_percent = 0;
    let mut network = VirtualNetwork::new(profile).expect("profile validates");

    // Base 10 ms, zero jitter, and no reorder give an exact 10 ms delay.
    let outcome = network.send(5, 1_u64, 100);
    assert_eq!(outcome, SendOutcome::Scheduled { copies: 1 });
    assert!(network.drain_ready(14).is_empty(), "nothing before 10 ms");
    let ready = network.drain_ready(15);
    assert_eq!(ready.len(), 1, "exactly one copy at 10 ms");
    assert_eq!(ready[0].payload, 1);
    assert_eq!(ready[0].sent_at_ms, 5);
    assert_eq!(ready[0].delivered_at_ms, 15);

    let mut heavy = NetworkProfile::stable(0x4E47_0000_2026_0014);
    heavy.loss_percent = 30;
    heavy.duplicate_percent = 70;
    heavy.reorder_percent = 40;
    heavy.base_delay_ms = 20;
    heavy.jitter_ms = 30;
    heavy.reorder_delay_ms = 40;
    heavy.max_in_flight_packets = 32;
    let mut network = VirtualNetwork::new(heavy).expect("profile validates");
    let mut rng = XorShift64(0x4E47_0000_2026_0015);
    let mut now_ms = 0_u64;
    let mut next_id = 0_u64;
    for _ in 0..500 {
        now_ms = now_ms.saturating_add(rng.next_u64(50));
        network.send(now_ms, next_id, rng.next_u64(300));
        next_id = next_id.wrapping_add(1);
    }

    let scheduled = network.stats().scheduled_copies;
    let drained = network.drain_ready(u64::MAX);
    assert_eq!(
        drained.len(),
        usize::try_from(scheduled).expect("scheduled copies stay tiny"),
        "full drain returns every scheduled copy"
    );
    assert_eq!(network.queued_copies(), 0, "full drain exhausts the queue");
    let stats = network.stats();
    assert_eq!(
        stats.delivered_copies, scheduled,
        "delivered matches scheduled"
    );
    assert!(
        network.drain_ready(u64::MAX).is_empty(),
        "second full drain is empty"
    );
}
