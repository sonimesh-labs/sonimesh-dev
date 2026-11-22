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
use sonimesh_protocol::Envelope;

use crate::{
    ControlSessionToken, ControlStreamAction, DesktopControlOwner, DesktopControlOwnerError,
    ReceiverPcmProducerInstaller, UdpPollOutcome, UdpReceiverError, UdpReceiverPipeline,
};

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
        let action = self.owner.accept_control(session, envelope)?;
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
    /// The caller supplies whether the preceding packet poll observed an
    /// authenticated arrival, which keeps target-gated scheduling explicit.
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
    }
}
