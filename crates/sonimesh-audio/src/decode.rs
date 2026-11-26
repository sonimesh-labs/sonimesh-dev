use core::fmt;

use crate::{
    EncodedFrame, FrameError, InsertOutcome, JitterBuffer, OpusFrameConfig, OpusFrameConfigError,
    PlayoutOutcome, RealtimePcmProducer, RealtimePcmPushOutcome,
};

/// Per-stream decoder boundary invoked outside the realtime output callback.
///
/// Implementations may wrap an Opus decoder once its policy and platform adapter
/// are selected. They must initialize between one and `output.len()` samples
/// and must implement packet-loss concealment without allocating.
pub trait FrameDecoder {
    /// Decoder-specific failure type.
    type Error;

    /// Decodes one authenticated encoded frame into caller-owned PCM storage.
    ///
    /// # Errors
    ///
    /// Returns a decoder-specific error for malformed or unsupported encoded
    /// input. Callers must isolate that failure to this stream.
    fn decode(&mut self, encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error>;

    /// Generates one PCM block for a confirmed missing frame.
    ///
    /// # Errors
    ///
    /// Returns a decoder-specific error when concealment cannot produce a
    /// bounded block.
    fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error>;

    /// Clears stream-local decoder history after a discontinuity.
    ///
    /// The default is suitable for stateless test decoders. Stateful decoders
    /// should clear PLC, predictor, or packet history without allocating.
    fn reset(&mut self) {}
}

/// Outcome of one per-stream decode scheduling decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeStep {
    /// The jitter buffer has not reached a playout decision yet.
    NotReady,
    /// One encoded frame was decoded into the realtime PCM queue.
    Decoded {
        /// Sequence selected for ordinary decode.
        sequence: u32,
        /// Initialized PCM samples in the queued block.
        samples: usize,
        /// Bounded PCM queue result.
        queue: RealtimePcmPushOutcome,
    },
    /// One missing frame was handled by decoder-side PLC.
    Concealed {
        /// Missing sequence selected for concealment.
        sequence: u32,
        /// Initialized PCM samples in the queued block.
        samples: usize,
        /// Bounded PCM queue result.
        queue: RealtimePcmPushOutcome,
    },
}

/// Failure while admitting or decoding a stream-local frame.
#[derive(Eq, PartialEq)]
pub enum DecodeWorkerError<DecoderError> {
    /// An authenticated payload exceeded inline jitter storage.
    Frame(FrameError),
    /// The decoder reported an invalid length for the output block or Opus frame.
    InvalidOutputLength,
    /// The concrete decoder rejected an authenticated frame or PLC request.
    Decoder(DecoderError),
}

impl<DecoderError> fmt::Debug for DecodeWorkerError<DecoderError> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Frame(error) => formatter.debug_tuple("Frame").field(error).finish(),
            Self::InvalidOutputLength => formatter.write_str("InvalidOutputLength"),
            Self::Decoder(_) => formatter.write_str("Decoder"),
        }
    }
}

impl<DecoderError: fmt::Display> fmt::Display for DecodeWorkerError<DecoderError> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Frame(error) => write!(formatter, "encoded frame rejected: {error}"),
            Self::InvalidOutputLength => {
                formatter.write_str("decoder reported an invalid PCM output length")
            }
            Self::Decoder(error) => write!(formatter, "stream decoder failed: {error}"),
        }
    }
}

impl<DecoderError> std::error::Error for DecodeWorkerError<DecoderError>
where
    DecoderError: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Frame(error) => Some(error),
            Self::InvalidOutputLength => None,
            Self::Decoder(error) => Some(error),
        }
    }
}

/// Per-stream non-realtime pipeline from authenticated frames to PCM blocks.
///
/// Encoded packets remain in a fixed-capacity jitter buffer. Normal decode and
/// PLC run only when [`Self::decode_next`] is called by the per-stream domain;
/// the output callback only reads the paired realtime consumer endpoint.
pub struct StreamDecodeWorker<
    D,
    const JITTER_CAPACITY: usize,
    const MAX_ENCODED_BYTES: usize,
    const PCM_SAMPLES_PER_BLOCK: usize,
> {
    decoder: D,
    jitter: JitterBuffer<JITTER_CAPACITY, MAX_ENCODED_BYTES>,
    producer: RealtimePcmProducer<PCM_SAMPLES_PER_BLOCK>,
    pcm: [f32; PCM_SAMPLES_PER_BLOCK],
    opus_frame: Option<OpusFrameConfig>,
}

impl<
    D,
    const JITTER_CAPACITY: usize,
    const MAX_ENCODED_BYTES: usize,
    const PCM_SAMPLES_PER_BLOCK: usize,
> StreamDecodeWorker<D, JITTER_CAPACITY, MAX_ENCODED_BYTES, PCM_SAMPLES_PER_BLOCK>
where
    D: FrameDecoder,
{
    /// Creates one stream-local worker with a negotiated playout sequence.
    #[must_use]
    pub fn new(
        decoder: D,
        producer: RealtimePcmProducer<PCM_SAMPLES_PER_BLOCK>,
        sequence: u32,
    ) -> Self {
        Self {
            decoder,
            jitter: JitterBuffer::with_start_sequence(sequence),
            producer,
            pcm: [0.0; PCM_SAMPLES_PER_BLOCK],
            opus_frame: None,
        }
    }

    /// Creates a stream-local worker for the initial validated Opus frame policy.
    ///
    /// This is a construction-time check in the non-realtime domain. A future
    /// native Opus binding must use this entry point so its caller-owned PCM
    /// block is proved large enough before any packet can reach the decoder.
    ///
    /// # Errors
    ///
    /// Returns the frame-policy error when the selected 48 kHz stereo 10/20 ms
    /// shape does not fit this worker's compile-time PCM block capacity.
    pub fn new_for_opus(
        decoder: D,
        producer: RealtimePcmProducer<PCM_SAMPLES_PER_BLOCK>,
        sequence: u32,
        frame: OpusFrameConfig,
    ) -> Result<Self, OpusFrameConfigError> {
        frame.validate_pcm_block_capacity(PCM_SAMPLES_PER_BLOCK)?;
        let mut worker = Self::new(decoder, producer, sequence);
        worker.opus_frame = Some(frame);
        Ok(worker)
    }

    /// Returns the selected Opus frame policy, or None for a generic worker.
    #[must_use]
    pub const fn opus_frame_config(&self) -> Option<OpusFrameConfig> {
        self.opus_frame
    }

    /// Selects a validated Opus frame shape and retires the previous playout epoch.
    ///
    /// The host must call this outside an active stream, after establishing the
    /// matching media session. An insufficient PCM block leaves the worker unchanged.
    ///
    /// # Errors
    ///
    /// Returns the capacity error if the selected frame does not fit.
    pub fn select_opus_frame(
        &mut self,
        frame: OpusFrameConfig,
        sequence: u32,
    ) -> Result<(), OpusFrameConfigError> {
        frame.validate_pcm_block_capacity(PCM_SAMPLES_PER_BLOCK)?;
        self.opus_frame = Some(frame);
        self.begin_epoch(sequence);
        Ok(())
    }

    /// Admits a packet that has already passed network authentication and replay checks.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeWorkerError::Frame`] when the authenticated payload does
    /// not fit the stream's fixed encoded-frame capacity.
    pub fn admit(
        &mut self,
        sequence: u32,
        sample_timestamp: u64,
        payload: &[u8],
    ) -> Result<InsertOutcome, DecodeWorkerError<D::Error>> {
        let frame = EncodedFrame::new(sequence, sample_timestamp, payload)
            .map_err(DecodeWorkerError::Frame)?;
        Ok(self.jitter.insert(frame))
    }

    /// Performs one scheduled decode or PLC decision outside the realtime callback.
    ///
    /// # Errors
    ///
    /// Returns a decoder error or rejects an invalid output length. An Opus
    /// worker requires exactly one configured frame. No block is queued on error.
    pub fn decode_next(&mut self) -> Result<DecodeStep, DecodeWorkerError<D::Error>> {
        let playout = self.jitter.take_next();
        let output_samples = self
            .opus_frame
            .map_or(PCM_SAMPLES_PER_BLOCK, OpusFrameConfig::interleaved_samples);
        let (sequence, concealed, samples) = match playout {
            PlayoutOutcome::NotReady => return Ok(DecodeStep::NotReady),
            PlayoutOutcome::Frame(frame) => (
                frame.sequence,
                false,
                self.decoder
                    .decode(frame.payload(), &mut self.pcm[..output_samples])
                    .map_err(DecodeWorkerError::Decoder)?,
            ),
            PlayoutOutcome::Missing { sequence } => (
                sequence,
                true,
                self.decoder
                    .decode_plc(&mut self.pcm[..output_samples])
                    .map_err(DecodeWorkerError::Decoder)?,
            ),
        };
        if samples == 0
            || samples > output_samples
            || self
                .opus_frame
                .is_some_and(|frame| samples != frame.interleaved_samples())
        {
            self.pcm.fill(0.0);
            return Err(DecodeWorkerError::InvalidOutputLength);
        }

        let queue = self.producer.push(&self.pcm[..samples]);
        if concealed {
            Ok(DecodeStep::Concealed {
                sequence,
                samples,
                queue,
            })
        } else {
            Ok(DecodeStep::Decoded {
                sequence,
                samples,
                queue,
            })
        }
    }

    /// Begins a fresh media epoch at its negotiated initial packet sequence.
    ///
    /// This drops only this stream's undecoded jitter frames, clears reusable
    /// decode scratch, and marks queued PCM so the realtime consumer discards
    /// blocks from the retired epoch. The caller installs matching network AEAD
    /// state before admitting frames for `sequence`.
    pub fn begin_epoch(&mut self, sequence: u32) {
        self.reset_stream_state();
        self.jitter.reset_to(sequence);
    }

    /// Clears one stream's playout state after an external hard resynchronization.
    ///
    /// The next admitted frame establishes a new playout sequence. Existing PCM
    /// is retired by advancing only this queue's local epoch, so the realtime
    /// consumer discards it before rendering a later block.
    pub fn hard_resynchronize(&mut self) {
        self.reset_stream_state();
    }

    /// Retires decoded PCM after the jitter buffer accepted a new resync point.
    ///
    /// The caller invokes this only after [`Self::admit`] returns
    /// [`InsertOutcome::Resynchronized`]. The jitter buffer already contains
    /// the new frame, so this clears only decoder history, reusable PCM scratch,
    /// and the previous PCM epoch without discarding that frame.
    pub fn reset_after_jitter_resynchronization(&mut self) {
        self.decoder.reset();
        self.pcm.fill(0.0);
        self.producer.begin_epoch();
    }

    /// Replaces the PCM producer after the old platform output source stopped.
    ///
    /// The new queue starts empty, so pending jitter, decoder history, and PCM
    /// scratch are cleared. Network admission remains owned by the caller's
    /// ingress boundary and is not changed here.
    pub fn replace_pcm_producer(&mut self, producer: RealtimePcmProducer<PCM_SAMPLES_PER_BLOCK>) {
        self.producer = producer;
        self.reset_decode_state();
    }

    /// Returns the number of queued PCM blocks for low-rate diagnostics.
    #[must_use]
    pub fn queued_blocks(&self) -> usize {
        self.producer.active_len()
    }

    /// Returns the number of encoded frames currently buffered for playout.
    ///
    /// This is an O(1) read of the fixed-capacity jitter buffer, safe to sample
    /// from the per-stream decode domain after admission or playout decisions.
    #[must_use]
    pub fn jitter_len(&self) -> usize {
        self.jitter.len()
    }

    fn reset_stream_state(&mut self) {
        self.reset_decode_state();
        self.producer.begin_epoch();
    }

    fn reset_decode_state(&mut self) {
        self.jitter.reset();
        self.decoder.reset();
        self.pcm.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RealtimePcmPopOutcome;
    use crate::realtime_pcm_queue;

    const JITTER_CAPACITY: usize = 4;
    const MAX_ENCODED_BYTES: usize = 64;
    const PCM_SAMPLES_PER_BLOCK: usize = 16;
    const QUEUE_CAPACITY: usize = 4;

    /// Deterministic decoder that writes markers and can report a bogus length.
    #[derive(Default)]
    struct MockDecoder {
        reported_samples: usize,
        fail_decode: bool,
    }

    impl MockDecoder {
        fn reported(reported_samples: usize) -> Self {
            Self {
                reported_samples,
                ..Self::default()
            }
        }
    }

    impl FrameDecoder for MockDecoder {
        type Error = &'static str;

        fn decode(&mut self, _encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
            if self.fail_decode {
                return Err("mock decode failure");
            }
            let writable = self.reported_samples.min(output.len());
            output[..writable].fill(1.0);
            Ok(self.reported_samples)
        }

        fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
            let writable = self.reported_samples.min(output.len());
            output[..writable].fill(-1.0);
            Ok(self.reported_samples)
        }
    }

    type Worker =
        StreamDecodeWorker<MockDecoder, JITTER_CAPACITY, MAX_ENCODED_BYTES, PCM_SAMPLES_PER_BLOCK>;

    fn make_worker(
        decoder: MockDecoder,
        sequence: u32,
    ) -> (Worker, crate::RealtimePcmConsumer<PCM_SAMPLES_PER_BLOCK>) {
        let (producer, consumer) =
            realtime_pcm_queue::<PCM_SAMPLES_PER_BLOCK>(QUEUE_CAPACITY).unwrap();
        (
            StreamDecodeWorker::new(decoder, producer, sequence),
            consumer,
        )
    }

    #[test]
    fn disabled_capacity_never_schedules_playout() {
        let (producer, _consumer) =
            realtime_pcm_queue::<PCM_SAMPLES_PER_BLOCK>(QUEUE_CAPACITY).unwrap();
        let mut worker = StreamDecodeWorker::<
            MockDecoder,
            0,
            MAX_ENCODED_BYTES,
            PCM_SAMPLES_PER_BLOCK,
        >::new(MockDecoder::default(), producer, 0);
        assert_eq!(
            worker.admit(0, 0, &[1, 2, 3]).unwrap(),
            InsertOutcome::Disabled
        );
        assert_eq!(worker.decode_next().unwrap(), DecodeStep::NotReady);
        assert_eq!(worker.decode_next().unwrap(), DecodeStep::NotReady);
    }

    #[test]
    fn decodes_the_negotiated_first_frame() {
        let (mut worker, mut consumer) = make_worker(MockDecoder::reported(4), 0);
        assert_eq!(
            worker.admit(0, 0, &[0xAB]).unwrap(),
            InsertOutcome::Inserted
        );
        assert_eq!(
            worker.decode_next().unwrap(),
            DecodeStep::Decoded {
                sequence: 0,
                samples: 4,
                queue: RealtimePcmPushOutcome::Inserted,
            }
        );
        assert_eq!(worker.jitter_len(), 0);
        assert_eq!(worker.queued_blocks(), 1);
        let mut block = [0.0_f32; PCM_SAMPLES_PER_BLOCK];
        assert!(matches!(
            consumer.pop_into(&mut block),
            RealtimePcmPopOutcome::Filled { samples: 4, .. }
        ));
        assert!(
            block[..4]
                .iter()
                .all(|sample| (sample - 1.0).abs() < 1.0e-6)
        );
    }

    #[test]
    fn conceals_a_confirmed_missing_frame_with_plc() {
        let (mut worker, mut consumer) = make_worker(MockDecoder::reported(4), 0);
        // Only sequence 1 arrives; the negotiated first sequence 0 is missing.
        assert_eq!(
            worker.admit(1, 960, &[0x01]).unwrap(),
            InsertOutcome::Inserted
        );
        assert_eq!(
            worker.decode_next().unwrap(),
            DecodeStep::Concealed {
                sequence: 0,
                samples: 4,
                queue: RealtimePcmPushOutcome::Inserted,
            }
        );
        assert_eq!(
            worker.decode_next().unwrap(),
            DecodeStep::Decoded {
                sequence: 1,
                samples: 4,
                queue: RealtimePcmPushOutcome::Inserted,
            }
        );
        let mut block = [0.0_f32; PCM_SAMPLES_PER_BLOCK];
        assert!(matches!(
            consumer.pop_into(&mut block),
            RealtimePcmPopOutcome::Filled { samples: 4, .. }
        ));
        assert!(
            block[..4]
                .iter()
                .all(|sample| (sample + 1.0).abs() < 1.0e-6)
        );
    }

    #[test]
    fn decodes_reordered_frames_in_sequence_order() {
        let (mut worker, _consumer) = make_worker(MockDecoder::reported(4), 0);
        assert_eq!(
            worker.admit(0, 0, &[0x00]).unwrap(),
            InsertOutcome::Inserted
        );
        assert_eq!(
            worker.admit(2, 1_920, &[0x02]).unwrap(),
            InsertOutcome::Inserted
        );
        assert_eq!(
            worker.admit(1, 960, &[0x01]).unwrap(),
            InsertOutcome::Reordered
        );

        for expected in [0_u32, 1, 2] {
            assert!(matches!(
                worker.decode_next().unwrap(),
                DecodeStep::Decoded {
                    sequence,
                    samples: 4,
                    queue: RealtimePcmPushOutcome::Inserted,
                } if sequence == expected
            ));
        }
    }

    #[test]
    fn rejects_empty_and_oversized_payloads_at_admission() {
        let (mut worker, _consumer) = make_worker(MockDecoder::default(), 0);
        assert!(matches!(
            worker.admit(0, 0, &[]),
            Err(DecodeWorkerError::Frame(FrameError::EmptyPayload))
        ));
        assert!(matches!(
            worker.admit(0, 0, &[0_u8; MAX_ENCODED_BYTES + 1]),
            Err(DecodeWorkerError::Frame(FrameError::PayloadTooLarge))
        ));
        assert_eq!(worker.jitter_len(), 0);
    }

    #[test]
    fn rejects_invalid_output_length_without_queuing() {
        let (mut worker, mut consumer) = make_worker(MockDecoder::reported(0), 0);
        worker.admit(0, 0, &[0x01]).unwrap();
        assert_eq!(
            worker.decode_next().unwrap_err(),
            DecodeWorkerError::InvalidOutputLength
        );
        assert_eq!(worker.queued_blocks(), 0);
        let mut block = [0.0_f32; PCM_SAMPLES_PER_BLOCK];
        assert!(matches!(
            consumer.pop_into(&mut block),
            RealtimePcmPopOutcome::Underflow { .. }
        ));

        let (mut worker, mut consumer) =
            make_worker(MockDecoder::reported(PCM_SAMPLES_PER_BLOCK + 1), 0);
        worker.admit(0, 0, &[0x01]).unwrap();
        assert_eq!(
            worker.decode_next().unwrap_err(),
            DecodeWorkerError::InvalidOutputLength
        );
        assert_eq!(worker.queued_blocks(), 0);
        assert!(matches!(
            consumer.pop_into(&mut block),
            RealtimePcmPopOutcome::Underflow { .. }
        ));
    }

    #[test]
    fn isolates_decoder_errors_without_queuing() {
        let mut decoder = MockDecoder::reported(4);
        decoder.fail_decode = true;
        let (mut worker, mut consumer) = make_worker(decoder, 0);
        worker.admit(0, 0, &[0x01]).unwrap();
        assert!(matches!(
            worker.decode_next(),
            Err(DecodeWorkerError::Decoder("mock decode failure"))
        ));
        assert_eq!(worker.queued_blocks(), 0);
        let mut block = [0.0_f32; PCM_SAMPLES_PER_BLOCK];
        assert!(matches!(
            consumer.pop_into(&mut block),
            RealtimePcmPopOutcome::Underflow { .. }
        ));
    }

    #[test]
    fn capacity_collision_resynchronizes_only_this_stream() {
        let (producer, _consumer) =
            realtime_pcm_queue::<PCM_SAMPLES_PER_BLOCK>(QUEUE_CAPACITY).unwrap();
        let mut worker = StreamDecodeWorker::<
            MockDecoder,
            2,
            MAX_ENCODED_BYTES,
            PCM_SAMPLES_PER_BLOCK,
        >::new(MockDecoder::reported(4), producer, 0);
        assert_eq!(
            worker.admit(0, 0, &[0x00]).unwrap(),
            InsertOutcome::Inserted
        );
        assert_eq!(
            worker.admit(2, 1_920, &[0x02]).unwrap(),
            InsertOutcome::Resynchronized { dropped_frames: 1 }
        );
        assert_eq!(worker.jitter_len(), 1);
        assert!(matches!(
            worker.decode_next().unwrap(),
            DecodeStep::Decoded { sequence: 2, .. }
        ));
    }

    #[test]
    fn begin_epoch_retires_queued_pcm() {
        let (mut worker, mut consumer) = make_worker(MockDecoder::reported(4), 0);
        worker.admit(0, 0, &[0x00]).unwrap();
        worker.decode_next().unwrap();
        assert_eq!(worker.queued_blocks(), 1);

        worker.begin_epoch(100);
        assert_eq!(worker.queued_blocks(), 0);
        let mut block = [0.0_f32; PCM_SAMPLES_PER_BLOCK];
        assert!(matches!(
            consumer.pop_into(&mut block),
            RealtimePcmPopOutcome::Underflow {
                discarded_stale_blocks: 1
            }
        ));
    }

    #[test]
    fn selecting_opus_frame_retires_old_audio_and_rejects_insufficient_capacity() {
        let frame_10 = OpusFrameConfig::new(48_000, 2, 10).unwrap();
        let frame_20 = OpusFrameConfig::new(48_000, 2, 20).unwrap();
        let (producer, mut consumer) = realtime_pcm_queue::<960>(4).unwrap();
        let mut worker = StreamDecodeWorker::<_, 4, 64, 960>::new_for_opus(
            MockDecoder::reported(960),
            producer,
            0,
            frame_10,
        )
        .unwrap();
        worker.admit(0, 0, &[1]).unwrap();
        worker.decode_next().unwrap();
        worker.admit(1, 480, &[2]).unwrap();

        assert_eq!(
            worker.select_opus_frame(frame_20, 4),
            Err(OpusFrameConfigError::PcmBlockTooSmall)
        );
        assert_eq!(worker.opus_frame_config(), Some(frame_10));
        assert_eq!(worker.jitter_len(), 1);
        assert_eq!(worker.queued_blocks(), 1);

        worker.select_opus_frame(frame_10, 4).unwrap();
        assert_eq!(worker.jitter_len(), 0);
        assert_eq!(worker.queued_blocks(), 0);
        let mut block = [0.0; 960];
        assert!(matches!(
            consumer.pop_into(&mut block),
            RealtimePcmPopOutcome::Underflow { .. }
        ));
        worker.admit(4, 1_920, &[3]).unwrap();
        assert!(matches!(
            worker.decode_next(),
            Ok(DecodeStep::Decoded {
                sequence: 4,
                samples: 960,
                ..
            })
        ));
    }

    #[test]
    fn hard_resynchronize_clears_jitter_and_pcm() {
        let (mut worker, mut consumer) = make_worker(MockDecoder::reported(4), 0);
        worker.admit(0, 0, &[0x00]).unwrap();
        worker.decode_next().unwrap();
        worker.admit(1, 960, &[0x01]).unwrap();
        assert_eq!(worker.jitter_len(), 1);

        worker.hard_resynchronize();
        assert_eq!(worker.jitter_len(), 0);
        assert_eq!(worker.decode_next().unwrap(), DecodeStep::NotReady);
        let mut block = [0.0_f32; PCM_SAMPLES_PER_BLOCK];
        assert!(matches!(
            consumer.pop_into(&mut block),
            RealtimePcmPopOutcome::Underflow { .. }
        ));
    }

    #[test]
    fn reset_after_jitter_resync_keeps_the_new_frame() {
        let (producer, _consumer) =
            realtime_pcm_queue::<PCM_SAMPLES_PER_BLOCK>(QUEUE_CAPACITY).unwrap();
        let mut worker = StreamDecodeWorker::<
            MockDecoder,
            2,
            MAX_ENCODED_BYTES,
            PCM_SAMPLES_PER_BLOCK,
        >::new(MockDecoder::reported(4), producer, 0);
        worker.admit(0, 0, &[0x00]).unwrap();
        assert_eq!(
            worker.admit(2, 1_920, &[0x02]).unwrap(),
            InsertOutcome::Resynchronized { dropped_frames: 1 }
        );
        worker.reset_after_jitter_resynchronization();
        assert_eq!(worker.jitter_len(), 1);
        assert!(matches!(
            worker.decode_next().unwrap(),
            DecodeStep::Decoded { sequence: 2, .. }
        ));
    }

    #[test]
    fn replace_pcm_producer_starts_empty_and_clears_decode_state() {
        let (mut worker, _old_consumer) = make_worker(MockDecoder::reported(4), 0);
        worker.admit(0, 0, &[0x00]).unwrap();
        worker.decode_next().unwrap();
        assert_eq!(worker.queued_blocks(), 1);

        let (producer, _new_consumer) =
            realtime_pcm_queue::<PCM_SAMPLES_PER_BLOCK>(QUEUE_CAPACITY).unwrap();
        worker.replace_pcm_producer(producer);
        assert_eq!(worker.queued_blocks(), 0);
        assert_eq!(worker.jitter_len(), 0);
        assert_eq!(worker.decode_next().unwrap(), DecodeStep::NotReady);
    }
}
