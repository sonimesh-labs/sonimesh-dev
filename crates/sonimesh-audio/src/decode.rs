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
    /// The decoder reported no PCM or more samples than caller-owned storage holds.
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
                formatter.write_str("decoder reported PCM outside the bounded output block")
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
        Ok(Self::new(decoder, producer, sequence))
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
    /// Returns a decoder error, or rejects a decoder that reports a length
    /// beyond the compile-time PCM block capacity. No block is queued on error.
    pub fn decode_next(&mut self) -> Result<DecodeStep, DecodeWorkerError<D::Error>> {
        let playout = self.jitter.take_next();
        let (sequence, concealed, samples) = match playout {
            PlayoutOutcome::NotReady => return Ok(DecodeStep::NotReady),
            PlayoutOutcome::Frame(frame) => (
                frame.sequence,
                false,
                self.decoder
                    .decode(frame.payload(), &mut self.pcm)
                    .map_err(DecodeWorkerError::Decoder)?,
            ),
            PlayoutOutcome::Missing { sequence } => (
                sequence,
                true,
                self.decoder
                    .decode_plc(&mut self.pcm)
                    .map_err(DecodeWorkerError::Decoder)?,
            ),
        };
        if samples == 0 || samples > PCM_SAMPLES_PER_BLOCK {
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
