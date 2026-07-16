use core::fmt;

use sonimesh_audio::{
    DecodeStep, DecodeWorkerError, FrameDecoder, InsertOutcome, RealtimePcmProducer,
    StreamDecodeWorker,
};
use sonimesh_network::{
    IngressError, ProtectedIngressError, ProtectedMediaIngress, StreamIdentity,
};
use sonimesh_protocol::{MediaCryptoError, MediaKeyContext, SessionSecret};

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
    /// derived from the caller-supplied authenticated session secret.
    pub fn new(
        secret: &SessionSecret,
        context: MediaKeyContext,
        decoder: D,
        producer: RealtimePcmProducer<PCM_SAMPLES_PER_BLOCK>,
        initial_sequence: u32,
    ) -> Result<Self, ProtectedIngressError> {
        Ok(Self {
            ingress: ProtectedMediaIngress::new(secret, context)?,
            decode: StreamDecodeWorker::new(decoder, producer, initial_sequence),
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
            let frame = self
                .ingress
                .receive(datagram, plaintext)
                .map_err(ReceiverPipelineError::Ingress)?;
            self.decode.admit(
                frame.header.sequence,
                frame.header.sample_timestamp,
                frame.opus_payload,
            )
        };
        match admission {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                plaintext.fill(0);
                Err(ReceiverPipelineError::Decode(error))
            }
        }
    }

    /// Runs one per-stream decode or PLC decision outside the output callback.
    ///
    /// # Errors
    ///
    /// Returns a bounded decoder failure without invoking platform output work.
    pub fn decode_next(&mut self) -> Result<DecodeStep, DecodeWorkerError<D::Error>> {
        self.decode.decode_next()
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
        Ok(())
    }
}
