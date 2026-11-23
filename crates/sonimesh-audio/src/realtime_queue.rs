use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crossbeam_queue::ArrayQueue;

/// Hard upper bound for decoded PCM blocks queued per stream.
pub const MAX_PCM_QUEUE_BLOCKS: usize = 128;

/// Invalid realtime PCM queue configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RealtimeQueueConfigError {
    /// A queue must have at least one block slot.
    ZeroCapacity,
    /// Capacity exceeds [`MAX_PCM_QUEUE_BLOCKS`].
    CapacityTooLarge,
    /// A PCM block must have room for at least one sample.
    ZeroSamplesPerBlock,
}

impl fmt::Display for RealtimeQueueConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroCapacity => formatter.write_str("PCM queue capacity must be non-zero"),
            Self::CapacityTooLarge => write!(
                formatter,
                "PCM queue capacity exceeds the {MAX_PCM_QUEUE_BLOCKS}-block limit"
            ),
            Self::ZeroSamplesPerBlock => {
                formatter.write_str("PCM queue blocks must hold at least one sample")
            }
        }
    }
}

impl std::error::Error for RealtimeQueueConfigError {}

/// Result of one decoder-domain queue write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RealtimePcmPushOutcome {
    /// Block entered a vacant slot.
    Inserted,
    /// Queue was full and atomically replaced its oldest block.
    ReplacedOldest,
    /// Input contains no samples.
    EmptyBlock,
    /// Input exceeds the configured samples per block.
    BlockTooLarge,
}

/// Result of one realtime callback queue read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RealtimePcmPopOutcome {
    /// One complete decoded block was copied to callback-owned output.
    Filled {
        /// Number of initialized samples copied.
        samples: usize,
        /// Producer-local sequence used for diagnostics and tests.
        sequence: u64,
        /// True when blocks were overwritten or a new stream epoch began.
        discontinuity: bool,
        /// Number of queued blocks discarded because their epoch was retired.
        discarded_stale_blocks: usize,
    },
    /// Queue was empty and callback-owned output was initialized to silence.
    Underflow {
        /// Number of queued blocks discarded because their epoch was retired.
        discarded_stale_blocks: usize,
    },
}

struct PcmBlock<const SAMPLES_PER_BLOCK: usize> {
    epoch: u64,
    sequence: u64,
    samples: [f32; SAMPLES_PER_BLOCK],
    initialized: usize,
}

struct QueueState<const SAMPLES_PER_BLOCK: usize> {
    queue: ArrayQueue<PcmBlock<SAMPLES_PER_BLOCK>>,
    active_epoch: AtomicU64,
    active_blocks: AtomicUsize,
}

/// Decoder-domain endpoint of a bounded logical SPSC PCM queue.
///
/// This endpoint is intentionally not cloneable. [`Self::push`] is lock-free and
/// allocates no memory after [`realtime_pcm_queue`] returns.
pub struct RealtimePcmProducer<const SAMPLES_PER_BLOCK: usize> {
    state: Arc<QueueState<SAMPLES_PER_BLOCK>>,
    epoch: u64,
    next_sequence: u64,
}

impl<const SAMPLES_PER_BLOCK: usize> RealtimePcmProducer<SAMPLES_PER_BLOCK> {
    /// Copies a decoded block into fixed-capacity storage.
    pub fn push(&mut self, samples: &[f32]) -> RealtimePcmPushOutcome {
        if samples.is_empty() {
            return RealtimePcmPushOutcome::EmptyBlock;
        }
        if samples.len() > SAMPLES_PER_BLOCK {
            return RealtimePcmPushOutcome::BlockTooLarge;
        }

        let mut storage = [0.0; SAMPLES_PER_BLOCK];
        storage[..samples.len()].copy_from_slice(samples);
        let block = PcmBlock {
            epoch: self.epoch,
            sequence: self.next_sequence,
            samples: storage,
            initialized: samples.len(),
        };
        self.next_sequence = self.next_sequence.wrapping_add(1);
        increment_active_blocks(&self.state.active_blocks, self.state.queue.capacity());

        let discarded = self.state.queue.force_push(block);
        if let Some(discarded) = discarded {
            if discarded.epoch == self.epoch {
                decrement_active_blocks(&self.state.active_blocks);
            }
            RealtimePcmPushOutcome::ReplacedOldest
        } else {
            RealtimePcmPushOutcome::Inserted
        }
    }

    /// Starts a stream-local epoch and retires queued PCM from the previous one.
    ///
    /// The counters are continuity markers only; they are not cryptographic
    /// nonces. Wrapping is therefore defined and harmless.
    pub fn begin_epoch(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.next_sequence = 0;
        self.state.active_epoch.store(self.epoch, Ordering::Release);
        self.state.active_blocks.store(0, Ordering::Release);
    }

    /// Approximate occupied block count for diagnostics.
    #[must_use]
    pub fn len(&self) -> usize {
        self.state.queue.len()
    }

    /// Approximate count of blocks from the active stream epoch.
    ///
    /// Unlike [`Self::len`], this excludes retired blocks that the realtime
    /// consumer has not yet observed after a stream-local reset. The published
    /// approximation never exceeds the configured queue capacity.
    #[must_use]
    pub fn active_len(&self) -> usize {
        active_block_count(&self.state)
    }

    /// Returns whether no decoded block is currently queued.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.state.queue.is_empty()
    }
}

/// Realtime output-domain endpoint of a bounded logical SPSC PCM queue.
///
/// This endpoint is intentionally not cloneable. [`Self::pop_into`] is lock-free,
/// initializes the complete output slice, and allocates no memory.
pub struct RealtimePcmConsumer<const SAMPLES_PER_BLOCK: usize> {
    state: Arc<QueueState<SAMPLES_PER_BLOCK>>,
    expected: (u64, u64),
}

impl<const SAMPLES_PER_BLOCK: usize> RealtimePcmConsumer<SAMPLES_PER_BLOCK> {
    /// Copies one block to callback-owned output or emits bounded silence.
    pub fn pop_into(&mut self, output: &mut [f32]) -> RealtimePcmPopOutcome {
        output.fill(0.0);
        let mut discarded_stale_blocks = 0;
        for _ in 0..MAX_PCM_QUEUE_BLOCKS {
            let Some(block) = self.state.queue.pop() else {
                return RealtimePcmPopOutcome::Underflow {
                    discarded_stale_blocks,
                };
            };
            let active_epoch = self.state.active_epoch.load(Ordering::Acquire);
            if block.epoch != active_epoch {
                discarded_stale_blocks += 1;
                continue;
            }

            decrement_active_blocks(&self.state.active_blocks);

            let samples = block.initialized.min(output.len());
            output[..samples].copy_from_slice(&block.samples[..samples]);
            let actual = (block.epoch, block.sequence);
            let discontinuity = actual != self.expected;
            self.expected = (block.epoch, block.sequence.wrapping_add(1));

            return RealtimePcmPopOutcome::Filled {
                samples,
                sequence: block.sequence,
                discontinuity,
                discarded_stale_blocks,
            };
        }
        RealtimePcmPopOutcome::Underflow {
            discarded_stale_blocks,
        }
    }

    /// Approximate occupied block count for low-cost diagnostics.
    #[must_use]
    pub fn len(&self) -> usize {
        self.state.queue.len()
    }

    /// Approximate count of blocks from the active stream epoch.
    ///
    /// Unlike [`Self::len`], this excludes retired blocks that the callback has
    /// not yet removed after a stream-local reset. The published approximation
    /// never exceeds the configured queue capacity.
    #[must_use]
    pub fn active_len(&self) -> usize {
        active_block_count(&self.state)
    }

    /// Returns whether the callback would currently underflow.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.state.queue.is_empty()
    }
}

/// Creates the only producer and consumer endpoints for one decoded stream.
///
/// Allocation and reference-count construction happen in this function, before
/// either endpoint is moved into a decoder or realtime output domain.
///
/// # Errors
///
/// Returns [`RealtimeQueueConfigError`] for zero or excessive capacity, or when
/// `SAMPLES_PER_BLOCK` is zero.
pub fn realtime_pcm_queue<const SAMPLES_PER_BLOCK: usize>(
    capacity: usize,
) -> Result<
    (
        RealtimePcmProducer<SAMPLES_PER_BLOCK>,
        RealtimePcmConsumer<SAMPLES_PER_BLOCK>,
    ),
    RealtimeQueueConfigError,
> {
    if capacity == 0 {
        return Err(RealtimeQueueConfigError::ZeroCapacity);
    }
    if capacity > MAX_PCM_QUEUE_BLOCKS {
        return Err(RealtimeQueueConfigError::CapacityTooLarge);
    }
    if SAMPLES_PER_BLOCK == 0 {
        return Err(RealtimeQueueConfigError::ZeroSamplesPerBlock);
    }

    let state = Arc::new(QueueState {
        queue: ArrayQueue::new(capacity),
        active_epoch: AtomicU64::new(0),
        active_blocks: AtomicUsize::new(0),
    });
    Ok((
        RealtimePcmProducer {
            state: Arc::clone(&state),
            epoch: 0,
            next_sequence: 0,
        },
        RealtimePcmConsumer {
            state,
            expected: (0, 0),
        },
    ))
}

fn active_block_count<const SAMPLES_PER_BLOCK: usize>(
    state: &QueueState<SAMPLES_PER_BLOCK>,
) -> usize {
    state
        .active_blocks
        .load(Ordering::Relaxed)
        .min(state.queue.capacity())
}

#[allow(deprecated)] // fetch_update supports the workspace's Rust 1.85 baseline.
fn increment_active_blocks(active_blocks: &AtomicUsize, capacity: usize) {
    // One transient extra count preserves occupancy when `force_push` replaces
    // an active oldest block immediately after this pre-insertion increment.
    let transient_maximum = capacity.saturating_add(1);
    let _ = active_blocks.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
        Some(count.saturating_add(1).min(transient_maximum))
    });
}

#[allow(deprecated)] // fetch_update supports the workspace's Rust 1.85 baseline.
fn decrement_active_blocks(active_blocks: &AtomicUsize) {
    let _ = active_blocks.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
        count.checked_sub(1)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_block_does_not_consume_queue_capacity() {
        let (mut producer, consumer) = realtime_pcm_queue::<2>(1).unwrap();

        assert_eq!(producer.push(&[]), RealtimePcmPushOutcome::EmptyBlock);
        assert_eq!(producer.len(), 0);
        assert_eq!(consumer.active_len(), 0);
    }

    #[test]
    fn public_active_length_clamps_transient_internal_overcount() {
        let (producer, consumer) = realtime_pcm_queue::<1>(2).unwrap();
        producer.state.active_blocks.store(3, Ordering::Relaxed);

        assert_eq!(producer.active_len(), 2);
        assert_eq!(consumer.active_len(), 2);
    }

    #[test]
    fn active_block_counter_does_not_wrap_during_oldest_replacement() {
        let (mut producer, consumer) = realtime_pcm_queue::<1>(2).unwrap();
        producer.push(&[1.0]);
        producer.push(&[2.0]);
        producer
            .state
            .active_blocks
            .store(usize::MAX, Ordering::Relaxed);

        assert_eq!(
            producer.push(&[3.0]),
            RealtimePcmPushOutcome::ReplacedOldest
        );
        assert_eq!(producer.active_len(), 2);
        assert_eq!(consumer.active_len(), 2);
    }
}
