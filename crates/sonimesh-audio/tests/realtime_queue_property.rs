//! Deterministic property coverage for the realtime PCM queue boundary.
//!
//! The queue is the SPSC transfer boundary between the decode worker and the
//! output source: pushes must keep FIFO order, a full queue must atomically
//! replace its oldest block, `begin_epoch` must retire queued blocks from the
//! previous stream epoch, and a pop must always initialize its whole output.
//! This test drives a fixed pseudo-random interleaving of pushes, pops, and
//! epoch resets against an independent `VecDeque` model and asserts every
//! outcome, every output buffer, and both published length counters agree.

use std::collections::VecDeque;

use sonimesh_audio::{RealtimePcmPopOutcome, RealtimePcmPushOutcome, realtime_pcm_queue};

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

/// Independent reference model using an ordinary FIFO and explicit epoch and
/// sequence counters, so the lock-free slot arithmetic of the implementation
/// cannot be replicated by construction.
struct QueueModel {
    blocks: VecDeque<(u64, u64, Vec<f32>)>,
    capacity: usize,
    epoch: u64,
    active_epoch: u64,
    next_sequence: u64,
    expected: (u64, u64),
}

impl QueueModel {
    fn new(capacity: usize) -> Self {
        Self {
            blocks: VecDeque::new(),
            capacity,
            epoch: 0,
            active_epoch: 0,
            next_sequence: 0,
            expected: (0, 0),
        }
    }

    fn push(&mut self, samples: &[f32]) -> RealtimePcmPushOutcome {
        if samples.is_empty() {
            return RealtimePcmPushOutcome::EmptyBlock;
        }
        if samples.len() > SAMPLES_PER_BLOCK {
            return RealtimePcmPushOutcome::BlockTooLarge;
        }
        let outcome = if self.blocks.len() == self.capacity {
            self.blocks.pop_front();
            RealtimePcmPushOutcome::ReplacedOldest
        } else {
            RealtimePcmPushOutcome::Inserted
        };
        self.blocks
            .push_back((self.epoch, self.next_sequence, samples.to_vec()));
        self.next_sequence = self.next_sequence.wrapping_add(1);
        outcome
    }

    fn begin_epoch(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.next_sequence = 0;
        self.active_epoch = self.epoch;
    }

    fn pop_into(&mut self, output: &mut [f32]) -> RealtimePcmPopOutcome {
        output.fill(0.0);
        let mut discarded_stale_blocks = 0;
        loop {
            let Some((epoch, sequence, samples)) = self.blocks.pop_front() else {
                return RealtimePcmPopOutcome::Underflow {
                    discarded_stale_blocks,
                };
            };
            if epoch != self.active_epoch {
                discarded_stale_blocks += 1;
                continue;
            }
            let count = samples.len().min(output.len());
            output[..count].copy_from_slice(&samples[..count]);
            let actual = (epoch, sequence);
            let discontinuity = actual != self.expected;
            self.expected = (epoch, sequence.wrapping_add(1));
            return RealtimePcmPopOutcome::Filled {
                samples: count,
                sequence,
                discontinuity,
                discarded_stale_blocks,
            };
        }
    }

    fn len(&self) -> usize {
        self.blocks.len()
    }

    fn active_len(&self) -> usize {
        self.blocks
            .iter()
            .filter(|(epoch, _, _)| *epoch == self.active_epoch)
            .count()
    }
}

/// `producer`/`consumer`/`reference`/`actual`/`expected` are short local
/// names, not a design smell, so the noisy `similar_names` lint is allowed.
/// `serial` is widened to `f32` for sample content; the cast cannot lose
/// precision for the small test values used here.
#[allow(clippy::cast_precision_loss, clippy::similar_names)]
#[test]
fn interleaved_operations_match_a_fifo_epoch_model() {
    let mut rng = XorShift64(0xC0FF_EE00_2026_0002);
    let (mut producer, mut consumer) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(CAPACITY).unwrap();
    let mut reference = QueueModel::new(CAPACITY);
    let mut serial = 0_u64;

    // Push branch counters: EmptyBlock, BlockTooLarge, Inserted, ReplacedOldest.
    let mut push_counts = [0_u64; 4];
    // Pop branch counters: Filled, Underflow.
    let mut pop_counts = [0_u64; 2];
    let mut epoch_resets = 0_u64;

    for _ in 0..100_000 {
        let op = rng.next_u32() % 64;
        if op == 0 {
            producer.begin_epoch();
            reference.begin_epoch();
            epoch_resets += 1;
        } else if op < 40 {
            let len = rng.next_usize(SAMPLES_PER_BLOCK + 2);
            let samples: Vec<f32> = (0..len)
                .map(|_| {
                    let value = (serial as f32) * (1.0 / 1024.0);
                    serial += 1;
                    value
                })
                .collect();
            let expected_outcome = reference.push(&samples);
            let actual_outcome = producer.push(&samples);
            assert_eq!(actual_outcome, expected_outcome, "push of len {len}");
            match expected_outcome {
                RealtimePcmPushOutcome::EmptyBlock => push_counts[0] += 1,
                RealtimePcmPushOutcome::BlockTooLarge => push_counts[1] += 1,
                RealtimePcmPushOutcome::Inserted => push_counts[2] += 1,
                RealtimePcmPushOutcome::ReplacedOldest => push_counts[3] += 1,
            }
        } else {
            let out_len = 1 + rng.next_usize(SAMPLES_PER_BLOCK * 2);
            let mut actual = vec![0.0; out_len];
            let mut expected = vec![0.0; out_len];
            let actual_outcome = consumer.pop_into(&mut actual);
            let expected_outcome = reference.pop_into(&mut expected);
            assert_eq!(actual_outcome, expected_outcome, "pop of len {out_len}");
            assert_eq!(actual, expected, "pop content of len {out_len}");
            match expected_outcome {
                RealtimePcmPopOutcome::Filled { .. } => pop_counts[0] += 1,
                RealtimePcmPopOutcome::Underflow { .. } => pop_counts[1] += 1,
            }
        }

        assert_eq!(producer.len(), reference.len(), "producer occupancy drift");
        assert_eq!(consumer.len(), reference.len(), "consumer occupancy drift");
        assert_eq!(
            producer.active_len(),
            reference.active_len(),
            "producer active occupancy drift"
        );
        assert_eq!(
            consumer.active_len(),
            reference.active_len(),
            "consumer active occupancy drift"
        );
    }

    assert!(
        push_counts.iter().all(|&count| count > 0),
        "the fixed sequence mix must exercise every push outcome: {push_counts:?}"
    );
    assert!(
        pop_counts.iter().all(|&count| count > 0),
        "the fixed sequence mix must exercise every pop outcome: {pop_counts:?}"
    );
    assert!(epoch_resets > 0, "epoch resets must be exercised");
    assert!(serial > 0, "sample serial must advance");
}
