use core::fmt;
use std::sync::Arc;

use sonimesh_audio::{
    DecodeStep, DecodeWorkerError, FrameDecoder, InsertOutcome, RealtimePcmProducer,
    RealtimePcmPushOutcome, StreamDecodeWorker,
};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_network::{
    IngressError, ProtectedIngressError, ProtectedMediaIngress, StreamIdentity,
};
use sonimesh_protocol::{MediaCryptoError, MediaKeyContext, ReplayDecision, SessionSecret};

/// Non-realtime composition of one protected ingress and one decode worker.
///
/// The control domain creates this pipeline after trust establishment. It owns
/// no root secret, TLS session, platform output, or Opus implementation; callers
/// supply the concrete decoder and paired PCM producer endpoint.
pub struct ReceiverStreamPipeline<
    D,
    const JITTER_CAPACITY: usize,
    const MAX_ENCODED_BYTES: usize,
    const PCM_SAMPLES_PER_BLOCK: usize,
> {
    ingress: ProtectedMediaIngress,
    decode: StreamDecodeWorker<D, JITTER_CAPACITY, MAX_ENCODED_BYTES, PCM_SAMPLES_PER_BLOCK>,
    counters: Arc<StreamCounters>,
}

/// Failure while receiving, admitting, decoding, or rotating one stream pipeline.
#[derive(Debug)]
pub enum ReceiverPipelineError<DecoderError> {
    /// Packet parsing, context matching, authentication, or replay failed.
    Ingress(IngressError<MediaCryptoError>),
    /// Authenticated payload did not fit jitter storage or decoder work failed.
    Decode(DecodeWorkerError<DecoderError>),
    /// Next media context could not safely replace the active ingress.
    Rotation(ProtectedIngressError),
}

/// Failure while replacing a pipeline with authenticated fresh media state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FreshSessionInstallError {
    /// The replacement reused the active media session ID.
    ReusedSessionId,
    /// The replacement AEAD opener could not be derived.
    Ingress(ProtectedIngressError),
}

impl fmt::Display for FreshSessionInstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReusedSessionId => {
                formatter.write_str("fresh media session reused the active session ID")
            }
            Self::Ingress(error) => write!(formatter, "fresh media ingress setup failed: {error}"),
        }
    }
}

impl std::error::Error for FreshSessionInstallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReusedSessionId => None,
            Self::Ingress(error) => Some(error),
        }
    }
}

impl<DecoderError: fmt::Display> fmt::Display for ReceiverPipelineError<DecoderError> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ingress(error) => write!(formatter, "receiver ingress failed: {error}"),
            Self::Decode(error) => write!(formatter, "receiver decode pipeline failed: {error}"),
            Self::Rotation(error) => write!(formatter, "receiver epoch rotation failed: {error}"),
        }
    }
}

impl<DecoderError> std::error::Error for ReceiverPipelineError<DecoderError> where
    DecoderError: std::error::Error + 'static
{
}

impl<
    D,
    const JITTER_CAPACITY: usize,
    const MAX_ENCODED_BYTES: usize,
    const PCM_SAMPLES_PER_BLOCK: usize,
> ReceiverStreamPipeline<D, JITTER_CAPACITY, MAX_ENCODED_BYTES, PCM_SAMPLES_PER_BLOCK>
where
    D: FrameDecoder,
{
    /// Creates one stream pipeline with fresh AEAD/replay and jitter state.
    ///
    /// # Errors
    ///
    /// Returns [`ProtectedIngressError`] when the initial media opener cannot be
    /// derived from the caller-supplied authenticated session secret. `counters`
    /// is shared only with low-rate diagnostics or other atomic writers; the
    /// realtime callback does not receive this pipeline.
    pub fn new(
        secret: &SessionSecret,
        context: MediaKeyContext,
        decoder: D,
        producer: RealtimePcmProducer<PCM_SAMPLES_PER_BLOCK>,
        counters: Arc<StreamCounters>,
        initial_sequence: u32,
    ) -> Result<Self, ProtectedIngressError> {
        Ok(Self {
            ingress: ProtectedMediaIngress::new(secret, context)?,
            decode: StreamDecodeWorker::new(decoder, producer, initial_sequence),
            counters,
        })
    }

    /// Returns the media key context currently accepted by this pipeline.
    #[must_use]
    pub const fn context(&self) -> MediaKeyContext {
        self.ingress.context()
    }

    /// Returns the session, stream, and epoch accepted by this pipeline.
    #[must_use]
    pub const fn identity(&self) -> StreamIdentity {
        self.ingress.identity()
    }

    /// Borrows atomic counters published by this stream's non-realtime pipeline.
    #[must_use]
    pub fn counters(&self) -> &StreamCounters {
        &self.counters
    }

    /// Authenticates and inserts one datagram into this stream's bounded jitter state.
    ///
    /// # Errors
    ///
    /// Returns an ingress or decode-admission error before any payload reaches
    /// the decoder. Decode-admission failure clears the caller-owned plaintext
    /// buffer before returning.
    pub fn receive(
        &mut self,
        datagram: &[u8],
        plaintext: &mut [u8],
    ) -> Result<InsertOutcome, ReceiverPipelineError<D::Error>> {
        let admission = {
            let frame = match self.ingress.receive(datagram, plaintext) {
                Ok(frame) => {
                    self.counters.authenticated();
                    frame
                }
                Err(error) => {
                    self.record_ingress_error(&error);
                    return Err(ReceiverPipelineError::Ingress(error));
                }
            };
            self.decode.admit(
                frame.header.sequence,
                frame.header.sample_timestamp,
                frame.opus_payload,
            )
        };
        plaintext.fill(0);
        match admission {
            Ok(outcome) => {
                if matches!(outcome, InsertOutcome::Resynchronized { .. }) {
                    self.decode.reset_after_jitter_resynchronization();
                    self.counters.reset_queued_pcm_epoch();
                }
                self.record_jitter_outcome(outcome);
                Ok(outcome)
            }
            Err(error) => {
                plaintext.fill(0);
                self.counters.rejected();
                Err(ReceiverPipelineError::Decode(error))
            }
        }
    }

    /// Runs one per-stream decode or PLC decision outside the output callback.
    ///
    /// # Errors
    ///
    /// Returns a bounded decoder failure after retiring only this stream's
    /// decoder, jitter, and PCM state without invoking platform output work.
    pub fn decode_next(&mut self) -> Result<DecodeStep, DecodeWorkerError<D::Error>> {
        let step = match self.decode.decode_next() {
            Ok(step) => step,
            Err(error) => {
                self.counters.decode_error();
                self.hard_resynchronize();
                return Err(error);
            }
        };
        match step {
            DecodeStep::Decoded { queue, .. } => self.record_queue_outcome(queue),
            DecodeStep::Concealed { queue, .. } => {
                self.counters.plc_frame();
                self.record_queue_outcome(queue);
            }
            DecodeStep::NotReady => {}
        }
        self.publish_queued_pcm_blocks();
        Ok(step)
    }

    /// Installs a strictly newer AEAD epoch and resets only this stream's audio state.
    ///
    /// # Errors
    ///
    /// Returns without changing decode state if the protected ingress rejects the
    /// next context. On success, old jitter and PCM blocks are retired before
    /// frames starting at `initial_sequence` can be admitted.
    pub fn rotate(
        &mut self,
        secret: &SessionSecret,
        next: MediaKeyContext,
        initial_sequence: u32,
    ) -> Result<(), ProtectedIngressError> {
        self.ingress.rotate(secret, next)?;
        self.decode.begin_epoch(initial_sequence);
        self.counters.reset_queued_pcm_epoch();
        self.counters.stream_reset();
        Ok(())
    }

    /// Installs a newly authenticated media session without recreating output ownership.
    ///
    /// Unlike [`Self::rotate`], the replacement context may change session or
    /// stream identity. The caller must establish the supplied secret and
    /// context through the control domain before calling this method.
    ///
    /// # Errors
    ///
    /// Returns without changing the active ingress or playout state when the
    /// replacement opener cannot be derived. On success, replay, jitter,
    /// decoder, and queued PCM state begin at `initial_sequence`.
    pub fn install_fresh_session(
        &mut self,
        secret: &SessionSecret,
        context: MediaKeyContext,
        initial_sequence: u32,
    ) -> Result<(), FreshSessionInstallError> {
        if context.session_id == self.ingress.context().session_id {
            return Err(FreshSessionInstallError::ReusedSessionId);
        }
        let replacement = ProtectedMediaIngress::new(secret, context)
            .map_err(FreshSessionInstallError::Ingress)?;
        self.ingress = replacement;
        self.decode.begin_epoch(initial_sequence);
        self.counters.reset_queued_pcm_epoch();
        self.counters.stream_reset();
        Ok(())
    }

    /// Clears this stream's playout state after a drift-controller hard resync.
    ///
    /// This leaves AEAD/replay state and other streams intact. The next
    /// authenticated packet establishes a fresh playout sequence, while the
    /// realtime consumer retires PCM queued before this reset.
    pub fn hard_resynchronize(&mut self) {
        self.decode.hard_resynchronize();
        self.counters.reset_queued_pcm_epoch();
        self.counters.stream_reset();
    }

    /// Installs a producer paired with a freshly created output source.
    ///
    /// The caller stops and drops the old platform callback before this method.
    /// Network admission remains active, while undecoded jitter and decoder
    /// history are discarded so the new output queue starts from a clean stream
    /// boundary.
    pub fn replace_pcm_producer(&mut self, producer: RealtimePcmProducer<PCM_SAMPLES_PER_BLOCK>) {
        self.decode.replace_pcm_producer(producer);
        self.counters.reset_queued_pcm_epoch();
        self.counters.stream_reset();
    }

    /// Returns the approximate queued PCM-block count for a low-rate controller.
    #[must_use]
    pub fn queued_pcm_blocks(&self) -> usize {
        self.publish_queued_pcm_blocks()
    }

    fn record_ingress_error(&self, error: &IngressError<MediaCryptoError>) {
        if let IngressError::Replay(decision) = error {
            self.counters.authenticated();
            if *decision == ReplayDecision::Duplicate {
                self.counters.duplicate();
            }
        }
        self.counters.rejected();
    }

    fn record_queue_outcome(&self, outcome: RealtimePcmPushOutcome) {
        if outcome == RealtimePcmPushOutcome::ReplacedOldest {
            self.counters.pcm_overflow();
        }
    }

    fn publish_queued_pcm_blocks(&self) -> usize {
        let blocks = self.decode.queued_blocks();
        self.counters
            .set_queued_pcm_blocks(u16::try_from(blocks).unwrap_or(u16::MAX));
        blocks
    }

    fn record_jitter_outcome(&self, outcome: InsertOutcome) {
        match outcome {
            InsertOutcome::Duplicate => self.counters.duplicate(),
            InsertOutcome::Late => self.counters.late(),
            InsertOutcome::Reordered => self.counters.reordered(),
            InsertOutcome::Resynchronized { dropped_frames } => {
                self.counters.jitter_resync_dropped_frames(dropped_frames);
                self.counters.stream_reset();
            }
            InsertOutcome::SequenceExhausted => self.counters.rejected(),
            InsertOutcome::Inserted | InsertOutcome::Disabled => {}
        }
    }
}
