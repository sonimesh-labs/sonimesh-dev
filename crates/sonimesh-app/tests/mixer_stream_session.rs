//! Mixer stream session tests: adapted admission through a real decode
//! pipeline, removal, output recovery with re-admission, and fail-closed
//! validation.

use core::convert::Infallible;
use std::sync::Arc;

use sonimesh_app::{
    MediaStreamIdentity, MixerStreamSession, MixerStreamSessionError, StreamStartRequest,
};
use sonimesh_audio::{FrameDecoder, realtime_pcm_queue};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceInfo, OutputError, OutputHealth, RenderSource,
    SampleFormat,
};
use sonimesh_protocol::{MediaDirection, MediaKeyContext, MediaSealer, SessionSecret};

const SLOTS: usize = 4;
const BLOCK: usize = 8;

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

/// Seals, receives, decodes, pumps, and renders one output block.
fn feed_cycle(
    session: &mut Session,
    pipeline: &mut Pipeline,
    sealer: &mut MediaSealer,
    sequence: u32,
) -> [f32; BLOCK] {
    let payload = [0x2A; 8];
    let packet = sealer.seal(0, u64::from(sequence) * 8, &payload).unwrap();
    let mut plaintext = [0_u8; 8];
    pipeline.receive(&packet.datagram, &mut plaintext).unwrap();
    pipeline.decode_next().unwrap();
    session.pump_all().unwrap();
    let mut rendered = [0.0_f32; BLOCK];
    session.output_mut().render(&mut rendered);
    rendered
}

#[test]
fn adapted_stream_renders_through_the_session() {
    let counters = Arc::new(StreamCounters::default());
    let mut session = session();
    let mut pipeline = pipeline(Arc::clone(&counters));
    let mut sealer = sealer();

    session.start_output(selection()).unwrap();
    let started = session
        .start_stream::<_, BLOCK>(
            request(1, 1),
            16,
            Some(Arc::clone(&counters)),
            &mut pipeline,
        )
        .unwrap();
    assert_eq!(started.slot, 0);
    assert_eq!(session.stream_count(), 1);

    let rendered = feed_cycle(&mut session, &mut pipeline, &mut sealer, 0);
    assert!(
        rendered.iter().any(|sample| *sample > 0.0),
        "a decoded and adapted stream must render through the mixer"
    );
    assert!(session.snapshot().stream_active());
}

#[test]
fn removing_the_stream_renders_silence() {
    let counters = Arc::new(StreamCounters::default());
    let mut session = session();
    let mut pipeline = pipeline(Arc::clone(&counters));
    let mut sealer = sealer();

    session.start_output(selection()).unwrap();
    session
        .start_stream::<_, BLOCK>(request(1, 1), 16, None, &mut pipeline)
        .unwrap();
    let rendered = feed_cycle(&mut session, &mut pipeline, &mut sealer, 0);
    assert!(rendered.iter().any(|sample| *sample > 0.0));

    session.remove_stream(request(1, 1).identity).unwrap();
    assert_eq!(session.stream_count(), 0);
    let rendered = feed_cycle(&mut session, &mut pipeline, &mut sealer, 1);
    assert!(
        rendered.iter().all(|sample| *sample == 0.0),
        "a removed stream must render silence"
    );
}

#[test]
fn admission_validation_fails_closed() {
    let counters = Arc::new(StreamCounters::default());
    let mut session = session();
    let mut pipeline = pipeline(Arc::clone(&counters));

    assert!(matches!(
        session.start_stream::<_, BLOCK>(request(1, 1), 16, None, &mut pipeline),
        Err(MixerStreamSessionError::OutputNotActive)
    ));

    session.start_output(selection()).unwrap();

    let mut wrong_block = request(1, 1);
    wrong_block.sample_rate_hz = 16_000;
    assert!(matches!(
        session.start_stream::<_, BLOCK>(wrong_block, 16, None, &mut pipeline),
        Err(MixerStreamSessionError::SourceBlockMismatch { expected: 16 })
    ));

    let mut non_integral = request(1, 1);
    non_integral.sample_rate_hz = 8_001;
    assert!(matches!(
        session.start_stream::<_, BLOCK>(non_integral, 16, None, &mut pipeline),
        Err(MixerStreamSessionError::NonIntegralFrameBlock)
    ));

    let mut absurd = request(1, 1);
    absurd.frame_duration_ms = u32::MAX;
    absurd.sample_rate_hz = 48_000;
    assert!(matches!(
        session.start_stream::<_, BLOCK>(absurd, 16, None, &mut pipeline),
        Err(MixerStreamSessionError::SourceBlockMismatch {
            expected: 206_158_430_160
        })
    ));

    session
        .start_stream::<_, BLOCK>(request(1, 1), 16, None, &mut pipeline)
        .unwrap();
    assert!(matches!(
        session.start_stream::<_, BLOCK>(request(1, 1), 16, None, &mut pipeline),
        Err(MixerStreamSessionError::DuplicateStream(_))
    ));
    assert!(matches!(
        session.remove_stream(MediaStreamIdentity {
            session_id: 9,
            stream_id: 9,
            key_epoch: 0,
        }),
        Err(MixerStreamSessionError::UnknownStream(_))
    ));
    assert_eq!(session.stream_count(), 1);
}

#[test]
fn rotation_replaces_identity_keeps_slot_gain_and_renders() {
    let counters = Arc::new(StreamCounters::default());
    let mut session = session();
    let mut pipeline = pipeline(Arc::clone(&counters));
    let mut sealer = sealer();

    session.start_output(selection()).unwrap();
    let started = session
        .start_stream::<_, BLOCK>(
            request(1, 1),
            16,
            Some(Arc::clone(&counters)),
            &mut pipeline,
        )
        .unwrap();
    assert_eq!(started.slot, 0);

    let mut rotated_request = request(1, 1);
    rotated_request.identity.key_epoch = 1;
    let rotated = session
        .rotate_stream(request(1, 1).identity, rotated_request)
        .unwrap();
    assert_eq!(rotated.slot, 0, "rotation must keep the same slot");
    assert_eq!(session.stream_count(), 1);
    assert!(
        session.gain(rotated_request.identity).is_ok(),
        "rotation keeps the gain handle for the new identity"
    );
    assert!(matches!(
        session.gain(request(1, 1).identity),
        Err(MixerStreamSessionError::UnknownStream(_))
    ));

    let rendered = feed_cycle(&mut session, &mut pipeline, &mut sealer, 0);
    assert!(
        rendered.iter().any(|sample| *sample > 0.0),
        "a rotated stream must keep rendering after the epoch reset"
    );
}

#[test]
fn rotation_with_changed_frame_budget_is_rejected() {
    let counters = Arc::new(StreamCounters::default());
    let mut session = session();
    let mut pipeline = pipeline(Arc::clone(&counters));

    session.start_output(selection()).unwrap();
    session
        .start_stream::<_, BLOCK>(request(1, 1), 16, None, &mut pipeline)
        .unwrap();

    let mut changed = request(1, 1);
    changed.identity.key_epoch = 1;
    changed.frame_duration_ms = 2; // 8_000 Hz x 2 ms = 16 frames per block
    assert_eq!(
        session.rotate_stream(request(1, 1).identity, changed),
        Err(MixerStreamSessionError::RotationFormatChange)
    );
    assert_eq!(session.stream_count(), 1);
    assert!(
        session.gain(request(1, 1).identity).is_ok(),
        "a rejected rotation must not change the admitted identity"
    );
}

#[test]
fn rotation_with_changed_channel_count_is_rejected() {
    let counters = Arc::new(StreamCounters::default());
    let mut session = session();
    let mut pipeline = pipeline(Arc::clone(&counters));

    session.start_output(selection()).unwrap();
    session
        .start_stream::<_, BLOCK>(request(1, 1), 16, None, &mut pipeline)
        .unwrap();

    let mut changed = request(1, 1);
    changed.identity.key_epoch = 1;
    changed.channels = 2;
    assert_eq!(
        session.rotate_stream(request(1, 1).identity, changed),
        Err(MixerStreamSessionError::RotationFormatChange)
    );
    assert_eq!(session.stream_count(), 1);
}

#[test]
fn rotation_to_an_already_active_identity_is_rejected() {
    let counters_a = Arc::new(StreamCounters::default());
    let counters_b = Arc::new(StreamCounters::default());
    let mut session = session();
    let mut pipeline_a = pipeline(Arc::clone(&counters_a));
    let mut pipeline_b = pipeline(Arc::clone(&counters_b));

    session.start_output(selection()).unwrap();
    session
        .start_stream::<_, BLOCK>(request(1, 1), 16, None, &mut pipeline_a)
        .unwrap();
    session
        .start_stream::<_, BLOCK>(request(2, 2), 16, None, &mut pipeline_b)
        .unwrap();

    let duplicate_target = request(2, 2);
    assert_eq!(
        session.rotate_stream(request(1, 1).identity, duplicate_target),
        Err(MixerStreamSessionError::DuplicateStream(
            duplicate_target.identity
        ))
    );
    assert_eq!(session.stream_count(), 2);
}

#[test]
fn rotation_of_an_unknown_stream_is_rejected() {
    let counters = Arc::new(StreamCounters::default());
    let mut session = session();
    let mut pipeline = pipeline(Arc::clone(&counters));

    session.start_output(selection()).unwrap();
    session
        .start_stream::<_, BLOCK>(request(1, 1), 16, None, &mut pipeline)
        .unwrap();

    assert_eq!(
        session.rotate_stream(request(9, 9).identity, request(9, 9)),
        Err(MixerStreamSessionError::UnknownStream(
            request(9, 9).identity
        ))
    );
    assert_eq!(session.stream_count(), 1);
}

#[test]
fn output_recovery_readmits_persisted_streams() {
    let counters = Arc::new(StreamCounters::default());
    let mut session = session();
    let mut pipeline = pipeline(Arc::clone(&counters));
    let mut sealer = sealer();

    session.start_output(selection()).unwrap();
    session
        .start_stream::<_, BLOCK>(
            request(1, 1),
            16,
            Some(Arc::clone(&counters)),
            &mut pipeline,
        )
        .unwrap();
    let rendered = feed_cycle(&mut session, &mut pipeline, &mut sealer, 0);
    assert!(rendered.iter().any(|sample| *sample > 0.0));

    session.output_mut().health = OutputHealth::StreamError;
    assert_eq!(session.monitor(), Some(OutputHealth::StreamError));
    assert_eq!(session.stream_count(), 1, "records survive the fault");
    assert_eq!(session.pending_commands(), 0);

    let identities = session.recover_output().unwrap();
    assert_eq!(identities, vec![request(1, 1).identity]);
    let started = session
        .recover_stream::<_, BLOCK>(request(1, 1).identity, &mut pipeline)
        .unwrap();
    assert_eq!(started.slot, 0);

    let rendered = feed_cycle(&mut session, &mut pipeline, &mut sealer, 2);
    assert!(
        rendered.iter().any(|sample| *sample > 0.0),
        "a recovered stream must render after fresh producer installation"
    );
    assert_eq!(session.stream_count(), 1);
    assert!(!session.snapshot().fault_reported());
    assert_eq!(session.output().start_calls, 2);
}

#[test]
fn underflow_of_one_stream_does_not_block_others() {
    let counters_a = Arc::new(StreamCounters::default());
    let counters_b = Arc::new(StreamCounters::default());
    let mut session = session();
    let mut pipeline_a = pipeline(Arc::clone(&counters_a));
    let mut pipeline_b = pipeline(Arc::clone(&counters_b));
    let mut sealer_a = sealer();

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
    assert_eq!(session.stream_count(), 2);

    // Feed only stream a; stream b's slot must underflow without blocking a.
    let rendered = feed_cycle(&mut session, &mut pipeline_a, &mut sealer_a, 0);
    assert!(rendered.iter().any(|sample| *sample > 0.0));
    assert_eq!(counters_a.snapshot().pcm_underflows, 0);
    assert!(
        counters_b.snapshot().pcm_underflows >= 1,
        "the starved slot must record its own underflow"
    );
}

#[test]
fn start_output_is_rejected_while_active() {
    let mut session = session();
    session.start_output(selection()).unwrap();
    assert!(matches!(
        session.start_output(selection()),
        Err(MixerStreamSessionError::Output(
            sonimesh_app::MixerOutputLifecycleError::StreamAlreadyActive
        ))
    ));
}

#[test]
fn stop_clears_records_and_allows_a_fresh_lifetime() {
    let counters = Arc::new(StreamCounters::default());
    let mut session = session();
    let mut pipeline = pipeline(Arc::clone(&counters));

    session.start_output(selection()).unwrap();
    session
        .start_stream::<_, BLOCK>(request(1, 1), 16, None, &mut pipeline)
        .unwrap();
    assert_eq!(session.stream_count(), 1);

    session.stop();
    assert_eq!(session.stream_count(), 0);
    assert_eq!(session.pending_commands(), 0);
    assert_eq!(session.output().stop_calls, 1);

    session.output_mut().health = OutputHealth::Healthy;
    session.start_output(selection()).unwrap();
    session
        .start_stream::<_, BLOCK>(request(1, 1), 16, None, &mut pipeline)
        .unwrap();
    assert_eq!(session.stream_count(), 1);
}

#[test]
fn session_exposes_queue_diagnostics() {
    let counters = Arc::new(StreamCounters::default());
    let mut session = session();
    let mut pipeline = pipeline(Arc::clone(&counters));
    let mut sealer = sealer();

    session.start_output(selection()).unwrap();
    session
        .start_stream::<_, BLOCK>(request(1, 1), 16, None, &mut pipeline)
        .unwrap();
    let identity = request(1, 1).identity;

    assert_eq!(session.queued_source_blocks(identity).unwrap(), 0);
    let payload = [0x2A; 8];
    let packet = sealer.seal(0, 0, &payload).unwrap();
    let mut plaintext = [0_u8; 8];
    pipeline.receive(&packet.datagram, &mut plaintext).unwrap();
    pipeline.decode_next().unwrap();
    assert!(
        session.queued_source_blocks(identity).unwrap() >= 1,
        "decoded source blocks must be visible before the pump"
    );

    session.pump_all().unwrap();
    assert_eq!(session.queued_source_blocks(identity).unwrap(), 0);
    assert!(
        session.queued_output_blocks(identity).unwrap() >= 1,
        "converted output blocks must be visible after the pump"
    );
}
