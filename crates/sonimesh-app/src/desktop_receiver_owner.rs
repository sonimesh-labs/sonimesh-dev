//! Desktop receiver composition that keeps the control owner and media
//! pipeline together.
//!
//! The Tauri shell and control transport can use this boundary without passing
//! a receiver pipeline through every lifecycle callback. The pipeline remains
//! injected, so this type does not choose a decoder, socket, or platform output
//! backend on behalf of the application.

use std::sync::Arc;

use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::AudioOutput;
use sonimesh_protocol::Envelope;

use crate::{
    ControlSessionToken, ControlStreamAction, DesktopControlOwner, DesktopControlOwnerError,
    ReceiverPcmProducerInstaller,
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sonimesh_audio::RealtimePcmProducer;
    use sonimesh_diagnostics::StreamCounters;
    use sonimesh_platform::{
        AudioOutput, OutputConfig, OutputDeviceId, OutputDeviceInfo, OutputError, OutputHealth,
        RenderSource, SampleFormat,
    };
    use sonimesh_protocol::{
        CONTROL_PROTOCOL_VERSION, Capability, Envelope, Hello, StartStream, envelope,
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
}
