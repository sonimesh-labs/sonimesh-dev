use core::fmt;

/// Failure while constructing a bounded encoded frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameError {
    /// Payload exceeds the configured per-frame capacity.
    PayloadTooLarge,
}

impl fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLarge => {
                formatter.write_str("encoded frame payload exceeds inline capacity")
            }
        }
    }
}

impl std::error::Error for FrameError {}

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
    /// Frame entered after a higher sequence was already admitted this epoch.
    Reordered,
    /// Frame sequence is already buffered.
    Duplicate,
    /// Frame is older than the current playout position.
    Late,
    /// Capacity/collision forced a stream-local controlled resynchronization.
    Resynchronized {
        /// Number of buffered frames discarded.
        dropped_frames: usize,
    },
    /// The active key epoch has consumed its final sequence number.
    SequenceExhausted,
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
    highest_admitted_sequence: Option<u32>,
    sequence_exhausted: bool,
    playout_started: bool,
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
            highest_admitted_sequence: None,
            sequence_exhausted: false,
            playout_started: false,
        }
    }

    /// Creates an empty buffer whose negotiated playout sequence is known.
    ///
    /// With nonzero capacity, a playout decision before that sequence arrives
    /// reports a missing frame, allowing the decoder worker to schedule PLC at
    /// the negotiated deadline. A zero-capacity buffer is disabled and instead
    /// remains [`PlayoutOutcome::NotReady`].
    #[must_use]
    pub fn with_start_sequence(sequence: u32) -> Self {
        let mut buffer = Self::new();
        buffer.next_sequence = Some(sequence);
        buffer.playout_started = true;
        buffer
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
        if self.sequence_exhausted {
            return InsertOutcome::SequenceExhausted;
        }
        if self.next_sequence.is_some_and(|next| frame.sequence < next) {
            return InsertOutcome::Late;
        }

        let sequence = frame.sequence;
        let reordered = self
            .highest_admitted_sequence
            .is_some_and(|highest| sequence < highest);
        let index = sequence as usize % CAPACITY;
        if let Some(existing) = &self.slots[index] {
            if existing.sequence == frame.sequence {
                return InsertOutcome::Duplicate;
            }
            return self.resynchronize(frame);
        }
        if self.buffered == CAPACITY {
            return self.resynchronize(frame);
        }

        self.next_sequence.get_or_insert(sequence);
        self.slots[index] = Some(frame);
        self.buffered += 1;
        self.highest_admitted_sequence = Some(
            self.highest_admitted_sequence
                .map_or(sequence, |highest| highest.max(sequence)),
        );
        if reordered {
            InsertOutcome::Reordered
        } else {
            InsertOutcome::Inserted
        }
    }

    /// Takes the next sequence for decode or reports a missing frame.
    pub fn take_next(&mut self) -> PlayoutOutcome<MAX_PAYLOAD> {
        if CAPACITY == 0 {
            return PlayoutOutcome::NotReady;
        }
        let Some(sequence) = self.next_sequence else {
            return PlayoutOutcome::NotReady;
        };
        if self.buffered == 0 {
            if !self.playout_started {
                return PlayoutOutcome::NotReady;
            }
            self.advance_sequence(sequence);
            return PlayoutOutcome::Missing { sequence };
        }

        let index = sequence as usize % CAPACITY;
        if let Some(frame) = self.slots[index].take_if(|frame| frame.sequence == sequence) {
            self.buffered -= 1;
            self.advance_sequence(sequence);
            self.playout_started = true;
            return PlayoutOutcome::Frame(frame);
        }

        self.advance_sequence(sequence);
        self.playout_started = true;
        PlayoutOutcome::Missing { sequence }
    }

    /// Clears only this stream's encoded frames.
    pub fn reset(&mut self) {
        self.slots.fill_with(|| None);
        self.buffered = 0;
        self.next_sequence = None;
        self.highest_admitted_sequence = None;
        self.sequence_exhausted = false;
        self.playout_started = false;
    }

    /// Clears this stream and resumes at a negotiated sequence.
    pub fn reset_to(&mut self, sequence: u32) {
        self.reset();
        self.next_sequence = Some(sequence);
        self.playout_started = true;
    }

    fn advance_sequence(&mut self, sequence: u32) {
        self.next_sequence = sequence.checked_add(1);
        self.sequence_exhausted = self.next_sequence.is_none();
    }

    fn resynchronize(&mut self, frame: EncodedFrame<MAX_PAYLOAD>) -> InsertOutcome {
        let dropped_frames = self.buffered;
        self.reset();
        let index = frame.sequence as usize % CAPACITY;
        self.next_sequence = Some(frame.sequence);
        self.highest_admitted_sequence = Some(frame.sequence);
        self.playout_started = true;
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
            &[sequence.to_le_bytes()[0]],
        )
        .unwrap()
    }

    #[test]
    fn reorders_within_fixed_capacity() {
        let mut buffer = JitterBuffer::<4, 8>::new();
        assert_eq!(buffer.insert(frame(10)), InsertOutcome::Inserted);
        assert_eq!(buffer.insert(frame(12)), InsertOutcome::Inserted);
        assert_eq!(buffer.insert(frame(11)), InsertOutcome::Reordered);

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

    #[test]
    fn disabled_buffer_never_schedules_playout() {
        let mut buffer = JitterBuffer::<0, 8>::with_start_sequence(10);

        assert_eq!(buffer.insert(frame(10)), InsertOutcome::Disabled);
        assert_eq!(buffer.take_next(), PlayoutOutcome::NotReady);
        assert_eq!(buffer.take_next(), PlayoutOutcome::NotReady);
    }

    #[test]
    fn negotiated_start_reports_initial_loss() {
        let mut buffer = JitterBuffer::<4, 8>::with_start_sequence(10);
        buffer.insert(frame(11));

        assert_eq!(buffer.take_next(), PlayoutOutcome::Missing { sequence: 10 });
        assert!(matches!(buffer.take_next(), PlayoutOutcome::Frame(_)));
    }

    #[test]
    fn active_playout_keeps_sequence_during_empty_underflow() {
        let mut buffer = JitterBuffer::<4, 8>::with_start_sequence(20);
        buffer.insert(frame(20));
        assert!(matches!(buffer.take_next(), PlayoutOutcome::Frame(_)));

        assert_eq!(buffer.take_next(), PlayoutOutcome::Missing { sequence: 21 });
        buffer.insert(frame(22));
        assert!(matches!(buffer.take_next(), PlayoutOutcome::Frame(_)));
    }

    #[test]
    fn consumes_the_final_sequence_without_wrapping() {
        let mut buffer = JitterBuffer::<4, 8>::with_start_sequence(u32::MAX - 1);
        buffer.insert(frame(u32::MAX - 1));
        buffer.insert(frame(u32::MAX));

        assert!(
            matches!(buffer.take_next(), PlayoutOutcome::Frame(actual) if actual.sequence == u32::MAX - 1)
        );
        assert!(
            matches!(buffer.take_next(), PlayoutOutcome::Frame(actual) if actual.sequence == u32::MAX)
        );
        assert_eq!(buffer.take_next(), PlayoutOutcome::NotReady);
    }

    #[test]
    fn rejects_new_frames_after_sequence_exhaustion() {
        let mut buffer = JitterBuffer::<4, 8>::with_start_sequence(u32::MAX);
        buffer.insert(frame(u32::MAX));
        assert!(matches!(buffer.take_next(), PlayoutOutcome::Frame(_)));

        assert_eq!(buffer.insert(frame(0)), InsertOutcome::SequenceExhausted);
    }

    #[test]
    fn reports_plc_for_a_missing_final_sequence_once() {
        let mut buffer = JitterBuffer::<4, 8>::with_start_sequence(u32::MAX);

        assert_eq!(
            buffer.take_next(),
            PlayoutOutcome::Missing { sequence: u32::MAX }
        );
        assert_eq!(buffer.take_next(), PlayoutOutcome::NotReady);
    }

    #[test]
    fn fresh_epoch_rearms_sequence_admission_after_exhaustion() {
        let mut buffer = JitterBuffer::<4, 8>::with_start_sequence(u32::MAX);
        buffer.insert(frame(u32::MAX));
        assert!(matches!(buffer.take_next(), PlayoutOutcome::Frame(_)));
        assert_eq!(buffer.insert(frame(0)), InsertOutcome::SequenceExhausted);

        buffer.reset_to(0);
        assert_eq!(buffer.insert(frame(0)), InsertOutcome::Inserted);
        assert!(
            matches!(buffer.take_next(), PlayoutOutcome::Frame(actual) if actual.sequence == 0)
        );
    }
}
