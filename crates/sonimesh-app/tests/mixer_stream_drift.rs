//! Control-domain per-stream drift tests for the mixer session path.
//!
//! These tests drive `MixerStreamDriftController` through a real
//! `MixerStreamSession`: ordinary occupancy errors publish bounded ppm
//! corrections to the stream's format adapter, emergency thresholds retire the
//! adapter epoch and clear the correction, each stream's controller stays
//! isolated from the others, and decisions publish truthful per-stream drift
//! metrics.

use core::convert::Infallible;
use std::sync::Arc;

use sonimesh_app::{
    MediaStreamIdentity, MixerStreamDriftController, MixerStreamSession, MixerStreamSessionError,
    StreamStartRequest,
};
use sonimesh_audio::{
    DriftConfig, DriftConfigError, DriftDecision, DriftResyncDirection, FrameDecoder,
    realtime_pcm_queue,
};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceInfo, OutputError, OutputHealth, RenderSource,
    SampleFormat,
};
use sonimesh_protocol::{MediaDirection, MediaKeyContext, MediaSealer, SessionSecret};

const SLOTS: usize = 4;
const BLOCK: usize = 8;

const CONFIG: DriftConfig = DriftConfig {
    target_blocks: 2,
    low_resync_blocks: 0,
    high_resync_blocks: 4,
    proportional_ppm_per_block: 100,
    integral_ppm_per_block_tick: 10,
    max_adjustment_ppm: 500,
    max_step_ppm: 500,
    max_integral_block_ticks: 16,
};

type Session = MixerStreamSession<TestOutput, SLOTS, BLOCK>;
type Pipeline = sonimesh_app::ReceiverStreamPipeline<ConstantDecoder, 4, 8, BLOCK>;

/// Decoder that fills each whole PCM block with a constant sample.
struct ConstantDecoder(f32);

impl FrameDecoder for ConstantDecoder {
    type Error = Infallible;

    fn decode(&mut self, _encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
        output.fill(self.0);
        Ok(output.len())
    }

    fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
        output.fill(0.0);
        Ok(output.len())
    }
}

struct TestOutput {
    health: OutputHealth,
    start_calls: usize,
    stop_calls: usize,
    fail_start: bool,
    source: Option<Box<dyn RenderSource>>,
}

impl TestOutput {
    fn render(&mut self, output: &mut [f32]) {
        self.source
            .as_mut()
            .expect("test output never captured a source")
            .render(output);
    }
}

impl AudioOutput for TestOutput {
    fn devices(&self) -> Result<Vec<OutputDeviceInfo>, OutputError> {
        Ok(Vec::new())
    }

    fn start(
        &mut self,
        _device: Option<&sonimesh_platform::OutputDeviceId>,
        _config: OutputConfig,
        source: Box<dyn RenderSource>,
    ) -> Result<(), OutputError> {
        self.start_calls += 1;
        if self.fail_start {
            return Err(OutputError::Backend("test start failure".to_owned()));
        }
        self.source = Some(source);
        Ok(())
    }

    fn stop(&mut self) {
        self.stop_calls += 1;
    }

    fn health(&self) -> OutputHealth {
        self.health
    }
}

fn selection() -> sonimesh_app::OutputSelection {
    sonimesh_app::OutputSelection {
        device: None,
        config: OutputConfig {
            sample_rate_hz: 48_000,
            channels: 1,
            sample_format: SampleFormat::Float32,
            requested_callback_frames: Some(480),
            maximum_callback_frames: 480,
        },
    }
}

fn context(session_id: u64, stream_id: u32) -> MediaKeyContext {
    MediaKeyContext::new(session_id, stream_id, 0, MediaDirection::AndroidToDesktop).unwrap()
}

fn request(session_id: u64, stream_id: u32) -> StreamStartRequest {
    StreamStartRequest {
        identity: MediaStreamIdentity {
            session_id,
            stream_id,
            key_epoch: 0,
        },
        // 8_000 Hz x 1 ms = 8 frames per block, matching BLOCK.
        sample_rate_hz: 8_000,
        channels: 1,
        frame_duration_ms: 1,
    }
}

fn session() -> Session {
    MixerStreamSession::new(
        TestOutput {
            health: OutputHealth::Healthy,
            start_calls: 0,
            stop_calls: 0,
            fail_start: false,
            source: None,
        },
        16,
    )
    .unwrap()
}

fn pipeline(counters: Arc<StreamCounters>) -> Pipeline {
    let secret = SessionSecret::new([0xA5; 32]);
    let (producer, _consumer) = realtime_pcm_queue::<BLOCK>(4).unwrap();
    sonimesh_app::ReceiverStreamPipeline::new(
        &secret,
        context(1, 1),
        ConstantDecoder(0.25),
        producer,
        counters,
        0,
    )
    .unwrap()
}

fn sealer() -> MediaSealer {
    MediaSealer::new(&SessionSecret::new([0xA5; 32]), context(1, 1)).unwrap()
}

/// Seals, receives, and decodes `count` blocks without pumping them.
fn feed_blocks(pipeline: &mut Pipeline, sealer: &mut MediaSealer, base_sequence: u32, count: u32) {
    let payload = [0x2A; 8];
    for offset in 0..count {
        let sequence = base_sequence + offset;
        let packet = sealer.seal(0, u64::from(sequence) * 8, &payload).unwrap();
        let mut plaintext = [0_u8; 8];
        pipeline.receive(&packet.datagram, &mut plaintext).unwrap();
        pipeline.decode_next().unwrap();
    }
}

#[test]
fn control_tick_publishes_ppm_through_the_session_adapter() {
    let counters = Arc::new(StreamCounters::default());
    let mut session = session();
    let mut pipeline = pipeline(Arc::clone(&counters));
    let mut sealer = sealer();
    let identity = request(1, 1).identity;

    session.start_output(selection()).unwrap();
    session
        .start_stream::<_, BLOCK>(
            request(1, 1),
            16,
            Some(Arc::clone(&counters)),
            &mut pipeline,
        )
        .unwrap();
    let mut controller = MixerStreamDriftController::new(identity, CONFIG, None).unwrap();
    assert_eq!(controller.identity(), identity);

    feed_blocks(&mut pipeline, &mut sealer, 0, 3);
    assert_eq!(session.queued_source_blocks(identity).unwrap(), 3);

    assert_eq!(
        controller.update(&mut session),
        Ok(DriftDecision::Adjust { ppm: 110 })
    );
    assert_eq!(controller.adjustment_ppm(), 110);

    // The adjusted adapter still converts and renders for the stream.
    session.pump_all().unwrap();
    let mut rendered = [0.0_f32; BLOCK];
    session.output_mut().render(&mut rendered);
    assert!(
        rendered.iter().any(|sample| *sample > 0.0),
        "a ppm-adjusted stream must keep rendering"
    );
}

#[test]
fn high_emergency_resync_retires_the_adapter_epoch_and_clears_ppm() {
    let counters = Arc::new(StreamCounters::default());
    let mut session = session();
    let mut pipeline = pipeline(Arc::clone(&counters));
    let mut sealer = sealer();
    let identity = request(1, 1).identity;

    session.start_output(selection()).unwrap();
    session
        .start_stream::<_, BLOCK>(request(1, 1), 16, None, &mut pipeline)
        .unwrap();
    let mut controller = MixerStreamDriftController::new(identity, CONFIG, None).unwrap();

    feed_blocks(&mut pipeline, &mut sealer, 0, 4);
    assert_eq!(session.queued_source_blocks(identity).unwrap(), 4);

    assert_eq!(
        controller.update(&mut session),
        Ok(DriftDecision::HardResync {
            direction: DriftResyncDirection::High,
        })
    );
    assert_eq!(controller.adjustment_ppm(), 0);
    assert_eq!(
        session.queued_output_blocks(identity).unwrap(),
        0,
        "the retired epoch must not leave queued output blocks"
    );

    // The adapter remains functional for the fresh epoch.
    session.pump_all().unwrap();
    let mut rendered = [0.0_f32; BLOCK];
    session.output_mut().render(&mut rendered);
    assert!(
        rendered.iter().any(|sample| *sample > 0.0),
        "the stream must render again after a high-side resync"
    );
}

#[test]
fn low_emergency_resync_clears_ppm_and_adjustment_resumes() {
    let counters = Arc::new(StreamCounters::default());
    let mut session = session();
    let mut pipeline = pipeline(Arc::clone(&counters));
    let mut sealer = sealer();
    let identity = request(1, 1).identity;

    session.start_output(selection()).unwrap();
    session
        .start_stream::<_, BLOCK>(request(1, 1), 16, None, &mut pipeline)
        .unwrap();
    let mut controller = MixerStreamDriftController::new(identity, CONFIG, None).unwrap();

    assert_eq!(session.queued_source_blocks(identity).unwrap(), 0);
    assert_eq!(
        controller.update(&mut session),
        Ok(DriftDecision::HardResync {
            direction: DriftResyncDirection::Low,
        })
    );
    assert_eq!(controller.adjustment_ppm(), 0);

    // One decoded block below the target resumes a bounded slow-down.
    feed_blocks(&mut pipeline, &mut sealer, 0, 1);
    assert_eq!(
        controller.update(&mut session),
        Ok(DriftDecision::Adjust { ppm: -110 })
    );
}

#[test]
fn one_stream_resync_does_not_disturb_another_streams_drift() {
    let counters_a = Arc::new(StreamCounters::default());
    let counters_b = Arc::new(StreamCounters::default());
    let mut session = session();
    let mut pipeline_a = pipeline(Arc::clone(&counters_a));
    let mut pipeline_b = pipeline(Arc::clone(&counters_b));
    let mut sealer_a = sealer();
    let mut sealer_b = sealer();
    let identity_a = request(1, 1).identity;
    let identity_b = request(2, 1).identity;

    session.start_output(selection()).unwrap();
    session
        .start_stream::<_, BLOCK>(
            request(1, 1),
            16,
            Some(Arc::clone(&counters_a)),
            &mut pipeline_a,
        )
        .unwrap();
    session
        .start_stream::<_, BLOCK>(
            request(2, 1),
            16,
            Some(Arc::clone(&counters_b)),
            &mut pipeline_b,
        )
        .unwrap();
    let mut controller_a = MixerStreamDriftController::new(identity_a, CONFIG, None).unwrap();
    let mut controller_b = MixerStreamDriftController::new(identity_b, CONFIG, None).unwrap();

    feed_blocks(&mut pipeline_a, &mut sealer_a, 0, 3);
    feed_blocks(&mut pipeline_b, &mut sealer_b, 0, 3);
    assert_eq!(
        controller_a.update(&mut session).unwrap(),
        DriftDecision::Adjust { ppm: 110 }
    );
    assert_eq!(
        controller_b.update(&mut session).unwrap(),
        DriftDecision::Adjust { ppm: 110 }
    );

    // Stream a crosses the high threshold; stream b must be untouched.
    feed_blocks(&mut pipeline_a, &mut sealer_a, 3, 1);
    assert_eq!(
        controller_a.update(&mut session).unwrap(),
        DriftDecision::HardResync {
            direction: DriftResyncDirection::High,
        }
    );
    assert_eq!(controller_a.adjustment_ppm(), 0);
    assert_eq!(session.queued_source_blocks(identity_b).unwrap(), 3);

    assert!(matches!(
        controller_b.update(&mut session).unwrap(),
        DriftDecision::Adjust { ppm } if ppm > 0
    ));
    assert_eq!(session.queued_source_blocks(identity_b).unwrap(), 3);
    assert_eq!(
        session.queued_output_blocks(identity_a).unwrap(),
        0,
        "stream a's epoch retirement must not touch stream b"
    );
}

#[test]
fn update_after_removal_returns_unknown_stream() {
    let counters = Arc::new(StreamCounters::default());
    let mut session = session();
    let mut pipeline = pipeline(Arc::clone(&counters));
    let identity = request(1, 1).identity;

    session.start_output(selection()).unwrap();
    session
        .start_stream::<_, BLOCK>(request(1, 1), 16, None, &mut pipeline)
        .unwrap();
    let mut controller = MixerStreamDriftController::new(identity, CONFIG, None).unwrap();

    session.remove_stream(identity).unwrap();
    assert_eq!(
        controller.update(&mut session),
        Err(MixerStreamSessionError::UnknownStream(identity))
    );
}

#[test]
fn reset_clears_history_for_a_recovered_stream() {
    let counters = Arc::new(StreamCounters::default());
    let mut session = session();
    let mut pipeline = pipeline(Arc::clone(&counters));
    let mut sealer = sealer();
    let identity = request(1, 1).identity;

    session.start_output(selection()).unwrap();
    session
        .start_stream::<_, BLOCK>(request(1, 1), 16, None, &mut pipeline)
        .unwrap();
    let mut controller = MixerStreamDriftController::new(identity, CONFIG, None).unwrap();

    feed_blocks(&mut pipeline, &mut sealer, 0, 3);
    assert_eq!(
        controller.update(&mut session).unwrap(),
        DriftDecision::Adjust { ppm: 110 }
    );

    // recover_stream installs a fresh nominal adapter; reset clears the stale
    // pre-recovery correction and integral history.
    controller.reset();
    assert_eq!(controller.adjustment_ppm(), 0);
    assert_eq!(
        controller.update(&mut session).unwrap(),
        DriftDecision::Adjust { ppm: 110 }
    );
}

#[test]
fn control_ticks_publish_per_stream_drift_metrics() {
    let counters = Arc::new(StreamCounters::default());
    let mut session = session();
    let mut pipeline = pipeline(Arc::clone(&counters));
    let mut sealer = sealer();
    let identity = request(1, 1).identity;

    session.start_output(selection()).unwrap();
    session
        .start_stream::<_, BLOCK>(
            request(1, 1),
            16,
            Some(Arc::clone(&counters)),
            &mut pipeline,
        )
        .unwrap();
    let mut controller =
        MixerStreamDriftController::new(identity, CONFIG, Some(Arc::clone(&counters))).unwrap();

    feed_blocks(&mut pipeline, &mut sealer, 0, 3);
    assert_eq!(
        controller.update(&mut session).unwrap(),
        DriftDecision::Adjust { ppm: 110 }
    );
    let adjusted = counters.snapshot();
    assert_eq!(adjusted.drift_adjustment_ppm, 110);
    assert_eq!(adjusted.drift_hard_resyncs, 0);
    assert_eq!(adjusted.queued_pcm_blocks, 3);
    assert_eq!(adjusted.queued_pcm_high_water_blocks, 3);

    // reset() publishes the nominal correction so diagnostics match recovery.
    controller.reset();
    assert_eq!(counters.snapshot().drift_adjustment_ppm, 0);

    feed_blocks(&mut pipeline, &mut sealer, 3, 1);
    assert_eq!(
        controller.update(&mut session).unwrap(),
        DriftDecision::HardResync {
            direction: DriftResyncDirection::High,
        }
    );
    let resynced = counters.snapshot();
    assert_eq!(resynced.drift_adjustment_ppm, 0);
    assert_eq!(resynced.drift_hard_resyncs, 1);
    assert_eq!(
        resynced.queued_pcm_blocks, 4,
        "the adapter source queue still holds the sampled occupancy"
    );
    assert_eq!(resynced.queued_pcm_high_water_blocks, 4);
}

#[test]
fn invalid_config_is_rejected_at_construction() {
    let identity = request(1, 1).identity;
    let mut config = CONFIG;
    config.low_resync_blocks = config.target_blocks;
    assert!(matches!(
        MixerStreamDriftController::new(identity, config, None),
        Err(DriftConfigError::InvalidOccupancyThresholds)
    ));
}
