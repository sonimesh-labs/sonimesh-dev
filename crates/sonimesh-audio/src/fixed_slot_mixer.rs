//! Allocation-free fixed-slot realtime mixer.
//!
//! This module implements the Phase 2 stream-membership boundary recorded in
//! ADR 0015: a preallocated slot table that the output callback iterates in
//! fixed order, with inactive slots rendering silence. Membership transitions
//! (activate, deactivate, replace) are control-domain work that never allocates
//! and never takes a lock; the callback only calls [`FixedSlotMixer::render`],
//! which visits every slot in fixed order and does bounded work per slot.

use std::fmt;
use std::sync::Arc;

use sonimesh_diagnostics::StreamCounters;

use crate::gain::GainSmoother;
use crate::gain_control::GainControlHandle;
use crate::realtime_queue::{MAX_PCM_QUEUE_BLOCKS, RealtimePcmConsumer, RealtimePcmPopOutcome};

/// Recommended slot count: headroom above the roadmap's four-source maximum.
///
/// The mixer itself takes the slot count as a construction-time constant; this
/// is the value the Phase 2 control domain should use.
pub const DEFAULT_MIXER_SLOTS: usize = 8;

/// Samples used for the per-slot discontinuity fade.
const DISCONTINUITY_FADE_SAMPLES: u16 = 128;

/// Samples used for a per-slot gain or mute ramp.
const GAIN_RAMP_SAMPLES: u16 = 128;

/// Failure while constructing a [`FixedSlotMixer`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MixerConfigError {
    /// The slot count must be at least one.
    ZeroSlots,
    /// Each slot's PCM block must hold at least one sample.
    ZeroSamplesPerBlock,
}

impl fmt::Display for MixerConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::ZeroSlots => "mixer slot count must be non-zero",
            Self::ZeroSamplesPerBlock => "mixer PCM blocks must hold at least one sample",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for MixerConfigError {}

/// Failure while changing fixed-slot membership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MixerError {
    /// Every slot is occupied; admission failed closed.
    SlotFull,
    /// The slot index is outside the fixed slot table.
    SlotOutOfRange,
    /// The slot index is valid but the slot is currently inactive.
    SlotInactive,
}

impl fmt::Display for MixerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::SlotFull => "every mixer slot is occupied",
            Self::SlotOutOfRange => "mixer slot index is out of range",
            Self::SlotInactive => "mixer slot is not active",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for MixerError {}

/// One preallocated stream slot: its consumer endpoint, block scratch,
/// per-slot discontinuity fade, per-slot gain and mute state, and optional
/// per-slot realtime counters that the callback records without locking.
struct Slot<const SAMPLES_PER_BLOCK: usize> {
    consumer: RealtimePcmConsumer<SAMPLES_PER_BLOCK>,
    scratch: [f32; SAMPLES_PER_BLOCK],
    scratch_len: usize,
    scratch_offset: usize,
    fade: GainSmoother,
    gain_handle: GainControlHandle,
    gain_smoother: GainSmoother,
    counters: Option<Arc<StreamCounters>>,
}

impl<const SAMPLES_PER_BLOCK: usize> Slot<SAMPLES_PER_BLOCK> {
    fn new(
        consumer: RealtimePcmConsumer<SAMPLES_PER_BLOCK>,
        gain_handle: GainControlHandle,
        counters: Option<Arc<StreamCounters>>,
    ) -> Self {
        Self {
            consumer,
            scratch: [0.0; SAMPLES_PER_BLOCK],
            scratch_len: 0,
            scratch_offset: 0,
            fade: GainSmoother::new(1.0),
            gain_handle,
            gain_smoother: GainSmoother::new(1.0),
            counters,
        }
    }

    /// Accumulates this slot's next samples into `target`.
    ///
    /// Underflow and the bounded per-callback refill limit leave the remaining
    /// target samples untouched, so this slot contributes bounded silence
    /// without affecting any other slot. A discontinuity resets this slot's
    /// fade to zero and ramps it back to unity. When the slot carries a
    /// counters endpoint, underflow and discontinuity events are recorded
    /// here; the refill-limit event is recorded once per callback by
    /// [`FixedSlotMixer::render`] when this method reports exhaustion.
    ///
    /// Returns `true` when the shared per-callback refill budget ran out while
    /// this slot still needed samples.
    fn render_into(&mut self, target: &mut [f32], refill_budget: &mut usize) -> bool {
        let mut written = 0;
        while written < target.len() {
            if self.scratch_offset == self.scratch_len {
                if *refill_budget == 0 {
                    return true;
                }
                *refill_budget -= 1;
                match self.consumer.pop_into(&mut self.scratch) {
                    RealtimePcmPopOutcome::Filled {
                        samples,
                        discontinuity,
                        ..
                    } => {
                        if discontinuity {
                            self.record_discontinuity();
                            self.fade = GainSmoother::new(0.0);
                            self.fade.set_target(1.0, DISCONTINUITY_FADE_SAMPLES);
                        }
                        self.scratch_len = samples;
                        self.scratch_offset = 0;
                        if samples == 0 {
                            continue;
                        }
                    }
                    RealtimePcmPopOutcome::Underflow { .. } => {
                        self.record_underflow();
                        break;
                    }
                }
            }

            let available = self.scratch_len - self.scratch_offset;
            let copy_len = available.min(target.len() - written);
            let chunk = &mut self.scratch[self.scratch_offset..self.scratch_offset + copy_len];
            self.fade.apply(chunk);
            apply_slot_gain(&self.gain_handle, &mut self.gain_smoother, chunk);
            for (target_sample, scratch_sample) in target[written..written + copy_len]
                .iter_mut()
                .zip(chunk.iter())
            {
                *target_sample += *scratch_sample;
            }
            written += copy_len;
            self.scratch_offset += copy_len;
        }
        false
    }
}

impl<const SAMPLES_PER_BLOCK: usize> Slot<SAMPLES_PER_BLOCK> {
    fn record_underflow(&self) {
        if let Some(counters) = &self.counters {
            counters.pcm_underflow();
        }
    }

    fn record_refill_limit_hit(&self) {
        if let Some(counters) = &self.counters {
            counters.pcm_refill_limit_hit();
        }
    }

    fn record_discontinuity(&self) {
        if let Some(counters) = &self.counters {
            counters.pcm_discontinuity();
        }
    }
}

/// Applies the published per-slot gain or mute target with a bounded block
/// ramp. The ramp restarts only when the target changes, so an unchanged
/// gain never resets a partial ramp or prevents convergence. The handle and
/// smoother come from the same slot, so the callback applies gain with
/// disjoint borrows and never takes a lock.
fn apply_slot_gain(
    gain_handle: &GainControlHandle,
    gain_smoother: &mut GainSmoother,
    samples: &mut [f32],
) {
    let state = gain_handle.snapshot();
    let target = if state.muted { 0.0 } else { state.gain };
    // Both values originate from the handle's atomic bit storage, so a
    // bit-exact comparison is the same as an exact value comparison.
    if target.to_bits() != gain_smoother.target().to_bits() {
        gain_smoother.set_target(target, GAIN_RAMP_SAMPLES);
    }
    gain_smoother.apply(samples);
}

/// Fixed preallocated slot mixer for the realtime output callback.
///
/// The mixer owns `SLOTS` fixed slots, each holding one stream's bounded PCM
/// consumer endpoint ([ADR 0002](./0002-realtime-pcm-queue.md)). The callback
/// calls [`FixedSlotMixer::render`], which zeroes the output and then, for each
/// output chunk, visits every slot in ascending fixed order and accumulates
/// that slot's next samples. Inactive or underflowing slots contribute silence.
/// All slots share one common Float32 bus; interleaved channel layouts are
/// summed sample-wise, and the caller applies the bounded-output limiter after
/// rendering.
///
/// Membership changes ([`FixedSlotMixer::insert`], [`FixedSlotMixer::remove`])
/// are control-domain operations and must never run inside the output callback.
pub struct FixedSlotMixer<const SLOTS: usize, const SAMPLES_PER_BLOCK: usize> {
    slots: [Option<Slot<SAMPLES_PER_BLOCK>>; SLOTS],
    active: usize,
}

impl<const SLOTS: usize, const SAMPLES_PER_BLOCK: usize> FixedSlotMixer<SLOTS, SAMPLES_PER_BLOCK> {
    /// Creates an empty mixer with all slots inactive.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when `SLOTS` or `SAMPLES_PER_BLOCK` is
    /// zero.
    pub fn new() -> Result<Self, MixerConfigError> {
        if SLOTS == 0 {
            return Err(MixerConfigError::ZeroSlots);
        }
        if SAMPLES_PER_BLOCK == 0 {
            return Err(MixerConfigError::ZeroSamplesPerBlock);
        }
        Ok(Self {
            slots: std::array::from_fn(|_| None),
            active: 0,
        })
    }

    /// Activates one stream at the lowest free slot, returning its index.
    ///
    /// The supplied [`GainControlHandle`] becomes the slot's per-stream gain
    /// and mute state; the control domain keeps a clone to publish changes
    /// that the callback reads without locking. `counters` is an optional
    /// per-slot metrics endpoint that the callback records into without
    /// locking; `None` leaves the slot metric-free and otherwise identical.
    /// This is control-domain work; it never allocates and never takes a lock.
    ///
    /// # Errors
    ///
    /// Returns [`MixerError::SlotFull`] when every slot is occupied, leaving
    /// the mixer unchanged (admission fails closed at the fixed bound).
    pub fn insert(
        &mut self,
        consumer: RealtimePcmConsumer<SAMPLES_PER_BLOCK>,
        gain: GainControlHandle,
        counters: Option<Arc<StreamCounters>>,
    ) -> Result<usize, MixerError> {
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(Slot::new(consumer, gain, counters));
                self.active += 1;
                return Ok(index);
            }
        }
        Err(MixerError::SlotFull)
    }

    /// Deactivates one slot and returns its consumer endpoint.
    ///
    /// This is control-domain work; it never allocates and never takes a lock.
    ///
    /// # Errors
    ///
    /// Returns [`MixerError::SlotOutOfRange`] when `slot` is outside the fixed
    /// table, or [`MixerError::SlotInactive`] when the slot is already
    /// inactive. The mixer is unchanged in both cases.
    pub fn remove(
        &mut self,
        slot: usize,
    ) -> Result<RealtimePcmConsumer<SAMPLES_PER_BLOCK>, MixerError> {
        let entry = self.slots.get_mut(slot).ok_or(MixerError::SlotOutOfRange)?;
        let present = entry.take().ok_or(MixerError::SlotInactive)?;
        self.active -= 1;
        Ok(present.consumer)
    }

    /// Renders one mixed bus block into `output`.
    ///
    /// `output` is first zeroed, then the active slots contribute their next
    /// samples in fixed ascending order; inactive slots contribute zero. Per
    /// slot, at most [`MAX_PCM_QUEUE_BLOCKS`] blocks are refilled per render
    /// call (one output callback); a slot that exhausts that shared budget
    /// records one refill-limit event and contributes bounded silence to the
    /// rest of the callback. Worst-case callback work is `SLOTS` times a
    /// per-slot constant with no allocation and no lock. This method is the
    /// only realtime entry point.
    pub fn render(&mut self, output: &mut [f32]) {
        output.fill(0.0);
        let mut budgets = [MAX_PCM_QUEUE_BLOCKS; SLOTS];
        let mut refill_limit_reported = [false; SLOTS];
        let mut written = 0;
        while written < output.len() {
            let chunk_len = (output.len() - written).min(SAMPLES_PER_BLOCK);
            let chunk = &mut output[written..written + chunk_len];
            for (index, slot) in self.slots.iter_mut().enumerate() {
                if let Some(slot) = slot {
                    if slot.render_into(chunk, &mut budgets[index]) && !refill_limit_reported[index]
                    {
                        refill_limit_reported[index] = true;
                        slot.record_refill_limit_hit();
                    }
                }
            }
            written += chunk_len;
        }
    }

    /// Total number of slots in the fixed table.
    #[must_use]
    pub const fn slots(&self) -> usize {
        SLOTS
    }

    /// Number of currently active slots.
    #[must_use]
    pub const fn active(&self) -> usize {
        self.active
    }

    /// Returns whether the given slot is currently active.
    #[must_use]
    pub fn is_active(&self, slot: usize) -> bool {
        self.slots.get(slot).is_some_and(Option::is_some)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sonimesh_diagnostics::StreamCounters;

    use super::{DEFAULT_MIXER_SLOTS, FixedSlotMixer, MixerConfigError, MixerError};
    use crate::gain_control::GainControlHandle;
    use crate::realtime_queue::{MAX_PCM_QUEUE_BLOCKS, realtime_pcm_queue};
    use crate::{PeakLimiter, RealtimePcmProducer, RealtimePcmPushOutcome};

    const SAMPLES_PER_BLOCK: usize = 8;

    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn value_for_slot(slot: usize) -> f32 {
        (slot + 1) as f32
    }

    fn push_constant(
        producer: &mut RealtimePcmProducer<SAMPLES_PER_BLOCK>,
        value: f32,
        blocks: usize,
    ) {
        for _ in 0..blocks {
            assert_eq!(
                producer.push(&[value; SAMPLES_PER_BLOCK]),
                RealtimePcmPushOutcome::Inserted
            );
        }
    }

    #[test]
    fn new_rejects_zero_slots_and_zero_samples_per_block() {
        assert!(matches!(
            FixedSlotMixer::<0, SAMPLES_PER_BLOCK>::new(),
            Err(MixerConfigError::ZeroSlots)
        ));
        assert!(matches!(
            FixedSlotMixer::<DEFAULT_MIXER_SLOTS, 0>::new(),
            Err(MixerConfigError::ZeroSamplesPerBlock)
        ));
    }

    #[test]
    fn empty_mixer_renders_bounded_silence() {
        let mut mixer: FixedSlotMixer<DEFAULT_MIXER_SLOTS, SAMPLES_PER_BLOCK> =
            FixedSlotMixer::new().unwrap();
        assert_eq!(mixer.active(), 0);
        assert_eq!(mixer.slots(), DEFAULT_MIXER_SLOTS);
        let mut output = [0.5; 32];
        mixer.render(&mut output);
        assert!(
            output
                .iter()
                .all(|sample| sample.to_bits() == 0.0_f32.to_bits())
        );
    }

    #[test]
    fn insert_returns_the_lowest_free_slot_and_render_sums() {
        let mut mixer: FixedSlotMixer<DEFAULT_MIXER_SLOTS, SAMPLES_PER_BLOCK> =
            FixedSlotMixer::new().unwrap();
        let mut producers = Vec::new();
        for slot in 0..3 {
            let (mut producer, consumer) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(4).unwrap();
            push_constant(&mut producer, value_for_slot(slot), 2);
            producers.push(producer);
            assert_eq!(
                mixer
                    .insert(consumer, GainControlHandle::new(), None)
                    .unwrap(),
                slot
            );
        }
        assert_eq!(mixer.active(), 3);
        for (index, producer) in producers.iter_mut().enumerate() {
            assert!(mixer.is_active(index));
            push_constant(producer, value_for_slot(index), 2);
        }

        let expected = value_for_slot(0) + value_for_slot(1) + value_for_slot(2);
        let mut output = [0.0; 32];
        mixer.render(&mut output);
        assert!(
            output
                .iter()
                .all(|sample| (*sample - expected).abs() < 1e-6)
        );
    }

    #[test]
    fn insert_fails_closed_when_full() {
        let mut mixer: FixedSlotMixer<2, SAMPLES_PER_BLOCK> = FixedSlotMixer::new().unwrap();
        for _ in 0..2 {
            let (_, consumer) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(2).unwrap();
            assert!(
                mixer
                    .insert(consumer, GainControlHandle::new(), None)
                    .is_ok()
            );
        }
        let (_, consumer) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(2).unwrap();
        assert_eq!(
            mixer.insert(consumer, GainControlHandle::new(), None),
            Err(MixerError::SlotFull)
        );
        assert_eq!(mixer.active(), 2);
    }

    #[test]
    fn remove_returns_the_consumer_and_frees_the_slot() {
        let mut mixer: FixedSlotMixer<DEFAULT_MIXER_SLOTS, SAMPLES_PER_BLOCK> =
            FixedSlotMixer::new().unwrap();
        let (_, consumer) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(2).unwrap();
        let index = mixer
            .insert(consumer, GainControlHandle::new(), None)
            .unwrap();
        assert_eq!(index, 0);

        let returned = mixer.remove(index).unwrap();
        drop(returned);
        assert!(!mixer.is_active(index));
        assert_eq!(mixer.active(), 0);

        let (_, consumer) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(2).unwrap();
        let again = mixer
            .insert(consumer, GainControlHandle::new(), None)
            .unwrap();
        assert_eq!(again, index);
    }

    #[test]
    fn remove_rejects_out_of_range_and_inactive_slots_without_mutation() {
        let mut mixer: FixedSlotMixer<DEFAULT_MIXER_SLOTS, SAMPLES_PER_BLOCK> =
            FixedSlotMixer::new().unwrap();
        assert!(matches!(
            mixer.remove(DEFAULT_MIXER_SLOTS),
            Err(MixerError::SlotOutOfRange)
        ));
        assert!(matches!(mixer.remove(0), Err(MixerError::SlotInactive)));
        assert_eq!(mixer.active(), 0);
    }

    #[test]
    fn one_slot_underflow_does_not_block_other_slots() {
        let mut mixer: FixedSlotMixer<DEFAULT_MIXER_SLOTS, SAMPLES_PER_BLOCK> =
            FixedSlotMixer::new().unwrap();
        let (mut producer_a, consumer_a) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(4).unwrap();
        let (_, consumer_b) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(4).unwrap();
        let (mut producer_c, consumer_c) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(4).unwrap();
        push_constant(&mut producer_a, 1.0, 4);
        push_constant(&mut producer_c, 3.0, 4);
        mixer
            .insert(consumer_a, GainControlHandle::new(), None)
            .unwrap();
        mixer
            .insert(consumer_b, GainControlHandle::new(), None)
            .unwrap();
        mixer
            .insert(consumer_c, GainControlHandle::new(), None)
            .unwrap();

        let mut output = [0.0; 32];
        mixer.render(&mut output);
        // Slot b underflows (no queued data) and contributes silence only.
        assert!(output.iter().all(|sample| (*sample - 4.0).abs() < 1e-6));
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    #[test]
    fn discontinuity_fades_only_the_affected_slot() {
        let mut mixer: FixedSlotMixer<DEFAULT_MIXER_SLOTS, SAMPLES_PER_BLOCK> =
            FixedSlotMixer::new().unwrap();
        let (mut producer_a, consumer_a) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(4).unwrap();
        let (mut producer_b, consumer_b) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(4).unwrap();
        // Slot a starts a fresh epoch before its first push, so its consumer
        // observes a discontinuity on the first pop.
        producer_a.begin_epoch();
        push_constant(&mut producer_a, 1.0, 2);
        push_constant(&mut producer_b, 2.0, 1);
        mixer
            .insert(consumer_a, GainControlHandle::new(), None)
            .unwrap();
        mixer
            .insert(consumer_b, GainControlHandle::new(), None)
            .unwrap();

        let mut output = [0.0; SAMPLES_PER_BLOCK];
        mixer.render(&mut output);
        // Slot b contributes a steady 2.0; slot a fades from zero toward unity.
        for (index, sample) in output.iter().enumerate() {
            let expected = 2.0 + f32::from((index + 1) as u16) / 128.0;
            assert!(
                (*sample - expected).abs() < 0.02,
                "sample {index} not on the expected fade curve: {sample} vs {expected}"
            );
        }
    }

    #[test]
    fn partial_blocks_carry_across_render_calls() {
        let mut mixer: FixedSlotMixer<DEFAULT_MIXER_SLOTS, SAMPLES_PER_BLOCK> =
            FixedSlotMixer::new().unwrap();
        let (mut producer, consumer) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(4).unwrap();
        assert_eq!(producer.push(&[1.0; 6]), RealtimePcmPushOutcome::Inserted);
        assert_eq!(producer.push(&[1.0; 6]), RealtimePcmPushOutcome::Inserted);
        mixer
            .insert(consumer, GainControlHandle::new(), None)
            .unwrap();

        let mut first = [0.0; 10];
        mixer.render(&mut first);
        assert!(first.iter().all(|sample| (*sample - 1.0).abs() < 1e-6));

        let mut second = [0.0; 2];
        mixer.render(&mut second);
        assert!(second.iter().all(|sample| (*sample - 1.0).abs() < 1e-6));
    }

    #[test]
    fn mixed_bus_passes_through_the_limiter_without_exceeding_the_ceiling() {
        let mut mixer: FixedSlotMixer<DEFAULT_MIXER_SLOTS, SAMPLES_PER_BLOCK> =
            FixedSlotMixer::new().unwrap();
        for _ in 0..DEFAULT_MIXER_SLOTS {
            let (mut producer, consumer) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(4).unwrap();
            push_constant(&mut producer, 0.3, 4);
            mixer
                .insert(consumer, GainControlHandle::new(), None)
                .unwrap();
        }

        let mut output = [0.0; 32];
        mixer.render(&mut output);
        // Unbounded sum: 8 * 0.3 = 2.4 exceeds full scale.
        assert!(output.iter().all(|sample| (*sample - 2.4).abs() < 1e-6));

        let mut limiter = PeakLimiter::default();
        limiter.apply_interleaved(&mut output, 2).unwrap();
        assert!(
            output
                .iter()
                .all(|sample| sample.is_finite() && sample.abs() <= 1.0 + 1e-6)
        );
        assert!(output.iter().all(|sample| (*sample - 1.0).abs() < 1e-6));
    }

    #[test]
    fn per_slot_gain_scales_only_that_slot() {
        let mut mixer: FixedSlotMixer<DEFAULT_MIXER_SLOTS, SAMPLES_PER_BLOCK> =
            FixedSlotMixer::new().unwrap();
        let (mut producer_a, consumer_a) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(24).unwrap();
        let (mut producer_b, consumer_b) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(24).unwrap();
        push_constant(&mut producer_a, 1.0, 24);
        push_constant(&mut producer_b, 1.0, 24);
        let gain_a = GainControlHandle::new();
        gain_a.set_gain(0.25).unwrap();
        mixer.insert(consumer_a, gain_a.clone(), None).unwrap();
        mixer
            .insert(consumer_b, GainControlHandle::new(), None)
            .unwrap();

        let mut output = [0.0; 24 * SAMPLES_PER_BLOCK];
        mixer.render(&mut output);
        // The first samples ride the bounded ramp; after it completes the mix
        // holds at 0.25 (slot a) plus 1.0 (slot b).
        for sample in &output[128..] {
            assert!((*sample - 1.25).abs() < 1e-5);
        }
        assert!(
            output[0] > 1.9,
            "ramp start should be near unity: {}",
            output[0]
        );
    }

    #[test]
    fn per_slot_mute_silences_only_that_slot() {
        let mut mixer: FixedSlotMixer<DEFAULT_MIXER_SLOTS, SAMPLES_PER_BLOCK> =
            FixedSlotMixer::new().unwrap();
        let (mut producer_a, consumer_a) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(24).unwrap();
        let (mut producer_b, consumer_b) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(24).unwrap();
        push_constant(&mut producer_a, 1.0, 24);
        push_constant(&mut producer_b, 1.0, 24);
        let gain_a = GainControlHandle::new();
        gain_a.set_muted(true);
        mixer.insert(consumer_a, gain_a.clone(), None).unwrap();
        mixer
            .insert(consumer_b, GainControlHandle::new(), None)
            .unwrap();

        let mut output = [0.0; 24 * SAMPLES_PER_BLOCK];
        mixer.render(&mut output);
        // Slot a ramps to silence; slot b keeps its full 1.0 contribution.
        for sample in &output[128..] {
            assert!((*sample - 1.0).abs() < 1e-5);
        }
        assert!(
            output[0] > 1.9,
            "mute ramp start should be near unity: {}",
            output[0]
        );
    }

    #[test]
    fn slot_underflow_records_counters_only_for_that_slot() {
        let mut mixer: FixedSlotMixer<DEFAULT_MIXER_SLOTS, SAMPLES_PER_BLOCK> =
            FixedSlotMixer::new().unwrap();
        let (mut producer_a, consumer_a) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(4).unwrap();
        let (_, consumer_b) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(4).unwrap();
        push_constant(&mut producer_a, 1.0, 2);
        let counters_a = Arc::new(StreamCounters::default());
        let counters_b = Arc::new(StreamCounters::default());
        mixer
            .insert(
                consumer_a,
                GainControlHandle::new(),
                Some(Arc::clone(&counters_a)),
            )
            .unwrap();
        mixer
            .insert(
                consumer_b,
                GainControlHandle::new(),
                Some(Arc::clone(&counters_b)),
            )
            .unwrap();

        let mut output = [0.0; 32];
        mixer.render(&mut output);
        // Slot a drains its two blocks and then underflows on the final two
        // chunks; empty slot b underflows on every chunk. Each underflow event
        // lands only on its own slot.
        assert!(
            output[..16]
                .iter()
                .all(|sample| (*sample - 1.0).abs() < 1e-6)
        );
        assert!(output[16..].iter().all(|sample| sample.abs() < 1e-6));
        let snapshot_a = counters_a.snapshot();
        assert_eq!(snapshot_a.pcm_underflows, 2);
        assert_eq!(snapshot_a.pcm_refill_limit_hits, 0);
        assert_eq!(snapshot_a.pcm_discontinuities, 0);
        let snapshot_b = counters_b.snapshot();
        assert_eq!(snapshot_b.pcm_underflows, 4);
        assert_eq!(snapshot_b.pcm_refill_limit_hits, 0);
        assert_eq!(snapshot_b.pcm_discontinuities, 0);
    }

    #[test]
    fn slot_refill_limit_records_a_distinct_callback_event() {
        const WIDE_BLOCK: usize = 256;
        let mut mixer: FixedSlotMixer<DEFAULT_MIXER_SLOTS, WIDE_BLOCK> =
            FixedSlotMixer::new().unwrap();
        let (mut producer_a, consumer_a) =
            realtime_pcm_queue::<WIDE_BLOCK>(MAX_PCM_QUEUE_BLOCKS).unwrap();
        for _ in 0..MAX_PCM_QUEUE_BLOCKS {
            assert_eq!(producer_a.push(&[1.0]), RealtimePcmPushOutcome::Inserted);
        }
        let (_, consumer_b) = realtime_pcm_queue::<WIDE_BLOCK>(2).unwrap();
        let counters_a = Arc::new(StreamCounters::default());
        let counters_b = Arc::new(StreamCounters::default());
        mixer
            .insert(
                consumer_a,
                GainControlHandle::new(),
                Some(Arc::clone(&counters_a)),
            )
            .unwrap();
        mixer
            .insert(
                consumer_b,
                GainControlHandle::new(),
                Some(Arc::clone(&counters_b)),
            )
            .unwrap();

        let mut output = [0.0; MAX_PCM_QUEUE_BLOCKS + 1];
        mixer.render(&mut output);
        // One wide output chunk spends the full per-callback refill budget on
        // one-sample blocks, so slot a records the distinct limit event while
        // empty slot b only underflows.
        assert!(
            output[..MAX_PCM_QUEUE_BLOCKS]
                .iter()
                .all(|sample| (*sample - 1.0).abs() < 1e-6)
        );
        assert!(output[MAX_PCM_QUEUE_BLOCKS].abs() < 1e-6);
        let snapshot_a = counters_a.snapshot();
        assert_eq!(snapshot_a.pcm_refill_limit_hits, 1);
        assert_eq!(snapshot_a.pcm_underflows, 0);
        let snapshot_b = counters_b.snapshot();
        assert_eq!(snapshot_b.pcm_refill_limit_hits, 0);
        assert_eq!(snapshot_b.pcm_underflows, 1);
    }

    #[test]
    fn refill_limit_is_one_event_per_callback() {
        let mut mixer: FixedSlotMixer<1, 1> = FixedSlotMixer::new().unwrap();
        let (mut producer, consumer) = realtime_pcm_queue::<1>(MAX_PCM_QUEUE_BLOCKS).unwrap();
        for _ in 0..MAX_PCM_QUEUE_BLOCKS {
            assert_eq!(producer.push(&[1.0]), RealtimePcmPushOutcome::Inserted);
        }
        let counters = Arc::new(StreamCounters::default());
        mixer
            .insert(
                consumer,
                GainControlHandle::new(),
                Some(Arc::clone(&counters)),
            )
            .unwrap();

        // One callback spans MAX_PCM_QUEUE_BLOCKS + 1 one-sample chunks. The
        // shared per-callback refill budget is spent on the 128 queued blocks,
        // so the final chunk records the limit event exactly once and emits
        // silence instead of a second underflow.
        let mut output = [0.0; MAX_PCM_QUEUE_BLOCKS + 1];
        mixer.render(&mut output);
        assert!(
            output[..MAX_PCM_QUEUE_BLOCKS]
                .iter()
                .all(|sample| (*sample - 1.0).abs() < 1e-6)
        );
        assert!(output[MAX_PCM_QUEUE_BLOCKS].abs() < 1e-6);
        let snapshot = counters.snapshot();
        assert_eq!(snapshot.pcm_refill_limit_hits, 1);
        assert_eq!(snapshot.pcm_underflows, 0);
    }

    #[test]
    fn slot_discontinuity_records_counters_only_for_that_slot() {
        let mut mixer: FixedSlotMixer<DEFAULT_MIXER_SLOTS, SAMPLES_PER_BLOCK> =
            FixedSlotMixer::new().unwrap();
        let (mut producer_a, consumer_a) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(4).unwrap();
        let (mut producer_b, consumer_b) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(4).unwrap();
        producer_a.begin_epoch();
        push_constant(&mut producer_a, 1.0, 2);
        push_constant(&mut producer_b, 2.0, 1);
        let counters_a = Arc::new(StreamCounters::default());
        let counters_b = Arc::new(StreamCounters::default());
        mixer
            .insert(
                consumer_a,
                GainControlHandle::new(),
                Some(Arc::clone(&counters_a)),
            )
            .unwrap();
        mixer
            .insert(
                consumer_b,
                GainControlHandle::new(),
                Some(Arc::clone(&counters_b)),
            )
            .unwrap();

        let mut output = [0.0; SAMPLES_PER_BLOCK];
        mixer.render(&mut output);
        // Slot a starts a fresh epoch, so only its first pop reports a
        // discontinuity; slot b's stream is continuous and neither slot
        // underflows within this block.
        assert_eq!(counters_a.snapshot().pcm_discontinuities, 1);
        assert_eq!(counters_b.snapshot().pcm_discontinuities, 0);
        assert_eq!(counters_a.snapshot().pcm_underflows, 0);
        assert_eq!(counters_b.snapshot().pcm_underflows, 0);
    }
}
