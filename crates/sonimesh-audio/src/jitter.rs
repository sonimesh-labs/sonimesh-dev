/// Failure while constructing a bounded encoded frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameError {
    /// Payload exceeds the configured per-frame capacity.
    PayloadTooLarge,
}

/// One encoded frame stored inline in a bounded jitter buffer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodedFrame<const MAX_PAYLOAD: usize> {
    /// Packet sequence.
    pub sequence: u32,
    /// Sender sample position.
    pub sample_timestamp: u64,
    payload_len: usize,
    payload: [u8; MAX_PAYLOAD],
}

impl<const MAX_PAYLOAD: usize> EncodedFrame<MAX_PAYLOAD> {
    /// Copies a payload into fixed-capacity frame storage.
    ///
    /// # Errors
    ///
    /// Returns [`FrameError::PayloadTooLarge`] when `payload` exceeds the
    /// configured capacity.
    pub fn new(sequence: u32, sample_timestamp: u64, payload: &[u8]) -> Result<Self, FrameError> {
        if payload.len() > MAX_PAYLOAD {
            return Err(FrameError::PayloadTooLarge);
        }
        let mut storage = [0_u8; MAX_PAYLOAD];
        storage[..payload.len()].copy_from_slice(payload);
        Ok(Self {
            sequence,
            sample_timestamp,
            payload_len: payload.len(),
            payload: storage,
        })
    }

    /// Returns the initialized encoded bytes.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload[..self.payload_len]
    }
}

/// Result of inserting an encoded frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InsertOutcome {
    /// Frame entered the bounded buffer.
    Inserted,
    /// Frame sequence is already buffered.
    Duplicate,
    /// Frame is older than the current playout position.
    Late,
    /// Capacity/collision forced a stream-local controlled resynchronization.
    Resynchronized {
        /// Number of buffered frames discarded.
        dropped_frames: usize,
    },
    /// Zero-capacity configuration cannot accept frames.
    Disabled,
}

/// Result of one playout decision.
#[derive(Debug, Eq, PartialEq)]
pub enum PlayoutOutcome<const MAX_PAYLOAD: usize> {
    /// Expected encoded frame is ready.
    Frame(EncodedFrame<MAX_PAYLOAD>),
    /// A future frame exists, so the missing sequence should use PLC.
    Missing {
        /// Missing sequence number.
        sequence: u32,
    },
    /// No frame is buffered; playout should wait rather than advance.
    NotReady,
}

/// Fixed-capacity sequence buffer for one encoded stream.
///
/// Scheduling time and adaptive target selection live above this container. This
/// type owns only ordering, duplicate handling, and bounded overflow behavior.
pub struct JitterBuffer<const CAPACITY: usize, const MAX_PAYLOAD: usize> {
    slots: [Option<EncodedFrame<MAX_PAYLOAD>>; CAPACITY],
    buffered: usize,
    next_sequence: Option<u32>,
}

impl<const CAPACITY: usize, const MAX_PAYLOAD: usize> Default
    for JitterBuffer<CAPACITY, MAX_PAYLOAD>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<const CAPACITY: usize, const MAX_PAYLOAD: usize> JitterBuffer<CAPACITY, MAX_PAYLOAD> {
    /// Creates an empty fixed-capacity buffer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| None),
            buffered: 0,
            next_sequence: None,
        }
    }

    /// Number of currently buffered frames.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.buffered
    }

    /// Returns whether no frame is buffered.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.buffered == 0
    }

    /// Inserts a frame without allocation.
    pub fn insert(&mut self, frame: EncodedFrame<MAX_PAYLOAD>) -> InsertOutcome {
        if CAPACITY == 0 {
            return InsertOutcome::Disabled;
        }
        if self.next_sequence.is_some_and(|next| frame.sequence < next) {
            return InsertOutcome::Late;
        }

        let index = frame.sequence as usize % CAPACITY;
        if let Some(existing) = &self.slots[index] {
            if existing.sequence == frame.sequence {
                return InsertOutcome::Duplicate;
            }
            return self.resynchronize(frame);
        }
        if self.buffered == CAPACITY {
            return self.resynchronize(frame);
        }

        self.next_sequence.get_or_insert(frame.sequence);
        self.slots[index] = Some(frame);
        self.buffered += 1;
        InsertOutcome::Inserted
    }

    /// Takes the next sequence for decode or reports a missing frame.
    pub fn take_next(&mut self) -> PlayoutOutcome<MAX_PAYLOAD> {
        let Some(sequence) = self.next_sequence else {
            return PlayoutOutcome::NotReady;
        };
        if self.buffered == 0 {
            self.next_sequence = None;
            return PlayoutOutcome::NotReady;
        }

        let index = sequence as usize % CAPACITY;
        if let Some(frame) = self.slots[index].take_if(|frame| frame.sequence == sequence) {
            self.buffered -= 1;
            self.next_sequence = sequence.checked_add(1);
            return PlayoutOutcome::Frame(frame);
        }

        self.next_sequence = sequence.checked_add(1);
        PlayoutOutcome::Missing { sequence }
    }

    /// Clears only this stream's encoded frames.
    pub fn reset(&mut self) {
        self.slots.fill_with(|| None);
        self.buffered = 0;
        self.next_sequence = None;
    }

    fn resynchronize(&mut self, frame: EncodedFrame<MAX_PAYLOAD>) -> InsertOutcome {
        let dropped_frames = self.buffered;
        self.reset();
        let index = frame.sequence as usize % CAPACITY;
        self.next_sequence = Some(frame.sequence);
        self.slots[index] = Some(frame);
        self.buffered = 1;
        InsertOutcome::Resynchronized { dropped_frames }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(sequence: u32) -> EncodedFrame<8> {
        EncodedFrame::new(
            sequence,
            u64::from(sequence) * 960,
            &[u8::try_from(sequence).unwrap()],
        )
        .unwrap()
    }

    #[test]
    fn reorders_within_fixed_capacity() {
        let mut buffer = JitterBuffer::<4, 8>::new();
        assert_eq!(buffer.insert(frame(10)), InsertOutcome::Inserted);
        assert_eq!(buffer.insert(frame(12)), InsertOutcome::Inserted);
        assert_eq!(buffer.insert(frame(11)), InsertOutcome::Inserted);

        for expected in [10, 11, 12] {
            let PlayoutOutcome::Frame(actual) = buffer.take_next() else {
                panic!("expected buffered frame");
            };
            assert_eq!(actual.sequence, expected);
        }
    }

    #[test]
    fn reports_gap_for_plc_without_allocating() {
        let mut buffer = JitterBuffer::<4, 8>::new();
        buffer.insert(frame(20));
        buffer.insert(frame(22));
        assert!(matches!(buffer.take_next(), PlayoutOutcome::Frame(_)));
        assert_eq!(buffer.take_next(), PlayoutOutcome::Missing { sequence: 21 });
        assert!(matches!(buffer.take_next(), PlayoutOutcome::Frame(_)));
    }

    #[test]
    fn collision_resynchronizes_only_this_buffer() {
        let mut buffer = JitterBuffer::<2, 8>::new();
        buffer.insert(frame(1));

        assert_eq!(
            buffer.insert(frame(3)),
            InsertOutcome::Resynchronized { dropped_frames: 1 }
        );
        assert_eq!(buffer.len(), 1);
    }

    #[test]
    fn rejects_late_and_duplicate_frames() {
        let mut buffer = JitterBuffer::<4, 8>::new();
        buffer.insert(frame(5));
        assert_eq!(buffer.insert(frame(5)), InsertOutcome::Duplicate);
        assert!(matches!(buffer.take_next(), PlayoutOutcome::Frame(_)));
        buffer.insert(frame(7));
        assert_eq!(buffer.insert(frame(5)), InsertOutcome::Late);
    }
}
