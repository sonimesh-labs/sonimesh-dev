//! Deterministic property coverage for the fixed-slot realtime mixer.
//!
//! ADR 0015 requires membership transitions (activate, deactivate, replace)
//! to never allocate or lock and to keep rendering correct: the callback
//! visits every slot in fixed order and inactive slots render silence. This
//! test drives a fixed pseudo-random sequence of insert/remove/render
//! operations against an independent active-set model and asserts the mixed
//! output, the observable membership state, and the per-slot realtime counter
//! events agree at every step. A second test composes the mixer with the
//! bounded-output limiter, which is the Phase 2 output stage.

use std::sync::Arc;

use sonimesh_audio::{
    FixedSlotMixer, RealtimePcmProducer, RealtimePcmPushOutcome, realtime_pcm_queue,
};
use sonimesh_diagnostics::StreamCounters;

const SLOTS: usize = 4;
const SAMPLES_PER_BLOCK: usize = 8;
const TOLERANCE: f32 = 1e-6;

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

/// Distinct constant value each slot contributes when fed and active.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn value_for_slot(slot: usize) -> f32 {
    (slot + 1) as f32
}

/// Pushes one full block of `value` through the slot's producer.
fn push_block(producer: &mut RealtimePcmProducer<SAMPLES_PER_BLOCK>, value: f32) {
    assert_eq!(
        producer.push(&[value; SAMPLES_PER_BLOCK]),
        RealtimePcmPushOutcome::Inserted
    );
}

/// Drives random membership transitions against an independent active-set
/// model. Renders consume a whole-block count and, half the time, one extra
/// chunk, so every active slot underflows once on that tail chunk. The model
/// tracks each slot's expected underflow events, and the per-slot counters
/// must agree with it while the mixed output stays the active-slot sum.
#[allow(clippy::similar_names, clippy::too_many_lines)]
#[test]
fn random_membership_transitions_keep_render_correct() {
    let mut rng = XorShift64(0x6A1E_0000_2026_000B);
    let mut mixer: FixedSlotMixer<SLOTS, SAMPLES_PER_BLOCK> = FixedSlotMixer::new().unwrap();
    let mut producers: [Option<RealtimePcmProducer<SAMPLES_PER_BLOCK>>; SLOTS] =
        std::array::from_fn(|_| None);
    let mut counters: [Option<Arc<StreamCounters>>; SLOTS] = std::array::from_fn(|_| None);
    let mut expected_underflows: [u64; SLOTS] = [0; SLOTS];
    let mut active: [bool; SLOTS] = [false; SLOTS];

    for step in 0..20_000 {
        match rng.next_u32() % 4 {
            0 => {
                // Activate the first free slot with a fresh stream.
                let slot = (0..SLOTS).find(|&index| !active[index]);
                if let Some(slot) = slot {
                    let (producer, consumer) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(16).unwrap();
                    let slot_counters = Arc::new(StreamCounters::default());
                    let index = mixer
                        .insert(
                            consumer,
                            sonimesh_audio::GainControlHandle::new(),
                            Some(Arc::clone(&slot_counters)),
                        )
                        .unwrap();
                    assert_eq!(index, slot, "insert should take the first free slot");
                    producers[slot] = Some(producer);
                    counters[slot] = Some(slot_counters);
                    expected_underflows[slot] = 0;
                    active[slot] = true;
                } else {
                    assert_eq!(mixer.active(), SLOTS);
                }
            }
            1 => {
                // Deactivate a random active slot and drop its endpoints.
                let candidates: Vec<usize> = (0..SLOTS).filter(|&index| active[index]).collect();
                if let Some(&slot) = candidates.get(rng.next_usize(candidates.len())) {
                    mixer.remove(slot).unwrap();
                    producers[slot] = None;
                    counters[slot] = None;
                    active[slot] = false;
                }
            }
            _ => {
                // Feed every active slot and render a whole-block output plus,
                // half the time, one over-run chunk that makes each active
                // slot underflow once.
                let blocks = 1 + rng.next_usize(3);
                let over_run = rng.next_u32() % 2 == 0;
                let mut expected: f32 = 0.0;
                for (slot, present) in active.iter().enumerate() {
                    if *present {
                        let value = value_for_slot(slot);
                        expected += value;
                        let producer = producers[slot].as_mut().unwrap();
                        for _ in 0..blocks {
                            push_block(producer, value);
                        }
                        if over_run {
                            expected_underflows[slot] += 1;
                        }
                    }
                }
                let rendered_blocks = blocks + usize::from(over_run);
                let mut output = vec![0.0; rendered_blocks * SAMPLES_PER_BLOCK];
                mixer.render(&mut output);
                let expected_blocks = blocks * SAMPLES_PER_BLOCK;
                for (index, sample) in output.iter().enumerate() {
                    let block_expected = if index < expected_blocks {
                        expected
                    } else {
                        0.0
                    };
                    assert!(
                        (*sample - block_expected).abs() <= TOLERANCE,
                        "step {step}: sample {index} = {sample}, expected {block_expected}"
                    );
                }
                for slot in 0..SLOTS {
                    if active[slot] {
                        let snapshot = counters[slot].as_ref().unwrap().snapshot();
                        assert_eq!(
                            snapshot.pcm_underflows, expected_underflows[slot],
                            "step {step}: slot {slot} underflow count diverged"
                        );
                        assert_eq!(snapshot.pcm_refill_limit_hits, 0);
                        assert_eq!(snapshot.pcm_discontinuities, 0);
                    }
                }
            }
        }

        // Membership state stays consistent with the model.
        assert_eq!(
            mixer.active(),
            active.iter().filter(|present| **present).count()
        );
        for (index, expected) in active.iter().enumerate() {
            assert_eq!(
                mixer.is_active(index),
                *expected,
                "step {step}: slot {index} membership diverged"
            );
        }
    }
}

/// Composes a full mixer with the bounded-output limiter: the raw sum exceeds
/// the ceiling, and the limiter pulls every sample back inside it.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::similar_names
)]
#[test]
fn mixed_bus_stays_within_the_limiter_ceiling() {
    let mut rng = XorShift64(0x6A1E_0000_2026_000C);
    for scenario in 0..1_000 {
        let mut mixer: FixedSlotMixer<SLOTS, SAMPLES_PER_BLOCK> = FixedSlotMixer::new().unwrap();
        let mut expected_sum: f32 = 0.0;
        for _ in 0..SLOTS {
            let value = 0.05 + (rng.next_u32() % 100) as f32 / 100.0;
            expected_sum += value;
            let (mut producer, consumer) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(4).unwrap();
            push_block(&mut producer, value);
            push_block(&mut producer, value);
            mixer
                .insert(consumer, sonimesh_audio::GainControlHandle::new(), None)
                .unwrap();
        }

        let mut output = vec![0.0; 2 * SAMPLES_PER_BLOCK];
        mixer.render(&mut output);
        assert!(
            output
                .iter()
                .all(|sample| (*sample - expected_sum).abs() <= TOLERANCE)
        );

        let mut limiter = sonimesh_audio::PeakLimiter::default();
        limiter.apply_interleaved(&mut output, 2).unwrap();
        for sample in &output {
            assert!(
                sample.is_finite() && sample.abs() <= 1.0 + TOLERANCE,
                "scenario {scenario}: limiter left sample {sample} above full scale"
            );
        }
    }
}
