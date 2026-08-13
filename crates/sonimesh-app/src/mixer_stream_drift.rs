//! Control-domain per-stream drift for the mixer session path.
//!
//! `MixerStreamDriftController` mirrors [`crate::StreamDriftController`] for
//! the Phase 2 mixer path: it samples only the bounded adapter source queue
//! (the queue the decode-domain `StreamFormatAdapter` reads), publishes
//! ordinary ppm changes to that stream's adapter resampler, and retires the
//! stream's partial conversion state when occupancy reaches an emergency
//! threshold. The mixer callback is never touched; the controller is driven at
//! a low fixed cadence by the same control loop that decodes and pumps. Call
//! [`Self::reset`] after [`MixerStreamSession::recover_stream`] installs a
//! fresh nominal adapter so a stale pre-recovery correction is not re-applied.

use std::sync::Arc;

use sonimesh_audio::{DriftConfig, DriftConfigError, DriftController, DriftDecision};
use sonimesh_diagnostics::StreamCounters;

use crate::{MediaStreamIdentity, MixerStreamSession, MixerStreamSessionError};

/// Control-domain owner that applies one stream's bounded drift decisions.
pub struct MixerStreamDriftController {
    identity: MediaStreamIdentity,
    controller: DriftController,
    counters: Option<Arc<StreamCounters>>,
}

impl MixerStreamDriftController {
    /// Creates a controller bound to one admitted mixer stream.
    ///
    /// Pass the same [`StreamCounters`] endpoint that the session's stream
    /// records so drift decisions publish truthful per-stream diagnostics.
    ///
    /// # Errors
    ///
    /// Returns [`DriftConfigError`] when the supplied controller bounds are
    /// invalid or cannot preserve a safe occupancy range.
    pub fn new(
        identity: MediaStreamIdentity,
        config: DriftConfig,
        counters: Option<Arc<StreamCounters>>,
    ) -> Result<Self, DriftConfigError> {
        Ok(Self {
            identity,
            controller: DriftController::new(config)?,
            counters,
        })
    }

    /// Samples the stream's adapter source occupancy and applies the decision.
    ///
    /// Ordinary adjustments publish to the stream's format adapter through the
    /// session; an emergency threshold retires the adapter's partial
    /// conversion state so stale samples cannot cross the resync boundary. Each
    /// decision also publishes the sampled occupancy and the drift-specific
    /// metrics into the stream's counters endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`MixerStreamSessionError::UnknownStream`] when the stream was
    /// removed, or a session error when an adjustment or reset cannot be
    /// applied. A valid [`DriftConfig`] cannot produce an out-of-bound
    /// adjustment.
    pub fn update<O, const SLOTS: usize, const OUTPUT_SAMPLES_PER_BLOCK: usize>(
        &mut self,
        session: &mut MixerStreamSession<O, SLOTS, OUTPUT_SAMPLES_PER_BLOCK>,
    ) -> Result<DriftDecision, MixerStreamSessionError>
    where
        O: sonimesh_platform::AudioOutput,
    {
        let occupancy_blocks =
            u16::try_from(session.queued_source_blocks(self.identity)?).unwrap_or(u16::MAX);
        let decision = self.controller.update(occupancy_blocks);
        match decision {
            DriftDecision::Adjust { ppm } => {
                session.set_rate_adjustment_ppm(self.identity, ppm)?;
            }
            DriftDecision::HardResync { .. } => {
                session.begin_epoch(self.identity)?;
            }
        }
        if let Some(counters) = self.counters.as_ref() {
            counters.set_queued_pcm_blocks(occupancy_blocks);
            match decision {
                DriftDecision::Adjust { ppm } => counters.set_drift_adjustment_ppm(ppm),
                DriftDecision::HardResync { .. } => {
                    counters.set_drift_adjustment_ppm(0);
                    counters.drift_hard_resync();
                }
            }
        }
        Ok(decision)
    }

    /// Returns the latest bounded correction requested by the controller.
    #[must_use]
    pub const fn adjustment_ppm(&self) -> i32 {
        self.controller.adjustment_ppm()
    }

    /// Clears controller history after an external stream reset.
    ///
    /// Call this after [`MixerStreamSession::recover_stream`] installs a fresh
    /// nominal adapter so the stale pre-recovery correction and integral
    /// history are not re-applied by the next update. The published drift
    /// correction returns to nominal so diagnostics match the fresh adapter.
    pub fn reset(&mut self) {
        self.controller.reset();
        if let Some(counters) = self.counters.as_ref() {
            counters.set_drift_adjustment_ppm(0);
        }
    }

    /// Returns the stream identity this controller addresses.
    #[must_use]
    pub const fn identity(&self) -> MediaStreamIdentity {
        self.identity
    }
}
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sonimesh_audio::{
        DriftConfig, DriftConfigError, DriftDecision, DriftResyncDirection, RealtimePcmProducer,
        RealtimePcmPushOutcome,
    };
    use sonimesh_diagnostics::StreamCounters;
    use sonimesh_platform::{
        AudioOutput, OutputConfig, OutputDeviceId, OutputDeviceInfo, OutputError, OutputHealth,
        RenderSource, SampleFormat,
    };

    use super::MixerStreamDriftController;
    use crate::{
        MediaStreamIdentity, MixerStreamSession, MixerStreamSessionError, OutputSelection,
        ReceiverPcmProducerInstaller, StreamStartRequest,
    };

    const SLOTS: usize = 2;
    const OUTPUT_SAMPLES: usize = 256;
    const SOURCE_SAMPLES: usize = 480;

    const CONFIG: DriftConfig = DriftConfig {
        target_blocks: 4,
        low_resync_blocks: 0,
        high_resync_blocks: 8,
        proportional_ppm_per_block: 100,
        integral_ppm_per_block_tick: 10,
        max_adjustment_ppm: 500,
        max_step_ppm: 50,
        max_integral_block_ticks: 16,
    };

    struct FakeOutput {
        started: bool,
        stopped: bool,
        health: OutputHealth,
        fail_start: bool,
    }

    impl Default for FakeOutput {
        fn default() -> Self {
            Self {
                started: false,
                stopped: false,
                health: OutputHealth::Healthy,
                fail_start: false,
            }
        }
    }

    impl AudioOutput for FakeOutput {
        fn devices(&self) -> Result<Vec<OutputDeviceInfo>, OutputError> {
            Ok(Vec::new())
        }

        fn start(
            &mut self,
            _device: Option<&OutputDeviceId>,
            _config: OutputConfig,
            _source: Box<dyn RenderSource>,
        ) -> Result<(), OutputError> {
            if self.fail_start {
                return Err(OutputError::DeviceUnavailable);
            }
            self.started = true;
            self.stopped = false;
            Ok(())
        }

        fn stop(&mut self) {
            self.started = false;
            self.stopped = true;
        }

        fn health(&self) -> OutputHealth {
            self.health
        }
    }

    struct FakePipeline<const S: usize> {
        replaced: usize,
        producer: Option<RealtimePcmProducer<S>>,
    }

    impl<const S: usize> Default for FakePipeline<S> {
        fn default() -> Self {
            Self {
                replaced: 0,
                producer: None,
            }
        }
    }

    impl<const S: usize> ReceiverPcmProducerInstaller<S> for FakePipeline<S> {
        fn replace_pcm_producer(&mut self, producer: RealtimePcmProducer<S>) {
            self.replaced += 1;
            self.producer = Some(producer);
        }
    }

    fn selection() -> OutputSelection {
        OutputSelection {
            device: None,
            config: OutputConfig {
                sample_rate_hz: 48_000,
                channels: 2,
                sample_format: SampleFormat::Float32,
                requested_callback_frames: Some(480),
                maximum_callback_frames: 1_024,
            },
        }
    }

    fn identity(stream_id: u32) -> MediaStreamIdentity {
        MediaStreamIdentity {
            session_id: 0x0102_0304,
            stream_id,
            key_epoch: 0,
        }
    }

    fn request(identity: MediaStreamIdentity) -> StreamStartRequest {
        StreamStartRequest {
            identity,
            sample_rate_hz: 48_000,
            channels: 1,
            frame_duration_ms: 10,
        }
    }

    type Session = MixerStreamSession<FakeOutput, SLOTS, OUTPUT_SAMPLES>;

    fn session(
        counters: &Arc<StreamCounters>,
        pipeline: &mut FakePipeline<SOURCE_SAMPLES>,
    ) -> Session {
        let mut session =
            MixerStreamSession::new(FakeOutput::default(), 8).expect("fixed test session is valid");
        session.start_output(selection()).unwrap();
        session
            .start_stream(
                request(identity(1)),
                8,
                Some(Arc::clone(counters)),
                pipeline,
            )
            .unwrap();
        session
    }

    fn push_blocks(pipeline: &mut FakePipeline<SOURCE_SAMPLES>, count: usize) {
        let producer = pipeline.producer.as_mut().unwrap();
        for _ in 0..count {
            assert_eq!(
                producer.push(&[0.5; SOURCE_SAMPLES]),
                RealtimePcmPushOutcome::Inserted
            );
        }
    }

    #[test]
    fn new_rejects_invalid_controller_bounds() {
        let mut config = CONFIG;
        config.low_resync_blocks = config.target_blocks;

        assert!(matches!(
            MixerStreamDriftController::new(identity(1), config, None),
            Err(DriftConfigError::InvalidOccupancyThresholds)
        ));
    }

    #[test]
    fn identity_and_adjustment_ppm_are_exposed() {
        let drift = MixerStreamDriftController::new(identity(1), CONFIG, None).unwrap();
        assert_eq!(drift.identity(), identity(1));
        assert_eq!(drift.adjustment_ppm(), 0);
    }

    #[test]
    fn update_publishes_a_positive_adjustment_and_metrics() {
        let counters = Arc::new(StreamCounters::default());
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        let mut session = session(&counters, &mut pipeline);
        let mut drift =
            MixerStreamDriftController::new(identity(1), CONFIG, Some(Arc::clone(&counters)))
                .unwrap();

        push_blocks(&mut pipeline, 6);
        assert_eq!(session.queued_source_blocks(identity(1)).unwrap(), 6);

        assert_eq!(
            drift.update(&mut session).unwrap(),
            DriftDecision::Adjust { ppm: 50 }
        );
        assert_eq!(drift.adjustment_ppm(), 50);
        let snapshot = counters.snapshot();
        assert_eq!(snapshot.queued_pcm_blocks, 6);
        assert_eq!(snapshot.drift_adjustment_ppm, 50);
    }

    #[test]
    fn update_high_side_hard_resync_retires_conversion_state() {
        let counters = Arc::new(StreamCounters::default());
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        let mut session = session(&counters, &mut pipeline);
        let mut drift =
            MixerStreamDriftController::new(identity(1), CONFIG, Some(Arc::clone(&counters)))
                .unwrap();

        push_blocks(&mut pipeline, 8);
        assert_eq!(
            drift.update(&mut session).unwrap(),
            DriftDecision::HardResync {
                direction: DriftResyncDirection::High,
            }
        );
        assert_eq!(drift.adjustment_ppm(), 0);
        let snapshot = counters.snapshot();
        assert_eq!(snapshot.queued_pcm_blocks, 8);
        assert_eq!(snapshot.drift_hard_resyncs, 1);
        assert_eq!(snapshot.drift_adjustment_ppm, 0);
    }

    #[test]
    fn update_low_side_hard_resync_on_an_empty_queue() {
        let counters = Arc::new(StreamCounters::default());
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        let mut session = session(&counters, &mut pipeline);
        let mut drift =
            MixerStreamDriftController::new(identity(1), CONFIG, Some(Arc::clone(&counters)))
                .unwrap();

        assert_eq!(session.queued_source_blocks(identity(1)).unwrap(), 0);
        assert_eq!(
            drift.update(&mut session).unwrap(),
            DriftDecision::HardResync {
                direction: DriftResyncDirection::Low,
            }
        );
        let snapshot = counters.snapshot();
        assert_eq!(snapshot.queued_pcm_blocks, 0);
        assert_eq!(snapshot.drift_hard_resyncs, 1);
        assert_eq!(snapshot.drift_adjustment_ppm, 0);
    }

    #[test]
    fn update_unknown_stream_is_reported() {
        let counters = Arc::new(StreamCounters::default());
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        let mut session = session(&counters, &mut pipeline);
        let mut drift = MixerStreamDriftController::new(identity(9), CONFIG, None).unwrap();

        assert_eq!(
            drift.update(&mut session),
            Err(MixerStreamSessionError::UnknownStream(identity(9)))
        );
    }

    #[test]
    fn reset_clears_history_and_published_metrics() {
        let counters = Arc::new(StreamCounters::default());
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        let mut session = session(&counters, &mut pipeline);
        let mut drift =
            MixerStreamDriftController::new(identity(1), CONFIG, Some(Arc::clone(&counters)))
                .unwrap();

        push_blocks(&mut pipeline, 6);
        drift.update(&mut session).unwrap();
        assert_eq!(drift.adjustment_ppm(), 50);

        drift.reset();
        assert_eq!(drift.adjustment_ppm(), 0);
        assert_eq!(counters.snapshot().drift_adjustment_ppm, 0);
    }
}
