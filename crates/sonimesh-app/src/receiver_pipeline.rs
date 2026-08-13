use core::fmt;
use std::sync::Arc;

use sonimesh_audio::{
    AdaptiveJitterTarget, ArrivalJitter, ArrivalJitterConfigError, DecodeStep, DecodeWorkerError,
    FrameDecoder, InsertOutcome, JitterTargetConfig, JitterTargetConfigError, OPUS_SAMPLE_RATE_HZ,
    RealtimePcmProducer, RealtimePcmPushOutcome, StreamDecodeWorker,
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
    adaptive_target: Option<AdaptiveTargetController>,
}

/// Opt-in per-stream adaptive jitter controller fed from the live packet stream.
///
/// The arrival estimator consumes the caller-supplied monotonic clock and the
/// packet sender sample timestamp, and the target controller converts each
/// deviation plus the current jitter occupancy into a bounded target step.
/// This is a packet-processing-domain object; it must never be called from the
/// output callback.
struct AdaptiveTargetController {
    arrival: ArrivalJitter,
    target: AdaptiveJitterTarget,
    tick_source: Box<dyn FnMut() -> u64>,
}

/// Configuration for the opt-in adaptive-target controller of one stream.
pub struct AdaptiveTargetConfig {
    /// Validated per-mode jitter target range and frame conversion.
    pub target: JitterTargetConfig,
    /// Rate of the caller's monotonic arrival clock, in ticks per second.
    pub arrival_ticks_per_second: u64,
    /// Monotonic arrival clock queried once per admitted packet.
    pub tick_source: Box<dyn FnMut() -> u64>,
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

/// Failure while building the opt-in adaptive-target controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdaptiveTargetSetupError {
    /// The protected ingress could not derive its initial media key.
    Ingress(ProtectedIngressError),
    /// The jitter target configuration is unbounded or cannot fit the buffer.
    Target(JitterTargetConfigError),
    /// The arrival-jitter estimator clock rates are invalid.
    Arrival(ArrivalJitterConfigError),
}

impl fmt::Display for AdaptiveTargetSetupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ingress(error) => {
                write!(formatter, "adaptive target ingress setup failed: {error}")
            }
            Self::Target(error) => {
                write!(formatter, "adaptive target configuration failed: {error}")
            }
            Self::Arrival(error) => {
                write!(formatter, "adaptive target arrival clock failed: {error}")
            }
        }
    }
}

impl std::error::Error for AdaptiveTargetSetupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Ingress(error) => Some(error),
            Self::Target(error) => Some(error),
            Self::Arrival(error) => Some(error),
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

impl<DecoderError> std::error::Error for ReceiverPipelineError<DecoderError>
where
    DecoderError: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Ingress(error) => Some(error),
            Self::Decode(error) => Some(error),
            Self::Rotation(error) => Some(error),
        }
    }
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
            adaptive_target: None,
        })
    }

    /// Creates one stream pipeline that also runs the adaptive jitter controller.
    ///
    /// The caller supplies an [`AdaptiveTargetConfig`] with a validated
    /// [`JitterTargetConfig`] and a monotonic arrival clock in ticks of
    /// `arrival_ticks_per_second` (for example `1_000_000_000` for a nanosecond
    /// clock). The clock is queried once per admitted packet; see
    /// [`monotonic_tick_source`](crate::monotonic_tick_source) for a ready-made
    /// process-monotonic source. The controller feeds each authenticated packet
    /// through [`ArrivalJitter`] and [`AdaptiveJitterTarget`] and publishes the
    /// active target state through the shared counters.
    ///
    /// # Errors
    ///
    /// Returns [`AdaptiveTargetSetupError`] when the controller configuration
    /// is unbounded or the initial media opener cannot be derived.
    pub fn new_with_adaptive_target(
        secret: &SessionSecret,
        context: MediaKeyContext,
        decoder: D,
        producer: RealtimePcmProducer<PCM_SAMPLES_PER_BLOCK>,
        counters: Arc<StreamCounters>,
        initial_sequence: u32,
        adaptive: AdaptiveTargetConfig,
    ) -> Result<Self, AdaptiveTargetSetupError> {
        let arrival = ArrivalJitter::new(OPUS_SAMPLE_RATE_HZ, adaptive.arrival_ticks_per_second)
            .map_err(AdaptiveTargetSetupError::Arrival)?;
        let target =
            AdaptiveJitterTarget::new(adaptive.target).map_err(AdaptiveTargetSetupError::Target)?;
        let mut pipeline = Self::new(
            secret,
            context,
            decoder,
            producer,
            counters,
            initial_sequence,
        )
        .map_err(AdaptiveTargetSetupError::Ingress)?;
        pipeline.adaptive_target = Some(AdaptiveTargetController {
            arrival,
            target,
            tick_source: adaptive.tick_source,
        });
        Ok(pipeline)
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
        let (admission, sample_timestamp) = {
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
            let sample_timestamp = frame.header.sample_timestamp;
            (
                self.decode
                    .admit(frame.header.sequence, sample_timestamp, frame.opus_payload),
                sample_timestamp,
            )
        };
        plaintext.fill(0);
        match admission {
            Ok(outcome) => {
                if matches!(outcome, InsertOutcome::Resynchronized { .. }) {
                    self.decode.reset_after_jitter_resynchronization();
                    self.counters.reset_queued_pcm_epoch();
                    self.counters.reset_jitter_epoch();
                    self.reset_adaptive_target_epoch();
                }
                self.record_jitter_outcome(outcome);
                self.publish_jitter_frames();
                self.observe_adaptive_target(outcome, sample_timestamp);
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
        self.publish_jitter_frames();
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
        self.counters.reset_jitter_epoch();
        self.reset_adaptive_target_epoch();
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
        self.counters.reset_jitter_epoch();
        self.reset_adaptive_target_epoch();
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
        self.counters.reset_jitter_epoch();
        self.reset_adaptive_target_epoch();
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
        self.counters.reset_jitter_epoch();
        self.reset_adaptive_target_epoch();
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

    fn publish_jitter_frames(&self) {
        let frames = self.decode.jitter_len();
        self.counters
            .set_jitter_frames(u16::try_from(frames).unwrap_or(u16::MAX));
    }

    fn observe_adaptive_target(&mut self, outcome: InsertOutcome, sample_timestamp: u64) {
        if !matches!(
            outcome,
            InsertOutcome::Inserted
                | InsertOutcome::Reordered
                | InsertOutcome::Late
                | InsertOutcome::Disabled
        ) {
            return;
        }
        let Some(adaptive) = &mut self.adaptive_target else {
            return;
        };
        let arrival_ticks = (adaptive.tick_source)();
        if let Some(deviation_ms) = adaptive.arrival.observe(arrival_ticks, sample_timestamp) {
            let buffered_frames = self.decode.jitter_len();
            let _ = adaptive.target.observe(deviation_ms, buffered_frames);
        }
        self.publish_target_state();
    }

    fn publish_target_state(&self) {
        let Some(adaptive) = &self.adaptive_target else {
            return;
        };
        self.counters.publish_target_state(
            u16::try_from(adaptive.target.min_frames()).unwrap_or(u16::MAX),
            u16::try_from(adaptive.target.max_frames()).unwrap_or(u16::MAX),
            u16::try_from(adaptive.target.target_frames()).unwrap_or(u16::MAX),
            adaptive.target.smoothed_jitter_ms(),
            adaptive.target.raised_steps(),
            adaptive.target.lowered_steps(),
        );
    }

    fn reset_adaptive_target_epoch(&mut self) {
        if let Some(adaptive) = &mut self.adaptive_target {
            adaptive.arrival.reset();
            adaptive.target.reset();
        }
        self.counters.reset_target_state();
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

#[cfg(test)]
mod fresh_session_error_tests {
    use super::FreshSessionInstallError;

    #[test]
    fn reused_session_policy_error_has_no_synthetic_cause() {
        assert!(std::error::Error::source(&FreshSessionInstallError::ReusedSessionId).is_none());
    }
}

#[cfg(test)]
mod adaptive_target_setup_error_tests {
    use std::error::Error;

    use sonimesh_audio::{ArrivalJitterConfigError, JitterTargetConfigError};
    use sonimesh_network::ProtectedIngressError;
    use sonimesh_protocol::MediaCryptoError;

    use super::AdaptiveTargetSetupError;

    fn source(error: &AdaptiveTargetSetupError) -> &(dyn Error + 'static) {
        error
            .source()
            .expect("composite error must retain its cause")
    }

    #[test]
    fn every_variant_retains_its_cause() {
        let ingress = AdaptiveTargetSetupError::Ingress(ProtectedIngressError::Crypto(
            MediaCryptoError::AuthenticationFailed,
        ));
        assert!(
            source(&ingress)
                .downcast_ref::<ProtectedIngressError>()
                .is_some()
        );

        let target = AdaptiveTargetSetupError::Target(JitterTargetConfigError::ZeroCapacity);
        assert!(
            source(&target)
                .downcast_ref::<JitterTargetConfigError>()
                .is_some()
        );

        let arrival = AdaptiveTargetSetupError::Arrival(ArrivalJitterConfigError::ZeroTickRate);
        assert!(
            source(&arrival)
                .downcast_ref::<ArrivalJitterConfigError>()
                .is_some()
        );
    }
}
