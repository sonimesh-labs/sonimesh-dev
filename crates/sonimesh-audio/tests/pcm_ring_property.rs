//! Deterministic property coverage for the preallocated PCM block ring.
//!
//! The ring is the realtime PCM transfer boundary between the decode worker
//! and the output source: pushes must keep FIFO order, overflow must drop the
//! oldest block, and a pop must always return initialized samples while
//! consuming the whole front block. This test drives a fixed pseudo-random
//! interleaving of pushes and pops against an independent `VecDeque` model and
//! asserts every outcome, every output buffer, and the occupied count agree.

use std::collections::VecDeque;

use sonimesh_audio::{PcmBlockRing, PcmPopOutcome, PcmPushOutcome};

const CAPACITY: usize = 4;
const SAMPLES_PER_BLOCK: usize = 4;

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

/// Independent reference model using an ordinary FIFO, so the fixed ring index
/// arithmetic of the implementation cannot be replicated by construction.
#[derive(Default)]
struct RingModel {
    blocks: VecDeque<Vec<f32>>,
}

impl RingModel {
    fn push(&mut self, samples: &[f32]) -> PcmPushOutcome {
        if samples.is_empty() {
            return PcmPushOutcome::EmptyBlock;
        }
        if samples.len() > SAMPLES_PER_BLOCK {
            return PcmPushOutcome::BlockTooLarge;
        }
        let outcome = if self.blocks.len() == CAPACITY {
            self.blocks.pop_front();
            PcmPushOutcome::DroppedOldest
        } else {
            PcmPushOutcome::Inserted
        };
        self.blocks.push_back(samples.to_vec());
        outcome
    }

    fn pop_into(&mut self, output: &mut [f32]) -> PcmPopOutcome {
        output.fill(0.0);
        let Some(front) = self.blocks.pop_front() else {
            return PcmPopOutcome::Underflow;
        };
        let samples = front.len().min(output.len());
        output[..samples].copy_from_slice(&front[..samples]);
        PcmPopOutcome::Filled { samples }
    }
}

/// Test sample values are tiny; the cast cannot lose precision here.
/// `ring`/`reference`/`actual`/`expected` are short local names, not a design
/// smell, so the noisy `similar_names` lint is allowed here.
#[allow(clippy::cast_precision_loss, clippy::similar_names)]
#[test]
fn push_pop_interleavings_match_a_fifo_model() {
    let mut rng = XorShift64(0xC0FF_EE00_2026_0001);
    let mut ring = PcmBlockRing::<CAPACITY, SAMPLES_PER_BLOCK>::new();
    let mut reference = RingModel::default();

    for _ in 0..100_000 {
        if rng.next_u32() % 2 == 0 {
            let len = rng.next_usize(SAMPLES_PER_BLOCK + 2);
            let samples: Vec<f32> = (0..len).map(|i| i as f32).collect();
            let expected_outcome = reference.push(&samples);
            let actual_outcome = ring.push(&samples);
            assert_eq!(actual_outcome, expected_outcome, "push of len {len}");
        } else {
            let out_len = 1 + rng.next_usize(SAMPLES_PER_BLOCK * 2);
            let mut actual = vec![0.0; out_len];
            let mut expected = vec![0.0; out_len];
            let actual_outcome = ring.pop_into(&mut actual);
            let expected_outcome = reference.pop_into(&mut expected);
            assert_eq!(actual_outcome, expected_outcome, "pop of len {out_len}");
            assert_eq!(actual, expected, "pop content of len {out_len}");
        }
        assert_eq!(ring.len(), reference.blocks.len(), "occupied count drift");
    }
}
