//! Desktop receiver composition that keeps the control owner and media
//! pipeline together.
//!
//! The Tauri shell and control transport can use this boundary without passing
//! a receiver pipeline through every lifecycle callback. The pipeline remains
//! injected, so this type does not choose a decoder, socket, or platform output
//! backend on behalf of the application.

use std::sync::Arc;

use sonimesh_audio::{DecodeStep, DecodeWorkerError, FrameDecoder};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::AudioOutput;
use sonimesh_protocol::{Envelope, MediaDirection, envelope};

use crate::{
    ControlSessionToken, ControlStreamAction, ControlStreamCoordinator, DesktopControlOwner,
    DesktopControlOwnerError, MediaStreamIdentity, ReceiverPcmProducerInstaller,
    ReceiverPipelineError, StartedMixerStream, StreamStartRequest, UdpPollOutcome,
    UdpReceiverError, UdpReceiverPipeline,
};

pub(crate) type MediaPrepare<'a, P> = dyn FnMut(
        ControlSessionToken,
        StreamStartRequest,
        &ControlStreamCoordinator,
        &mut P,
    ) -> Result<(), DesktopControlOwnerError>
    + 'a;

fn opus_request_frame(request: StreamStartRequest) -> Option<sonimesh_audio::OpusFrameConfig> {
    u16::try_from(request.channels).ok().and_then(|channels| {
        u16::try_from(request.frame_duration_ms)
            .ok()
            .and_then(|duration| {
                sonimesh_audio::OpusFrameConfig::new(request.sample_rate_hz, channels, duration)
                    .ok()
            })
    })
}

/// Result of one UDP media scheduler tick.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UdpMediaTick {
    poll: UdpPollOutcome,
    decode: DecodeStep,
}

impl UdpMediaTick {
    /// Returns the non-blocking packet poll outcome.
    #[must_use]
    pub const fn poll(&self) -> UdpPollOutcome {
        self.poll
    }

    /// Returns the stream-local decode or PLC decision.
    #[must_use]
    pub const fn decode(&self) -> DecodeStep {
        self.decode
    }
}

/// Owns one shell-facing desktop control owner and its injected media pipeline.
pub struct DesktopReceiverOwner<
    O,
    P,
    const SLOTS: usize,
    const OUTPUT_SAMPLES_PER_BLOCK: usize,
    const MAX_SOURCES: usize,
    const PCM_SAMPLES_PER_BLOCK: usize,
> where
    O: AudioOutput,
    P: ReceiverPcmProducerInstaller<PCM_SAMPLES_PER_BLOCK>,
{
    owner: DesktopControlOwner<O, SLOTS, OUTPUT_SAMPLES_PER_BLOCK, MAX_SOURCES>,
    pipeline: P,
    counters: Option<Arc<StreamCounters>>,
}

impl<
    O,
    P,
    const SLOTS: usize,
    const OUTPUT_SAMPLES_PER_BLOCK: usize,
    const MAX_SOURCES: usize,
    const PCM_SAMPLES_PER_BLOCK: usize,
> DesktopReceiverOwner<O, P, SLOTS, OUTPUT_SAMPLES_PER_BLOCK, MAX_SOURCES, PCM_SAMPLES_PER_BLOCK>
where
    O: AudioOutput,
    P: ReceiverPcmProducerInstaller<PCM_SAMPLES_PER_BLOCK>,
{
    /// Creates a receiver owner around an already-configured control owner and
    /// media pipeline.
    #[must_use]
    pub fn new(
        owner: DesktopControlOwner<O, SLOTS, OUTPUT_SAMPLES_PER_BLOCK, MAX_SOURCES>,
        pipeline: P,
        counters: Option<Arc<StreamCounters>>,
    ) -> Self {
        Self {
            owner,
            pipeline,
            counters,
        }
    }

    /// Borrows the shell-facing control owner.
    #[must_use]
    pub const fn owner(
        &self,
    ) -> &DesktopControlOwner<O, SLOTS, OUTPUT_SAMPLES_PER_BLOCK, MAX_SOURCES> {
        &self.owner
    }

    /// Mutably borrows the control owner for pairing, snapshots, and recovery.
    pub fn owner_mut(
        &mut self,
    ) -> &mut DesktopControlOwner<O, SLOTS, OUTPUT_SAMPLES_PER_BLOCK, MAX_SOURCES> {
        &mut self.owner
    }

    /// Mutably borrows the injected media pipeline for packet polling and decode.
    pub fn pipeline_mut(&mut self) -> &mut P {
        &mut self.pipeline
    }

    /// Re-admits a persisted stream to a recreated output using this receiver's pipeline.
    ///
    /// The caller must select the identity corresponding to this owned pipeline.
    ///
    /// # Errors
    ///
    /// Returns an error when output is unavailable or the stream cannot be re-admitted.
    pub fn recover_stream(
        &mut self,
        identity: MediaStreamIdentity,
    ) -> Result<StartedMixerStream, DesktopControlOwnerError> {
        self.owner.recover_stream(identity, &mut self.pipeline)
    }

    /// Accepts one authenticated control envelope and executes its lifecycle action.
    ///
    /// The returned action is preserved for transport reply generation. A
    /// negotiation, no-op, or ignored action completes without touching the
    /// pipeline; start actions use the optional counters supplied at construction.
    ///
    /// # Errors
    ///
    /// Returns a control or mixer error when envelope acceptance or action
    /// completion fails.
    pub fn accept_and_execute(
        &mut self,
        session: ControlSessionToken,
        envelope: &Envelope,
    ) -> Result<ControlStreamAction, DesktopControlOwnerError> {
        self.accept_and_execute_inner(session, envelope, None)
    }

    /// Executes a control envelope with host-provided preparation for a new stream.
    ///
    /// Preparation runs only after a new start is accepted, before media context
    /// checks and mixer admission. It must leave the pipeline unchanged on error.
    /// Preparation is rejected while another stream is active.
    /// A previously installed context may be reused by the host for a retry of
    /// the same pending start; it must not issue the same allocator grant twice.
    /// Duplicate commands and rotations never invoke this callback.
    ///
    /// # Errors
    ///
    /// Returns the preparation error after aborting its pending start, or an
    /// existing control, format, context, or mixer error.
    pub fn accept_and_execute_with_prepare(
        &mut self,
        session: ControlSessionToken,
        envelope: &Envelope,
        prepare: &mut impl FnMut(
            ControlSessionToken,
            StreamStartRequest,
            &ControlStreamCoordinator,
            &mut P,
        ) -> Result<(), DesktopControlOwnerError>,
    ) -> Result<ControlStreamAction, DesktopControlOwnerError> {
        self.accept_and_execute_inner(session, envelope, Some(prepare))
    }

    pub(crate) fn accept_and_execute_inner(
        &mut self,
        session: ControlSessionToken,
        envelope: &Envelope,
        prepare: Option<&mut MediaPrepare<'_, P>>,
    ) -> Result<ControlStreamAction, DesktopControlOwnerError> {
        let action = self.owner.accept_control(session, envelope)?;
        if let ControlStreamAction::Start { token, request } = action {
            if prepare.is_some()
                && self.pipeline.opus_frame_config().is_some()
                && opus_request_frame(request)
                    .is_none_or(|frame| self.pipeline.validate_fresh_opus_frame(frame).is_err())
            {
                let _ = self
                    .owner
                    .bridge_mut()
                    .coordinator_mut()
                    .abort_pending(token);
                return Err(DesktopControlOwnerError::MediaFormatMismatch);
            }
            if let Some(prepare) = prepare {
                if self.owner.bridge().coordinator().active().is_some() {
                    let _ = self
                        .owner
                        .bridge_mut()
                        .coordinator_mut()
                        .abort_pending(token);
                    return Err(DesktopControlOwnerError::MediaContextMismatch);
                }
                if let Err(error) = prepare(
                    session,
                    request,
                    self.owner.bridge().coordinator(),
                    &mut self.pipeline,
                ) {
                    let _ = self
                        .owner
                        .bridge_mut()
                        .coordinator_mut()
                        .abort_pending(token);
                    return Err(error);
                }
            }
        }
        let requested = match action {
            ControlStreamAction::Start { token, request } => Some((Some(token), request.identity)),
            ControlStreamAction::Rotate { token, next, .. } => Some((Some(token), next.identity)),
            ControlStreamAction::Noop => match envelope.payload.as_ref() {
                Some(envelope::Payload::StartStream(start)) => {
                    u16::try_from(start.media_key_epoch).ok().map(|key_epoch| {
                        (
                            None,
                            MediaStreamIdentity {
                                session_id: start.media_session_id,
                                stream_id: start.stream_id,
                                key_epoch,
                            },
                        )
                    })
                }
                _ => None,
            },
            _ => None,
        };
        if let (Some(context), Some((token, identity))) = (self.pipeline.media_context(), requested)
        {
            if identity
                .media_key_context(MediaDirection::AndroidToDesktop)
                .ok()
                != Some(context)
            {
                if let Some(token) = token {
                    let _ = self
                        .owner
                        .bridge_mut()
                        .coordinator_mut()
                        .abort_pending(token);
                }
                return Err(DesktopControlOwnerError::MediaContextMismatch);
            }
        }
        if let (Some(frame), Some(envelope::Payload::StartStream(start))) =
            (self.pipeline.opus_frame_config(), envelope.payload.as_ref())
        {
            if start.sample_rate_hz != sonimesh_audio::OPUS_SAMPLE_RATE_HZ
                || start.channels != u32::from(sonimesh_audio::OPUS_CHANNELS)
                || start.frame_duration_ms != u32::from(frame.frame_duration_ms())
            {
                if let Some((Some(token), _)) = requested {
                    let _ = self
                        .owner
                        .bridge_mut()
                        .coordinator_mut()
                        .abort_pending(token);
                }
                return Err(DesktopControlOwnerError::MediaFormatMismatch);
            }
        }
        self.owner
            .execute_control_action(action, self.counters.clone(), &mut self.pipeline)?;
        Ok(action)
    }
}

impl<
    O,
    D,
    const SLOTS: usize,
    const OUTPUT_SAMPLES_PER_BLOCK: usize,
    const MAX_SOURCES: usize,
    const JITTER_CAPACITY: usize,
    const MAX_ENCODED_BYTES: usize,
    const PCM_SAMPLES_PER_BLOCK: usize,
>
    DesktopReceiverOwner<
        O,
        UdpReceiverPipeline<D, JITTER_CAPACITY, MAX_ENCODED_BYTES, PCM_SAMPLES_PER_BLOCK>,
        SLOTS,
        OUTPUT_SAMPLES_PER_BLOCK,
        MAX_SOURCES,
        PCM_SAMPLES_PER_BLOCK,
    >
where
    O: AudioOutput,
    D: FrameDecoder,
{
    /// Polls one UDP datagram outside the output callback.
    ///
    /// # Errors
    ///
    /// Returns socket, source-endpoint, or protected-pipeline errors from the
    /// injected UDP receiver.
    pub fn poll_media(&mut self) -> Result<UdpPollOutcome, UdpReceiverError<D::Error>> {
        self.pipeline.poll()
    }

    /// Runs one decode or PLC decision outside the output callback.
    ///
    /// The caller supplies whether further authenticated arrivals are pending;
    /// a successful packet poll alone does not establish that condition.
    ///
    /// # Errors
    ///
    /// Returns the injected decoder's bounded worker error.
    pub fn decode_media(
        &mut self,
        arrivals_pending: bool,
    ) -> Result<DecodeStep, DecodeWorkerError<D::Error>> {
        self.pipeline
            .decode_next_with_arrivals_pending(arrivals_pending)
    }

    /// Polls one datagram and immediately runs one decode or PLC decision.
    ///
    /// A packet poll cannot establish whether further arrivals are pending, so
    /// this convenience path drains immediately. Call [`Self::poll_media`] and
    /// [`Self::decode_media`] separately when that signal is available.
    ///
    /// # Errors
    ///
    /// Returns the UDP socket, protected-pipeline, or decoder error that stops
    /// this tick.
    pub fn tick_media(&mut self) -> Result<UdpMediaTick, UdpReceiverError<D::Error>> {
        let poll = self.pipeline.poll()?;
        let decode = self
            .pipeline
            .decode_next_with_arrivals_pending(false)
            .map_err(|error| UdpReceiverError::Pipeline(ReceiverPipelineError::Decode(error)))?;
        Ok(UdpMediaTick { poll, decode })
    }

    /// Returns the local UDP media endpoint assigned to the injected pipeline.
    ///
    /// # Errors
    ///
    /// Returns the operating-system error when the socket endpoint cannot be
    /// queried.
    pub fn local_media_addr(&self) -> Result<std::net::SocketAddr, std::io::Error> {
        self.pipeline.local_addr()
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;
    use std::net::SocketAddr;
    use std::sync::Arc;

    use sonimesh_audio::{FrameDecoder, RealtimePcmProducer, realtime_pcm_queue};
    use sonimesh_diagnostics::StreamCounters;
    use sonimesh_platform::{
        AudioOutput, OutputConfig, OutputDeviceId, OutputDeviceInfo, OutputError, OutputHealth,
        RenderSource, SampleFormat,
    };
    use sonimesh_protocol::{
        CONTROL_PROTOCOL_VERSION, Capability, Envelope, Hello, MediaDirection, MediaKeyContext,
        SessionSecret, StartStream, envelope,
    };

    use super::DesktopReceiverOwner;
    use crate::{
        ControlStreamAction, ControlToMixerBridge, DesktopControlOwner, MediaStreamIdentity,
        OutputSelection, ReceiverPcmProducerInstaller, SessionState,
    };

    const SLOTS: usize = 2;
    const OUTPUT_SAMPLES: usize = 256;
    const SOURCE_SAMPLES: usize = 480;

    #[derive(Default)]
    struct TestOutput;

    impl AudioOutput for TestOutput {
        fn devices(&self) -> Result<Vec<OutputDeviceInfo>, OutputError> {
            Ok(Vec::new())
        }

        fn start(
            &mut self,
            _device: Option<&OutputDeviceId>,
            _config: OutputConfig,
            _source: Box<dyn RenderSource>,
        ) -> Result<(), OutputError> {
            Ok(())
        }

        fn stop(&mut self) {}

        fn health(&self) -> OutputHealth {
            OutputHealth::Healthy
        }
    }

    #[derive(Default)]
    struct TestPipeline {
        replacements: usize,
        producer: Option<RealtimePcmProducer<SOURCE_SAMPLES>>,
    }

    struct ByteDecoder;

    impl FrameDecoder for ByteDecoder {
        type Error = Infallible;

        fn decode(&mut self, _encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
            output.fill(0.25);
            Ok(output.len())
        }

        fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
            output.fill(0.0);
            Ok(output.len())
        }
    }

    impl ReceiverPcmProducerInstaller<SOURCE_SAMPLES> for TestPipeline {
        fn replace_pcm_producer(&mut self, producer: RealtimePcmProducer<SOURCE_SAMPLES>) {
            self.replacements += 1;
            self.producer = Some(producer);
        }
    }

    fn receiver()
    -> DesktopReceiverOwner<TestOutput, TestPipeline, SLOTS, OUTPUT_SAMPLES, 1, SOURCE_SAMPLES>
    {
        let bridge = ControlToMixerBridge::new(
            SessionState::READY,
            &[Capability::FrameDuration10Ms],
            TestOutput,
            OutputSelection {
                device: None,
                config: OutputConfig {
                    sample_rate_hz: 48_000,
                    channels: 2,
                    sample_format: SampleFormat::Float32,
                    requested_callback_frames: Some(480),
                    maximum_callback_frames: 1_024,
                },
            },
            8,
            8,
        )
        .unwrap();
        let owner = DesktopControlOwner::new(bridge).unwrap();
        DesktopReceiverOwner::new(
            owner,
            TestPipeline::default(),
            Some(Arc::new(StreamCounters::default())),
        )
    }

    fn identity() -> MediaStreamIdentity {
        MediaStreamIdentity {
            session_id: 7,
            stream_id: 1,
            key_epoch: 0,
        }
    }

    fn hello() -> Envelope {
        Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 1,
            payload: Some(envelope::Payload::Hello(Hello {
                supported_versions: vec![CONTROL_PROTOCOL_VERSION],
                capabilities: vec![Capability::FrameDuration10Ms as i32],
                display_name: "test sender".to_owned(),
            })),
        }
    }

    fn start() -> Envelope {
        let id = identity();
        Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 2,
            payload: Some(envelope::Payload::StartStream(StartStream {
                media_session_id: id.session_id,
                stream_id: id.stream_id,
                sample_rate_hz: 48_000,
                channels: 1,
                frame_duration_ms: 10,
                media_key_epoch: u32::from(id.key_epoch),
            })),
        }
    }

    #[test]
    fn accepts_and_executes_negotiation_then_start() {
        let mut receiver = receiver();
        let session = receiver
            .owner_mut()
            .install_authenticated_control_session()
            .unwrap();
        assert!(matches!(
            receiver.accept_and_execute(session, &hello()),
            Ok(ControlStreamAction::Negotiated(_))
        ));
        assert!(matches!(
            receiver.accept_and_execute(session, &start()),
            Ok(ControlStreamAction::Start { .. })
        ));
        assert_eq!(receiver.pipeline_mut().replacements, 1);
        assert_eq!(
            receiver
                .owner_mut()
                .view_at(1)
                .unwrap()
                .stream_metrics()
                .len(),
            1
        );
    }

    #[test]
    fn preparation_failure_aborts_start_and_duplicate_does_not_prepare_again() {
        let mut receiver = receiver();
        let session = receiver
            .owner_mut()
            .install_authenticated_control_session()
            .unwrap();
        receiver.accept_and_execute(session, &hello()).unwrap();
        let mut calls = 0;
        let mut prepare = |token,
                           request: crate::StreamStartRequest,
                           coordinator: &crate::ControlStreamCoordinator,
                           _: &mut TestPipeline| {
            calls += 1;
            assert_eq!(token, session);
            assert_eq!(request.identity, identity());
            assert!(coordinator.has_authenticated_control_session(token));
            if calls == 1 {
                Err(crate::DesktopControlOwnerError::MediaContextMismatch)
            } else {
                Ok(())
            }
        };
        assert!(matches!(
            receiver.accept_and_execute_with_prepare(session, &start(), &mut prepare),
            Err(crate::DesktopControlOwnerError::MediaContextMismatch)
        ));
        assert!(!receiver.owner().bridge().coordinator().has_pending_change());
        assert!(receiver.owner().bridge().coordinator().active().is_none());
        assert!(matches!(
            receiver.accept_and_execute_with_prepare(session, &start(), &mut prepare),
            Ok(ControlStreamAction::Start { .. })
        ));
        assert!(matches!(
            receiver.accept_and_execute_with_prepare(session, &start(), &mut prepare),
            Ok(ControlStreamAction::Noop)
        ));
        let mut competing = start();
        if let Some(envelope::Payload::StartStream(request)) = &mut competing.payload {
            request.media_session_id = 8;
            request.stream_id = 2;
        }
        assert!(
            receiver
                .accept_and_execute_with_prepare(session, &competing, &mut prepare)
                .is_err()
        );
        assert_eq!(calls, 2);
        assert_eq!(receiver.pipeline_mut().replacements, 1);
        assert!(receiver.owner().bridge().coordinator().active().is_some());
    }

    #[test]
    fn opus_request_validation_checks_fixed_capacity() {
        let request = crate::StreamStartRequest {
            identity: identity(),
            sample_rate_hz: 48_000,
            channels: 2,
            frame_duration_ms: 20,
        };
        let frame = super::opus_request_frame(request).unwrap();
        assert!(frame.validate_pcm_block_capacity(960).is_err());
        assert!(frame.validate_pcm_block_capacity(1920).is_ok());
    }

    #[test]
    fn adaptive_capacity_rejects_before_preparation_and_allows_a_fitting_retry() {
        use crate::{AdaptiveTargetConfig, UdpReceiverPipeline};
        use sonimesh_audio::{JitterMode, OpusFrameConfig, StreamDecodeWorker};

        const PCM: usize = 1920;
        let secret = SessionSecret::new([7; 32]);
        let old_context = MediaKeyContext::new(8, 1, 0, MediaDirection::AndroidToDesktop).unwrap();
        let (producer, _) = realtime_pcm_queue::<PCM>(8).unwrap();
        let worker = StreamDecodeWorker::<_, 4, 256, PCM>::new_for_opus(
            ByteDecoder,
            producer,
            0,
            OpusFrameConfig::new(48_000, 2, 20).unwrap(),
        )
        .unwrap();
        let counters = Arc::new(StreamCounters::default());
        let pipeline = UdpReceiverPipeline::bind_with_worker_and_adaptive_target(
            SocketAddr::from(([127, 0, 0, 1], 0)),
            &secret,
            old_context,
            worker,
            Arc::clone(&counters),
            AdaptiveTargetConfig::preset(JitterMode::Balanced, 20, 4, 1_000),
        )
        .unwrap();
        let bridge = ControlToMixerBridge::new(
            SessionState::READY,
            &[Capability::FrameDuration10Ms, Capability::FrameDuration20Ms],
            TestOutput,
            OutputSelection {
                device: None,
                config: OutputConfig {
                    sample_rate_hz: 48_000,
                    channels: 2,
                    sample_format: SampleFormat::Float32,
                    requested_callback_frames: Some(480),
                    maximum_callback_frames: 1_024,
                },
            },
            8,
            8,
        )
        .unwrap();
        let owner = DesktopControlOwner::new(bridge).unwrap();
        let mut receiver: DesktopReceiverOwner<_, _, SLOTS, OUTPUT_SAMPLES, 1, PCM> =
            DesktopReceiverOwner::new(owner, pipeline, Some(counters));
        let session = receiver
            .owner_mut()
            .install_authenticated_control_session()
            .unwrap();
        let mut greeting = hello();
        if let Some(envelope::Payload::Hello(hello)) = &mut greeting.payload {
            hello
                .capabilities
                .push(Capability::FrameDuration20Ms as i32);
        }
        receiver.accept_and_execute(session, &greeting).unwrap();
        let mut request = start();
        if let Some(envelope::Payload::StartStream(start)) = &mut request.payload {
            start.channels = 2;
        }
        let mut calls = 0;
        let mut prepare =
            |_,
             request: crate::StreamStartRequest,
             _: &_,
             pipeline: &mut UdpReceiverPipeline<ByteDecoder, 4, 256, PCM>| {
                calls += 1;
                let frame = OpusFrameConfig::new(
                    request.sample_rate_hz,
                    u16::try_from(request.channels).unwrap(),
                    u16::try_from(request.frame_duration_ms).unwrap(),
                )
                .unwrap();
                let context = request
                    .identity
                    .media_key_context(MediaDirection::AndroidToDesktop)
                    .unwrap();
                pipeline
                    .install_fresh_opus_session(&secret, context, 0, frame)
                    .unwrap();
                Ok(())
            };
        assert!(matches!(
            receiver.accept_and_execute_with_prepare(session, &request, &mut prepare),
            Err(crate::DesktopControlOwnerError::MediaFormatMismatch)
        ));
        assert!(!receiver.owner().bridge().coordinator().has_pending_change());
        assert_eq!(receiver.pipeline_mut().context(), old_context);
        if let Some(envelope::Payload::StartStream(start)) = &mut request.payload {
            start.frame_duration_ms = 20;
        }
        assert!(matches!(
            receiver.accept_and_execute_with_prepare(session, &request, &mut prepare),
            Ok(ControlStreamAction::Start { .. })
        ));
        assert_eq!(calls, 1);
        assert_eq!(
            receiver.pipeline_mut().context().session_id,
            identity().session_id
        );
    }

    #[test]
    fn udp_pipeline_media_methods_poll_and_decode_outside_control_actions() {
        const JITTER: usize = 8;
        const MAX_ENCODED: usize = 256;
        const PCM: usize = 480;
        type UdpPipeline = crate::UdpReceiverPipeline<ByteDecoder, JITTER, MAX_ENCODED, PCM>;
        type Receiver =
            DesktopReceiverOwner<TestOutput, UdpPipeline, SLOTS, OUTPUT_SAMPLES, 1, PCM>;

        let (producer, _consumer) = realtime_pcm_queue::<PCM>(8).unwrap();
        let secret = SessionSecret::new([7; 32]);
        let context = MediaKeyContext::new(7, 1, 0, MediaDirection::AndroidToDesktop).unwrap();
        let counters = Arc::new(StreamCounters::default());
        let pipeline = UdpPipeline::bind(
            SocketAddr::from(([127, 0, 0, 1], 0)),
            &secret,
            context,
            ByteDecoder,
            producer,
            Arc::clone(&counters),
            0,
        )
        .unwrap();
        let bridge = ControlToMixerBridge::new(
            SessionState::READY,
            &[Capability::FrameDuration10Ms],
            TestOutput,
            OutputSelection {
                device: None,
                config: OutputConfig {
                    sample_rate_hz: 48_000,
                    channels: 2,
                    sample_format: SampleFormat::Float32,
                    requested_callback_frames: Some(480),
                    maximum_callback_frames: 1_024,
                },
            },
            8,
            8,
        )
        .unwrap();
        let owner = DesktopControlOwner::new(bridge).unwrap();
        let mut receiver: Receiver = DesktopReceiverOwner::new(owner, pipeline, Some(counters));

        assert!(receiver.local_media_addr().unwrap().port() != 0);
        assert!(matches!(
            receiver.poll_media(),
            Ok(crate::UdpPollOutcome::Empty)
        ));
        assert!(receiver.decode_media(false).is_ok());
        let tick = receiver.tick_media().unwrap();
        assert!(matches!(tick.poll(), crate::UdpPollOutcome::Empty));
        let _decode = tick.decode();
    }
}
