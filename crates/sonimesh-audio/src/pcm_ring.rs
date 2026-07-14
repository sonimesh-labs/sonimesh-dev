/// Result of writing one PCM block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PcmPushOutcome {
    /// Block entered an available slot.
    Inserted,
    /// Oldest block was discarded before the new block was written.
    DroppedOldest,
    /// Block exceeded per-slot capacity.
    BlockTooLarge,
    /// Zero-capacity configuration cannot accept blocks.
    Disabled,
}

/// Result of reading one PCM block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PcmPopOutcome {
    /// One block was copied into the output slice.
    Filled {
        /// Number of valid samples copied.
        samples: usize,
    },
    /// Ring was empty and the output slice was filled with silence.
    Underflow,
}

/// Preallocated PCM block ring with explicit overflow and underflow behavior.
///
/// This container is intentionally single-owner. A platform adapter may transfer
/// ownership through a reviewed SPSC boundary; it must not wrap this type in a
/// mutex used by the output callback.
pub struct PcmBlockRing<const CAPACITY: usize, const SAMPLES_PER_BLOCK: usize> {
    blocks: [[f32; SAMPLES_PER_BLOCK]; CAPACITY],
    lengths: [usize; CAPACITY],
    read: usize,
    write: usize,
    occupied: usize,
}

impl<const CAPACITY: usize, const SAMPLES_PER_BLOCK: usize> Default
    for PcmBlockRing<CAPACITY, SAMPLES_PER_BLOCK>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<const CAPACITY: usize, const SAMPLES_PER_BLOCK: usize>
    PcmBlockRing<CAPACITY, SAMPLES_PER_BLOCK>
{
    /// Creates a fully preallocated PCM ring.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            blocks: [[0.0; SAMPLES_PER_BLOCK]; CAPACITY],
            lengths: [0; CAPACITY],
            read: 0,
            write: 0,
            occupied: 0,
        }
    }

    /// Number of occupied block slots.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.occupied
    }

    /// Returns whether the ring has no ready block.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.occupied == 0
    }

    /// Copies one decoded block into preallocated storage.
    pub fn push(&mut self, samples: &[f32]) -> PcmPushOutcome {
        if CAPACITY == 0 {
            return PcmPushOutcome::Disabled;
        }
        if samples.len() > SAMPLES_PER_BLOCK {
            return PcmPushOutcome::BlockTooLarge;
        }

        let outcome = if self.occupied == CAPACITY {
            self.read = (self.read + 1) % CAPACITY;
            self.occupied -= 1;
            PcmPushOutcome::DroppedOldest
        } else {
            PcmPushOutcome::Inserted
        };

        self.blocks[self.write][..samples.len()].copy_from_slice(samples);
        self.lengths[self.write] = samples.len();
        self.write = (self.write + 1) % CAPACITY;
        self.occupied += 1;
        outcome
    }

    /// Copies one block to the callback-owned output and always initializes it.
    pub fn pop_into(&mut self, output: &mut [f32]) -> PcmPopOutcome {
        output.fill(0.0);
        if self.occupied == 0 || CAPACITY == 0 {
            return PcmPopOutcome::Underflow;
        }

        let samples = self.lengths[self.read].min(output.len());
        output[..samples].copy_from_slice(&self.blocks[self.read][..samples]);
        self.lengths[self.read] = 0;
        self.read = (self.read + 1) % CAPACITY;
        self.occupied -= 1;
        PcmPopOutcome::Filled { samples }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn underflow_writes_bounded_silence() {
        let mut ring = PcmBlockRing::<2, 4>::new();
        let mut output = [1.0; 4];

        assert_eq!(ring.pop_into(&mut output), PcmPopOutcome::Underflow);
        assert!(output.iter().all(|sample| sample.abs() < f32::EPSILON));
    }

    #[test]
    fn overflow_drops_oldest_block() {
        let mut ring = PcmBlockRing::<2, 2>::new();
        assert_eq!(ring.push(&[1.0]), PcmPushOutcome::Inserted);
        assert_eq!(ring.push(&[2.0]), PcmPushOutcome::Inserted);
        assert_eq!(ring.push(&[3.0]), PcmPushOutcome::DroppedOldest);
        let mut output = [0.0; 2];

        assert_eq!(
            ring.pop_into(&mut output),
            PcmPopOutcome::Filled { samples: 1 }
        );
        assert!((output[0] - 2.0).abs() < f32::EPSILON);
        assert!(output[1].abs() < f32::EPSILON);
    }
}
