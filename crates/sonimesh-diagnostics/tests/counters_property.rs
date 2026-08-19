//! Deterministic property coverage for the stream diagnostics counters.
//!
//! `StreamCounters` publishes four distinct state machines: fourteen
//! cumulative saturating counters, two target-gated scheduler epoch counters,
//! two packed (occupancy, high-water) epoch pairs — the PCM queue and the
//! jitter buffer — and a six-field adaptive target-state epoch. This test
//! drives a real counter instance under seeded
//! operation walks against independent models — a plain saturating `u64`
//! struct for the cumulative counters, separate `(occupancy, high_water)`
//! pairs for the epochs, and a `(min, max, target, smoothed, raised, lowered)`
//! struct for the target state — and compares every published snapshot field
//! after each step.

use sonimesh_diagnostics::{StreamCounters, StreamMetricsSnapshot};

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
}

/// Independent saturating model of the fourteen cumulative counters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CounterModel {
    authenticated: u64,
    duplicate: u64,
    rejected: u64,
    late: u64,
    reordered: u64,
    jitter_resync_dropped_frames: u64,
    plc_frames: u64,
    decode_errors: u64,
    pcm_underflows: u64,
    pcm_refill_limit_hits: u64,
    pcm_overflows: u64,
    pcm_discontinuities: u64,
    stream_resets: u64,
    drift_hard_resyncs: u64,
}

impl CounterModel {
    fn record(&mut self, counter: usize, amount: u64) {
        let slot = match counter {
            0 => &mut self.authenticated,
            1 => &mut self.duplicate,
            2 => &mut self.rejected,
            3 => &mut self.late,
            4 => &mut self.reordered,
            5 => &mut self.jitter_resync_dropped_frames,
            6 => &mut self.plc_frames,
            7 => &mut self.decode_errors,
            8 => &mut self.pcm_underflows,
            9 => &mut self.pcm_refill_limit_hits,
            10 => &mut self.pcm_overflows,
            11 => &mut self.pcm_discontinuities,
            12 => &mut self.stream_resets,
            13 => &mut self.drift_hard_resyncs,
            _ => unreachable!("counter index bounded by 14"),
        };
        *slot = slot.saturating_add(amount);
    }
}

fn assert_cumulative_snapshot(snapshot: &StreamMetricsSnapshot, model: &CounterModel) {
    assert_eq!(
        snapshot.authenticated_packets, model.authenticated,
        "authenticated"
    );
    assert_eq!(snapshot.duplicate_packets, model.duplicate, "duplicate");
    assert_eq!(snapshot.rejected_packets, model.rejected, "rejected");
    assert_eq!(snapshot.late_packets, model.late, "late");
    assert_eq!(snapshot.reordered_packets, model.reordered, "reordered");
    assert_eq!(
        snapshot.jitter_resync_dropped_frames, model.jitter_resync_dropped_frames,
        "jitter_resync_dropped_frames"
    );
    assert_eq!(snapshot.plc_frames, model.plc_frames, "plc_frames");
    assert_eq!(snapshot.decode_errors, model.decode_errors, "decode_errors");
    assert_eq!(
        snapshot.pcm_underflows, model.pcm_underflows,
        "pcm_underflows"
    );
    assert_eq!(
        snapshot.pcm_refill_limit_hits, model.pcm_refill_limit_hits,
        "pcm_refill_limit_hits"
    );
    assert_eq!(snapshot.pcm_overflows, model.pcm_overflows, "pcm_overflows");
    assert_eq!(
        snapshot.pcm_discontinuities, model.pcm_discontinuities,
        "pcm_discontinuities"
    );
    assert_eq!(snapshot.stream_resets, model.stream_resets, "stream_resets");
    assert_eq!(
        snapshot.drift_hard_resyncs, model.drift_hard_resyncs,
        "drift_hard_resyncs"
    );
}

fn apply_increments(counters: &StreamCounters, counter: usize, count: u64) {
    if counter == 5 {
        counters.jitter_resync_dropped_frames(
            usize::try_from(count).expect("unit increment count stays tiny"),
        );
        return;
    }
    for _ in 0..count {
        match counter {
            0 => counters.authenticated(),
            1 => counters.duplicate(),
            2 => counters.rejected(),
            3 => counters.late(),
            4 => counters.reordered(),
            6 => counters.plc_frame(),
            7 => counters.decode_error(),
            8 => counters.pcm_underflow(),
            9 => counters.pcm_refill_limit_hit(),
            10 => counters.pcm_overflow(),
            11 => counters.pcm_discontinuity(),
            12 => counters.stream_reset(),
            13 => counters.drift_hard_resync(),
            _ => unreachable!("counter index bounded by 14"),
        }
    }
}

/// One randomized counter write.
enum CounterWrite {
    /// Applies `count` unit increments to the indexed cumulative counter.
    Increment { counter: usize, count: u64 },
    /// Records an arbitrary frame count into the jitter resync counter.
    JitterFrames { frames: usize },
}

/// Drives every cumulative counter under interleaved random writes against an
/// independent saturating model. Unit counters are incremented one at a time
/// while the jitter counter accepts arbitrary frame counts, so the walk
/// exercises both the counter wiring and the saturating-add path at `u64::MAX`.
#[test]
fn randomized_counter_writes_match_an_independent_saturating_model() {
    const STEPS: usize = 20_000;
    let mut rng = XorShift64(0xD1A6_0000_2026_0014);
    let counters = StreamCounters::default();
    let mut model = CounterModel::default();

    let mut observed_jitter_saturation = false;

    for _ in 0..STEPS {
        let write = if rng.next_usize(4) == 0 {
            CounterWrite::JitterFrames {
                frames: match rng.next_usize(4) {
                    0 => 0,
                    1 => 3,
                    2 => 10_000,
                    _ => usize::MAX,
                },
            }
        } else {
            CounterWrite::Increment {
                counter: rng.next_usize(14),
                count: u64::try_from(rng.next_usize(4)).expect("bounded by 4"),
            }
        };

        match write {
            CounterWrite::Increment { counter, count } => {
                apply_increments(&counters, counter, count);
                model.record(counter, count);
            }
            CounterWrite::JitterFrames { frames } => {
                // Mirror the counter's saturating usize -> u64 conversion.
                let amount = u64::try_from(frames).unwrap_or(u64::MAX);
                counters.jitter_resync_dropped_frames(frames);
                model.record(5, amount);
            }
        }

        assert_cumulative_snapshot(&counters.snapshot(), &model);
        if model.jitter_resync_dropped_frames == u64::MAX {
            observed_jitter_saturation = true;
        }
    }

    assert!(
        observed_jitter_saturation,
        "the walk must drive the jitter counter into saturation"
    );
    assert_eq!(model.jitter_resync_dropped_frames, u64::MAX);
}

/// Independent model of the packed PCM queue epoch.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct QueueEpochModel {
    blocks: u16,
    high_water: u16,
}

/// Drives `set_queued_pcm_blocks` and `reset_queued_pcm_epoch` against an
/// independent `(blocks, high_water)` model: the occupancy must reflect the
/// latest published value, the high-water mark must never decrease within an
/// epoch, and a reset must retire both to zero.
#[test]
fn queued_pcm_epoch_matches_an_independent_high_water_model() {
    const STEPS: usize = 20_000;
    let mut rng = XorShift64(0xD1A6_0000_2026_0015);
    let counters = StreamCounters::default();
    let mut model = QueueEpochModel::default();

    let mut observed_high_water_increase = false;
    let mut observed_high_water_unchanged = false;
    let mut observed_reset_after_activity = false;

    for _ in 0..STEPS {
        if rng.next_usize(10) == 0 {
            counters.reset_queued_pcm_epoch();
            if model.blocks != 0 || model.high_water != 0 {
                observed_reset_after_activity = true;
            }
            model = QueueEpochModel::default();
        } else {
            let blocks = match rng.next_usize(5) {
                0 => 0,
                1 => u16::try_from(rng.next_u32() % 16 + 1).expect("small occupancy"),
                2 => u16::try_from(rng.next_u32() % 65_536).expect("full-range occupancy"),
                3 => u16::MAX,
                _ => u16::try_from(rng.next_u32() % 1_024).expect("medium occupancy"),
            };
            let previous_high_water = model.high_water;
            counters.set_queued_pcm_blocks(blocks);
            model.high_water = model.high_water.max(blocks);
            model.blocks = blocks;

            if model.high_water > previous_high_water {
                observed_high_water_increase = true;
            } else {
                observed_high_water_unchanged = true;
            }
        }

        let snapshot = counters.snapshot();
        assert_eq!(snapshot.queued_pcm_blocks, model.blocks, "queued blocks");
        assert_eq!(
            snapshot.queued_pcm_high_water_blocks, model.high_water,
            "queued high-water"
        );
        assert!(
            model.high_water >= model.blocks,
            "high-water must never fall below the active occupancy"
        );
    }

    assert!(
        observed_high_water_increase,
        "the walk must raise the high-water mark"
    );
    assert!(
        observed_high_water_unchanged,
        "the walk must publish at or below the high-water mark"
    );
    assert!(
        observed_reset_after_activity,
        "the walk must reset a populated epoch"
    );
}

/// Independent model of the packed jitter-buffer epoch.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct JitterEpochModel {
    frames: u16,
    high_water: u16,
}

/// Drives `set_jitter_frames` and `reset_jitter_epoch` against an independent
/// `(frames, high_water)` model: the occupancy must reflect the latest
/// published value, the high-water mark must never decrease within an epoch,
/// and a reset must retire both to zero.
#[test]
fn jitter_epoch_matches_an_independent_high_water_model() {
    const STEPS: usize = 20_000;
    let mut rng = XorShift64(0xD1A6_0000_2026_0016);
    let counters = StreamCounters::default();
    let mut model = JitterEpochModel::default();

    let mut observed_high_water_increase = false;
    let mut observed_high_water_unchanged = false;
    let mut observed_reset_after_activity = false;

    for _ in 0..STEPS {
        if rng.next_usize(10) == 0 {
            counters.reset_jitter_epoch();
            if model.frames != 0 || model.high_water != 0 {
                observed_reset_after_activity = true;
            }
            model = JitterEpochModel::default();
        } else {
            let frames = match rng.next_usize(5) {
                0 => 0,
                1 => u16::try_from(rng.next_u32() % 16 + 1).expect("small occupancy"),
                2 => u16::try_from(rng.next_u32() % 65_536).expect("full-range occupancy"),
                3 => u16::MAX,
                _ => u16::try_from(rng.next_u32() % 1_024).expect("medium occupancy"),
            };
            let previous_high_water = model.high_water;
            counters.set_jitter_frames(frames);
            model.high_water = model.high_water.max(frames);
            model.frames = frames;

            if model.high_water > previous_high_water {
                observed_high_water_increase = true;
            } else {
                observed_high_water_unchanged = true;
            }
        }

        let snapshot = counters.snapshot();
        assert_eq!(snapshot.jitter_frames, model.frames, "jitter frames");
        assert_eq!(
            snapshot.jitter_high_water_frames, model.high_water,
            "jitter high-water"
        );
        assert!(
            model.high_water >= model.frames,
            "high-water must never fall below the active occupancy"
        );
    }

    assert!(
        observed_high_water_increase,
        "the walk must raise the jitter high-water mark"
    );
    assert!(
        observed_high_water_unchanged,
        "the walk must publish at or below the jitter high-water mark"
    );
    assert!(
        observed_reset_after_activity,
        "the walk must reset a populated jitter epoch"
    );
}

/// Independent model of the adaptive-target state epoch.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct TargetEpochModel {
    min_frames: u16,
    max_frames: u16,
    target_frames: u16,
    smoothed_jitter_ms: u32,
    raised_steps: u32,
    lowered_steps: u32,
}

/// Drives `publish_target_state` and `reset_target_state` against an
/// independent six-field model: every published value must match the latest
/// write, the minimum must never exceed the maximum, the target must stay
/// inside its water levels, and a reset must retire all six fields to zero.
#[allow(clippy::too_many_lines)]
#[test]
fn target_epoch_matches_an_independent_model() {
    const STEPS: usize = 20_000;
    let mut rng = XorShift64(0xD1A6_0000_2026_0017);
    let counters = StreamCounters::default();
    let mut model = TargetEpochModel::default();

    let mut observed_published_state = false;
    let mut observed_min_below_max = false;
    let mut observed_reset_after_activity = false;

    for _ in 0..STEPS {
        if rng.next_usize(10) == 0 {
            counters.reset_target_state();
            if model != TargetEpochModel::default() {
                observed_reset_after_activity = true;
            }
            model = TargetEpochModel::default();
        } else {
            let min_frames = match rng.next_usize(4) {
                0 => 0,
                1 => u16::try_from(rng.next_u32() % 16 + 1).expect("small bound"),
                2 => u16::try_from(rng.next_u32() % 65_536).expect("full-range bound"),
                _ => u16::try_from(rng.next_u32() % 1_024).expect("medium bound"),
            };
            let span = match rng.next_usize(4) {
                0 => 0,
                1 => u16::try_from(rng.next_u32() % 16 + 1).expect("small span"),
                2 => u16::MAX - min_frames,
                _ => u16::try_from(rng.next_u32() % 1_024).expect("medium span"),
            };
            let max_frames = min_frames.saturating_add(span);
            let target_frames = if max_frames == min_frames {
                min_frames
            } else {
                let offset = match rng.next_usize(4) {
                    0 => 0,
                    1 => max_frames - min_frames,
                    2 => (max_frames - min_frames) / 2,
                    _ => u16::try_from(rng.next_u32() % (u32::from(max_frames - min_frames) + 1))
                        .expect("bounded offset"),
                };
                min_frames + offset
            };
            let smoothed_jitter_ms = match rng.next_usize(4) {
                0 => 0,
                1 => rng.next_u32() % 128,
                2 => rng.next_u32() % 65_536,
                _ => u32::MAX,
            };
            let raised_steps = match rng.next_usize(4) {
                0 => 0,
                1 => rng.next_u32() % 16,
                2 => rng.next_u32() % 1_024,
                _ => u32::MAX,
            };
            let lowered_steps = match rng.next_usize(4) {
                0 => 0,
                1 => rng.next_u32() % 16,
                2 => rng.next_u32() % 1_024,
                _ => u32::MAX,
            };

            counters.publish_target_state(
                min_frames,
                max_frames,
                target_frames,
                smoothed_jitter_ms,
                raised_steps,
                lowered_steps,
            );
            model = TargetEpochModel {
                min_frames,
                max_frames,
                target_frames,
                smoothed_jitter_ms,
                raised_steps,
                lowered_steps,
            };
            if model != TargetEpochModel::default() {
                observed_published_state = true;
            }
            if min_frames < max_frames {
                observed_min_below_max = true;
            }
        }

        let snapshot = counters.snapshot();
        assert_eq!(snapshot.target_min_frames, model.min_frames, "target min");
        assert_eq!(snapshot.target_max_frames, model.max_frames, "target max");
        assert_eq!(snapshot.target_frames, model.target_frames, "target frames");
        assert_eq!(
            snapshot.target_smoothed_jitter_ms, model.smoothed_jitter_ms,
            "target smoothed jitter"
        );
        assert_eq!(
            snapshot.target_raised_steps, model.raised_steps,
            "target raised"
        );
        assert_eq!(
            snapshot.target_lowered_steps, model.lowered_steps,
            "target lowered"
        );
        assert!(
            model.min_frames <= model.max_frames,
            "generated minimum must never exceed the maximum"
        );
        assert!(
            model.min_frames <= model.target_frames && model.target_frames <= model.max_frames,
            "the target must stay inside its water levels"
        );
    }

    assert!(
        observed_published_state,
        "the walk must publish a populated target state"
    );
    assert!(
        observed_min_below_max,
        "the walk must publish a strict min/max span"
    );
    assert!(
        observed_reset_after_activity,
        "the walk must reset a populated target epoch"
    );
}

/// Drives the target-gated playout telemetry epoch independently from the
/// adaptive water-level epoch. Both counters are exported as decimal values,
/// and an explicit scheduler reset must retire them without touching the
/// adaptive target state.
#[test]
fn target_gated_playout_epoch_matches_an_independent_model() {
    const STEPS: usize = 20_000;
    let mut rng = XorShift64(0xD1A6_0000_2026_0018);
    let counters = StreamCounters::default();
    let mut model = (0_u64, 0_u64);
    let mut observed_activity = false;
    let mut observed_reset_after_activity = false;

    for _ in 0..STEPS {
        if rng.next_usize(10) == 0 {
            counters.reset_target_gated_playout_state();
            if model != (0, 0) {
                observed_reset_after_activity = true;
            }
            model = (0, 0);
        } else {
            let gated_ticks = match rng.next_usize(4) {
                0 => 0,
                1 => u64::from(rng.next_u32() % 16),
                2 => u64::from(rng.next_u32()),
                _ => u64::MAX,
            };
            let forced_playouts = match rng.next_usize(4) {
                0 => 0,
                1 => u64::from(rng.next_u32() % 16),
                2 => u64::from(rng.next_u32()),
                _ => u64::MAX,
            };
            counters.publish_target_gated_playout_state(gated_ticks, forced_playouts);
            model = (gated_ticks, forced_playouts);
            observed_activity |= model != (0, 0);
        }

        let snapshot = counters.snapshot();
        assert_eq!(
            (snapshot.target_gated_ticks, snapshot.target_forced_playouts),
            model
        );
    }

    assert!(
        observed_activity,
        "the walk must publish scheduler telemetry"
    );
    assert!(
        observed_reset_after_activity,
        "the walk must retire a populated scheduler epoch"
    );
}
