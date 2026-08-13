//! Control-to-mixer bridge tests: the coordinator and mixer session driven as
//! one object, with output lifecycle, rollback, rotation, fault recovery, and
//! expiry.

use core::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use sonimesh_app::{
    ControlSessionToken, ControlStreamAction, ControlToMixerBridge, ControlToMixerError,
    DiagnosticExportAction, ExportRetentionPolicy, ExportStore, MixerStreamSessionError,
    PairingAttemptOutcome, PairingId, PairingInvitationController, PairingProofVerification,
    PairingSnapshotState, PairingSource, SessionEvent, SessionPhase, SessionState,
};
use sonimesh_audio::{FrameDecoder, realtime_pcm_queue};
use sonimesh_diagnostics::{ExportPlatformFormat, ExportSessionPhase, StreamCounters};
use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceInfo, OutputError, OutputHealth, RenderSource,
    SampleFormat,
};
use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, Envelope, Hello, MediaDirection, MediaKeyContext,
    MediaSealer, SessionSecret, StartStream, StopStream, envelope,
};

const SLOTS: usize = 4;
const BLOCK: usize = 480;

type Bridge = ControlToMixerBridge<TestOutput, SLOTS, BLOCK>;
type Pipeline = sonimesh_app::ReceiverStreamPipeline<ConstantDecoder, 4, 8, BLOCK>;
type ShortPipeline = sonimesh_app::ReceiverStreamPipeline<ConstantDecoder, 4, 8, 8>;

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

fn trusted_session() -> SessionState {
    let mut pairing = PairingInvitationController::<1>::new();
    let mut session = SessionState::IDLE;
    pairing
        .begin(&mut session, PairingId::new([1; 16]), 100, 50, 1)
        .unwrap();
    assert_eq!(
        pairing.verify_proof(&mut session, 101, PairingSource::new([2; 16]), || {
            PairingProofVerification::Verified
        }),
        Ok(PairingAttemptOutcome::Paired)
    );
    session
}

fn capabilities() -> &'static [Capability] {
    &[Capability::FrameDuration10Ms]
}

fn hello(capabilities: &[Capability]) -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 1,
        payload: Some(envelope::Payload::Hello(Hello {
            supported_versions: vec![CONTROL_PROTOCOL_VERSION],
            capabilities: capabilities
                .iter()
                .map(|capability| *capability as i32)
                .collect(),
            display_name: "Lab sender".to_owned(),
        })),
    }
}

fn bridge_with(state: SessionState) -> Bridge {
    ControlToMixerBridge::new(
        state,
        capabilities(),
        TestOutput {
            health: OutputHealth::Healthy,
            start_calls: 0,
            stop_calls: 0,
            fail_start: false,
            source: None,
        },
        selection(),
        16,
        16,
    )
    .unwrap()
}

fn bridge() -> Bridge {
    bridge_with(trusted_session())
}

fn authenticated(bridge: &mut Bridge) -> ControlSessionToken {
    let token = bridge.install_authenticated_control_session().unwrap();
    assert!(matches!(
        bridge.accept(token, &hello(capabilities())),
        Ok(ControlStreamAction::Negotiated(_))
    ));
    token
}

fn context(session_id: u64, stream_id: u32) -> MediaKeyContext {
    MediaKeyContext::new(session_id, stream_id, 0, MediaDirection::AndroidToDesktop).unwrap()
}

fn start_for(epoch: u32, session_id: u64, stream_id: u32) -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: u64::from(epoch) * 100 + u64::from(stream_id) + 1,
        payload: Some(envelope::Payload::StartStream(StartStream {
            media_session_id: session_id,
            stream_id,
            sample_rate_hz: 48_000,
            channels: 1,
            frame_duration_ms: 10,
            media_key_epoch: epoch,
        })),
    }
}

fn stop_for(epoch: u32, session_id: u64, stream_id: u32) -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: u64::from(epoch) * 100 + u64::from(stream_id) + 100,
        payload: Some(envelope::Payload::StopStream(StopStream {
            media_session_id: session_id,
            stream_id,
            media_key_epoch: epoch,
        })),
    }
}

fn pipeline_for(session_id: u64, stream_id: u32, counters: Arc<StreamCounters>) -> Pipeline {
    let secret = SessionSecret::new([0xA5; 32]);
    let (producer, _consumer) = realtime_pcm_queue::<BLOCK>(4).unwrap();
    sonimesh_app::ReceiverStreamPipeline::new(
        &secret,
        context(session_id, stream_id),
        ConstantDecoder(0.25),
        producer,
        counters,
        0,
    )
    .unwrap()
}

fn short_pipeline(counters: Arc<StreamCounters>) -> ShortPipeline {
    let secret = SessionSecret::new([0xA5; 32]);
    let (producer, _consumer) = realtime_pcm_queue::<8>(4).unwrap();
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
    bridge: &mut Bridge,
    pipeline: &mut Pipeline,
    sealer: &mut MediaSealer,
    sequence: u32,
) -> [f32; BLOCK] {
    let payload = [0x2A; 8];
    let offset = u64::from(sequence) * u64::try_from(BLOCK).unwrap();
    let packet = sealer.seal(0, offset, &payload).unwrap();
    let mut plaintext = [0_u8; 8];
    pipeline.receive(&packet.datagram, &mut plaintext).unwrap();
    pipeline.decode_next().unwrap();
    bridge.mixer_mut().pump_all().unwrap();
    let mut rendered = [0.0_f32; BLOCK];
    bridge.mixer_mut().output_mut().render(&mut rendered);
    rendered
}

fn start_one(
    bridge: &mut Bridge,
    token: ControlSessionToken,
    session_id: u64,
    stream_id: u32,
    counters: Option<Arc<StreamCounters>>,
    pipeline: &mut Pipeline,
) -> sonimesh_app::StreamStartRequest {
    let ControlStreamAction::Start { token, request } = bridge
        .accept(token, &start_for(0, session_id, stream_id))
        .unwrap()
    else {
        panic!("expected a stream start action");
    };
    bridge
        .execute_start::<_, BLOCK>(token, request, counters, pipeline)
        .unwrap();
    request
}

#[test]
fn first_start_starts_output_admits_and_renders() {
    let counters = Arc::new(StreamCounters::default());
    let mut bridge = bridge();
    let token = authenticated(&mut bridge);
    let mut pipeline = pipeline_for(1, 1, Arc::clone(&counters));
    let mut sealer = sealer();

    let request = start_one(
        &mut bridge,
        token,
        1,
        1,
        Some(Arc::clone(&counters)),
        &mut pipeline,
    );
    assert_eq!(bridge.state().phase(), SessionPhase::Streaming);
    assert_eq!(bridge.coordinator().active(), Some(request.identity));
    assert_eq!(bridge.coordinator().stream_count(), 1);
    assert_eq!(bridge.mixer().stream_count(), 1);
    assert!(bridge.output_active());
    assert_eq!(bridge.mixer().output().start_calls, 1);

    let rendered = feed_cycle(&mut bridge, &mut pipeline, &mut sealer, 0);
    assert!(
        rendered.iter().any(|sample| *sample > 0.0),
        "a started stream must render through the bridge"
    );
}

#[test]
fn second_stream_admits_without_restarting_the_output() {
    let counters_a = Arc::new(StreamCounters::default());
    let counters_b = Arc::new(StreamCounters::default());
    let mut bridge = bridge();
    let token = authenticated(&mut bridge);
    let mut pipeline_a = pipeline_for(1, 1, Arc::clone(&counters_a));
    let mut pipeline_b = pipeline_for(2, 2, Arc::clone(&counters_b));

    start_one(&mut bridge, token, 1, 1, None, &mut pipeline_a);
    start_one(&mut bridge, token, 2, 2, None, &mut pipeline_b);

    assert_eq!(bridge.coordinator().stream_count(), 2);
    assert_eq!(bridge.mixer().stream_count(), 2);
    assert_eq!(bridge.state().phase(), SessionPhase::Streaming);
    assert_eq!(bridge.mixer().output().start_calls, 1, "output starts once");
}

#[test]
fn stopping_one_stream_keeps_the_other_and_the_output() {
    let counters_a = Arc::new(StreamCounters::default());
    let counters_b = Arc::new(StreamCounters::default());
    let mut bridge = bridge();
    let token = authenticated(&mut bridge);
    let mut pipeline_a = pipeline_for(1, 1, Arc::clone(&counters_a));
    let mut pipeline_b = pipeline_for(2, 2, Arc::clone(&counters_b));

    start_one(&mut bridge, token, 1, 1, None, &mut pipeline_a);
    start_one(&mut bridge, token, 2, 2, None, &mut pipeline_b);

    let ControlStreamAction::Stop { token, identity } =
        bridge.accept(token, &stop_for(0, 2, 2)).unwrap()
    else {
        panic!("expected a stream stop action");
    };
    bridge.execute_stop(token, identity).unwrap();

    assert_eq!(bridge.coordinator().stream_count(), 1);
    assert_eq!(bridge.mixer().stream_count(), 1);
    assert_eq!(bridge.state().phase(), SessionPhase::Streaming);
    assert!(bridge.output_active());
    assert_eq!(
        bridge.mixer().output().stop_calls,
        0,
        "output stays while a stream plays"
    );
}

#[test]
fn stopping_the_last_stream_returns_to_ready_and_stops_output() {
    let counters = Arc::new(StreamCounters::default());
    let mut bridge = bridge();
    let token = authenticated(&mut bridge);
    let mut pipeline = pipeline_for(1, 1, Arc::clone(&counters));

    start_one(&mut bridge, token, 1, 1, None, &mut pipeline);

    let ControlStreamAction::Stop { token, identity } =
        bridge.accept(token, &stop_for(0, 1, 1)).unwrap()
    else {
        panic!("expected a stream stop action");
    };
    bridge.execute_stop(token, identity).unwrap();

    assert_eq!(bridge.state().phase(), SessionPhase::Ready);
    assert_eq!(bridge.coordinator().stream_count(), 0);
    assert_eq!(bridge.mixer().stream_count(), 0);
    assert!(!bridge.output_active());
    assert_eq!(bridge.mixer().output().stop_calls, 1);
}

#[test]
fn rotation_commits_a_newer_epoch_and_preserves_the_stream() {
    let counters = Arc::new(StreamCounters::default());
    let mut bridge = bridge();
    let token = authenticated(&mut bridge);
    let mut pipeline = pipeline_for(1, 1, Arc::clone(&counters));
    let mut sealer = sealer();

    let request = start_one(
        &mut bridge,
        token,
        1,
        1,
        Some(Arc::clone(&counters)),
        &mut pipeline,
    );
    assert_eq!(request.identity.key_epoch, 0);

    let ControlStreamAction::Rotate {
        token,
        previous,
        next,
    } = bridge.accept(token, &start_for(1, 1, 1)).unwrap()
    else {
        panic!("expected a rotation action");
    };
    assert_eq!(previous, request.identity);
    bridge.execute_rotation(token, previous, next).unwrap();

    assert_eq!(bridge.coordinator().active().unwrap().key_epoch, 1);
    assert_eq!(bridge.coordinator().stream_count(), 1);
    assert_eq!(bridge.mixer().stream_count(), 1);
    assert!(
        bridge.mixer().gain(next.identity).is_ok(),
        "rotation keeps the gain handle"
    );
    assert!(matches!(
        bridge.mixer().gain(previous),
        Err(MixerStreamSessionError::UnknownStream(_))
    ));

    let rendered = feed_cycle(&mut bridge, &mut pipeline, &mut sealer, 0);
    assert!(
        rendered.iter().any(|sample| *sample > 0.0),
        "a rotated stream must keep rendering"
    );
}

#[test]
fn rotation_with_a_changed_format_is_rejected_and_pending_aborts() {
    let counters = Arc::new(StreamCounters::default());
    let mut bridge = bridge();
    let session = authenticated(&mut bridge);
    let mut pipeline = pipeline_for(1, 1, Arc::clone(&counters));

    start_one(&mut bridge, session, 1, 1, None, &mut pipeline);

    let mut changed = start_for(1, 1, 1);
    if let Some(envelope::Payload::StartStream(stream)) = changed.payload.as_mut() {
        stream.channels = 2;
    }
    let ControlStreamAction::Rotate {
        token,
        previous,
        next,
    } = bridge.accept(session, &changed).unwrap()
    else {
        panic!("expected a rotation action");
    };
    assert!(matches!(
        bridge.execute_rotation(token, previous, next),
        Err(ControlToMixerError::Mixer(
            MixerStreamSessionError::RotationFormatChange
        ))
    ));
    assert_eq!(bridge.coordinator().active().unwrap().key_epoch, 0);
    assert_eq!(bridge.mixer().stream_count(), 1);

    // The aborted pending operation must not block a correct rotation retry.
    let ControlStreamAction::Rotate {
        token,
        previous,
        next,
    } = bridge.accept(session, &start_for(1, 1, 1)).unwrap()
    else {
        panic!("expected a rotation action");
    };
    bridge.execute_rotation(token, previous, next).unwrap();
    assert_eq!(bridge.coordinator().active().unwrap().key_epoch, 1);
}

#[test]
fn output_start_failure_aborts_pending_without_changes() {
    let counters = Arc::new(StreamCounters::default());
    let mut bridge = bridge();
    let session = authenticated(&mut bridge);
    let mut pipeline = pipeline_for(1, 1, Arc::clone(&counters));

    bridge.mixer_mut().output_mut().fail_start = true;
    let ControlStreamAction::Start { token, request } =
        bridge.accept(session, &start_for(0, 1, 1)).unwrap()
    else {
        panic!("expected a stream start action");
    };
    assert!(matches!(
        bridge.execute_start::<_, BLOCK>(token, request, None, &mut pipeline),
        Err(ControlToMixerError::Mixer(MixerStreamSessionError::Output(
            _
        )))
    ));
    assert!(!bridge.output_active());
    assert_eq!(bridge.mixer().stream_count(), 0);

    // The aborted pending operation allows the same start to retry cleanly.
    bridge.mixer_mut().output_mut().fail_start = false;
    let ControlStreamAction::Start { token, request } =
        bridge.accept(session, &start_for(0, 1, 1)).unwrap()
    else {
        panic!("expected a stream start action");
    };
    bridge
        .execute_start::<_, BLOCK>(token, request, None, &mut pipeline)
        .unwrap();
    assert_eq!(bridge.state().phase(), SessionPhase::Streaming);
}

#[test]
fn admission_failure_after_output_start_rolls_back_the_output() {
    let counters = Arc::new(StreamCounters::default());
    let mut bridge = bridge();
    let token = authenticated(&mut bridge);
    let mut short_pipeline = short_pipeline(Arc::clone(&counters));

    let ControlStreamAction::Start { token, request } =
        bridge.accept(token, &start_for(0, 1, 1)).unwrap()
    else {
        panic!("expected a stream start action");
    };
    assert!(matches!(
        bridge.execute_start::<_, 8>(token, request, None, &mut short_pipeline),
        Err(ControlToMixerError::Mixer(
            MixerStreamSessionError::SourceBlockMismatch { expected: 480 }
        ))
    ));
    assert!(!bridge.output_active());
    assert_eq!(bridge.mixer().stream_count(), 0);
    assert_eq!(bridge.mixer().output().start_calls, 1);
    assert_eq!(
        bridge.mixer().output().stop_calls,
        1,
        "a newly started output must roll back after admission failure"
    );
}

#[test]
fn output_fault_and_recovery_readmit_persisted_streams() {
    let counters = Arc::new(StreamCounters::default());
    let mut bridge = bridge();
    let token = authenticated(&mut bridge);
    let mut pipeline = pipeline_for(1, 1, Arc::clone(&counters));
    let mut sealer = sealer();

    let request = start_one(
        &mut bridge,
        token,
        1,
        1,
        Some(Arc::clone(&counters)),
        &mut pipeline,
    );
    let rendered = feed_cycle(&mut bridge, &mut pipeline, &mut sealer, 0);
    assert!(rendered.iter().any(|sample| *sample > 0.0));

    bridge.mixer_mut().output_mut().health = OutputHealth::StreamError;
    assert_eq!(bridge.monitor().unwrap(), Some(OutputHealth::StreamError));
    assert_eq!(bridge.state().phase(), SessionPhase::OutputUnavailable);
    assert!(!bridge.output_active());
    assert_eq!(
        bridge.mixer().stream_count(),
        1,
        "records survive the fault"
    );

    let identities = bridge.recover_output().unwrap();
    assert_eq!(identities, vec![request.identity]);
    assert_eq!(bridge.state().phase(), SessionPhase::Streaming);
    assert!(bridge.output_active());
    bridge
        .recover_stream::<_, BLOCK>(request.identity, &mut pipeline)
        .unwrap();

    let rendered = feed_cycle(&mut bridge, &mut pipeline, &mut sealer, 2);
    assert!(
        rendered.iter().any(|sample| *sample > 0.0),
        "a recovered stream must render after producer reinstallation"
    );
}

#[test]
fn connection_loss_keeps_streams_and_reconnect_stops_the_mixer() {
    let counters = Arc::new(StreamCounters::default());
    let mut bridge = bridge();
    let token = authenticated(&mut bridge);
    let mut pipeline = pipeline_for(1, 1, Arc::clone(&counters));

    start_one(&mut bridge, token, 1, 1, None, &mut pipeline);

    bridge
        .acknowledge_platform_event(SessionEvent::ConnectionLost)
        .unwrap();
    assert_eq!(bridge.state().phase(), SessionPhase::Reconnecting);
    assert_eq!(
        bridge.mixer().stream_count(),
        1,
        "streams keep playing in grace"
    );

    bridge
        .acknowledge_platform_event(SessionEvent::Reconnected)
        .unwrap();
    assert_eq!(bridge.state().phase(), SessionPhase::Ready);
    assert_eq!(
        bridge.mixer().stream_count(),
        0,
        "fresh session material retires streams"
    );
    assert!(!bridge.output_active());
}

#[test]
fn expire_control_session_stops_the_mixer() {
    let counters = Arc::new(StreamCounters::default());
    let mut bridge = bridge();
    let token = authenticated(&mut bridge);
    let mut pipeline = pipeline_for(1, 1, Arc::clone(&counters));

    start_one(&mut bridge, token, 1, 1, None, &mut pipeline);

    bridge.expire_control_session();
    assert_eq!(bridge.state().phase(), SessionPhase::Idle);
    assert_eq!(bridge.mixer().stream_count(), 0);
    assert!(!bridge.output_active());
    assert_eq!(bridge.mixer().output().stop_calls, 1);
}

#[test]
fn export_snapshot_before_start_omits_format_and_streams() {
    let bridge = bridge();
    let mut metrics = Vec::new();
    let snapshot = bridge.export_snapshot("0.1.0", None, None, &mut metrics);
    assert_eq!(snapshot.software_version, "0.1.0");
    assert_eq!(snapshot.session_phase, Some(ExportSessionPhase::Ready));
    assert_eq!(snapshot.platform_format, None);
    assert!(snapshot.streams.is_empty());
}

#[test]
fn export_snapshot_after_start_includes_format_and_metrics() {
    let counters = Arc::new(StreamCounters::default());
    let mut bridge = bridge();
    let token = authenticated(&mut bridge);
    let mut pipeline = pipeline_for(1, 1, Arc::clone(&counters));
    let mut sealer = sealer();

    start_one(
        &mut bridge,
        token,
        1,
        1,
        Some(Arc::clone(&counters)),
        &mut pipeline,
    );
    feed_cycle(&mut bridge, &mut pipeline, &mut sealer, 0);

    let mut metrics = Vec::new();
    let snapshot = bridge.export_snapshot("0.1.0", None, None, &mut metrics);
    assert_eq!(snapshot.session_phase, Some(ExportSessionPhase::Streaming));
    assert_eq!(
        snapshot.platform_format,
        Some(ExportPlatformFormat {
            sample_rate_hz: 48_000,
            channels: 1
        })
    );
    assert_eq!(snapshot.streams.len(), 1);
    assert_eq!(snapshot.streams[0].authenticated_packets, 1);
}

#[test]
fn export_stream_metrics_follow_slot_order() {
    let counters_a = Arc::new(StreamCounters::default());
    let counters_b = Arc::new(StreamCounters::default());
    let mut bridge = bridge();
    let token = authenticated(&mut bridge);
    let mut pipeline_a = pipeline_for(1, 1, Arc::clone(&counters_a));
    let mut pipeline_b = pipeline_for(2, 2, Arc::clone(&counters_b));

    start_one(
        &mut bridge,
        token,
        1,
        1,
        Some(Arc::clone(&counters_a)),
        &mut pipeline_a,
    );
    start_one(
        &mut bridge,
        token,
        2,
        2,
        Some(Arc::clone(&counters_b)),
        &mut pipeline_b,
    );

    counters_b.authenticated();

    let mut metrics = Vec::new();
    let snapshot = bridge.export_snapshot("0.1.0", None, None, &mut metrics);
    assert_eq!(snapshot.streams.len(), 2);
    assert_eq!(snapshot.streams[0].authenticated_packets, 0);
    assert_eq!(snapshot.streams[1].authenticated_packets, 1);
}

#[test]
fn built_bundle_redacts_endpoint_and_forbidden_bytes() {
    let counters = Arc::new(StreamCounters::default());
    let mut bridge = bridge();
    let token = authenticated(&mut bridge);
    let mut pipeline = pipeline_for(1, 1, Arc::clone(&counters));
    let mut sealer = sealer();

    start_one(
        &mut bridge,
        token,
        1,
        1,
        Some(Arc::clone(&counters)),
        &mut pipeline,
    );
    feed_cycle(&mut bridge, &mut pipeline, &mut sealer, 0);

    let endpoint = "10.0.0.5:1234".parse().unwrap();
    let mut metrics = Vec::new();
    let snapshot = bridge.export_snapshot("0.1.0", Some(endpoint), None, &mut metrics);
    let bundle = sonimesh_diagnostics::build_export_bundle(&snapshot).unwrap();

    assert!(bundle.contains("\"software_version\":\"0.1.0\""));
    assert!(bundle.contains("\"session_phase\":\"streaming\""));
    assert!(bundle.contains("\"sample_rate_hz\":48000"));
    assert!(
        !bundle.contains("10.0.0.5"),
        "raw endpoint must be redacted"
    );
    assert!(bundle.contains("1234"), "redacted endpoint keeps the port");
    assert!(
        !bundle
            .as_bytes()
            .windows(8)
            .any(|window| window == [0x2A; 8]),
        "audio payload bytes must never reach a bundle"
    );
    assert!(
        !bundle
            .as_bytes()
            .windows(32)
            .any(|window| window == [0xA5; 32]),
        "session key bytes must never reach a bundle"
    );
}

#[test]
fn snapshot_projects_low_rate_bridge_state() {
    let counters = Arc::new(StreamCounters::default());
    let mut bridge = bridge();
    let token = authenticated(&mut bridge);
    let mut pipeline = pipeline_for(1, 1, Arc::clone(&counters));

    let before = bridge.snapshot();
    assert_eq!(before.session(), SessionPhase::Ready);
    assert!(!before.output_active());
    assert_eq!(before.active_streams(), 0);
    assert!(!before.output().stream_active());

    start_one(&mut bridge, token, 1, 1, None, &mut pipeline);

    let after = bridge.snapshot();
    assert_eq!(after.session(), SessionPhase::Streaming);
    assert!(after.output_active());
    assert_eq!(after.active_streams(), 1);
    assert!(after.output().stream_active());

    let ControlStreamAction::Stop { token, identity } =
        bridge.accept(token, &stop_for(0, 1, 1)).unwrap()
    else {
        panic!("expected a stream stop action");
    };
    bridge.execute_stop(token, identity).unwrap();

    let stopped = bridge.snapshot();
    assert_eq!(stopped.session(), SessionPhase::Ready);
    assert!(!stopped.output_active());
    assert_eq!(stopped.active_streams(), 0);
    assert!(!stopped.output().stream_active());
}

#[test]
fn bridge_projects_output_lifecycle_for_existing_sources() {
    let counters = Arc::new(StreamCounters::default());
    let mut bridge = bridge();
    let token = authenticated(&mut bridge);
    let mut pipeline = pipeline_for(1, 1, Arc::clone(&counters));

    start_one(&mut bridge, token, 1, 1, None, &mut pipeline);

    let output = sonimesh_app::OutputSnapshotSource::snapshot(&bridge);
    assert!(output.stream_active());
    assert_eq!(output.health(), OutputHealth::Healthy);
}

#[test]
fn export_action_issues_bounded_bundles_and_retains_the_newest() {
    let counters = Arc::new(StreamCounters::default());
    let mut bridge = bridge();
    let token = authenticated(&mut bridge);
    let mut pipeline = pipeline_for(1, 1, Arc::clone(&counters));
    let _request = start_one(
        &mut bridge,
        token,
        1,
        1,
        Some(Arc::clone(&counters)),
        &mut pipeline,
    );
    assert!(bridge.output_active());

    let mut action =
        DiagnosticExportAction::new(ExportRetentionPolicy::bounded(2, 16 * 1024)).unwrap();

    let first = action
        .export(
            &bridge,
            "0.1.0",
            Some("127.0.0.1:1234".parse().unwrap()),
            Some("balanced"),
        )
        .unwrap();
    assert_eq!(first.receipt.sequence, 0);
    assert_eq!(first.receipt.bytes, first.bundle.len());
    assert!(first.bundle.contains("\"software_version\":\"0.1.0\""));
    assert!(first.bundle.contains("\"buffer_policy\":\"balanced\""));
    assert!(first.bundle.contains("\"session_phase\":\"streaming\""));
    assert!(
        first
            .bundle
            .contains("\"streams\":[{\"authenticated_packets\"")
    );
    assert!(!first.bundle.contains("127.0.0.1"));
    assert_eq!(action.retained_count(), 1);

    let second = action
        .export(
            &bridge,
            "0.1.0",
            Some("127.0.0.1:1234".parse().unwrap()),
            None,
        )
        .unwrap();
    assert_eq!(second.receipt.sequence, 1);
    assert_eq!(action.retained_count(), 2);

    let third = action.export(&bridge, "0.1.0", None, None).unwrap();
    assert_eq!(third.receipt.sequence, 2);
    assert_eq!(
        action.retained_count(),
        2,
        "the oldest receipt is evicted past the count bound"
    );
    assert_eq!(
        action.retained_bytes(),
        third.receipt.bytes + second.receipt.bytes,
        "retained bytes follow the newest two receipts"
    );
}

#[test]
fn export_flow_persists_bundles_to_the_disk_store_and_recovers_them() {
    let counters = Arc::new(StreamCounters::default());
    let mut bridge = bridge();
    let token = authenticated(&mut bridge);
    let mut pipeline = pipeline_for(1, 1, Arc::clone(&counters));
    start_one(
        &mut bridge,
        token,
        1,
        1,
        Some(Arc::clone(&counters)),
        &mut pipeline,
    );
    assert!(bridge.output_active());

    let dir = export_temp_dir();
    let policy = ExportRetentionPolicy::bounded(4, 16 * 1024);
    let mut action = DiagnosticExportAction::new(policy).unwrap();
    let mut store = ExportStore::open(&dir, policy).unwrap();

    let first = action
        .export(
            &bridge,
            "0.1.0",
            Some("127.0.0.1:1234".parse().unwrap()),
            Some("balanced"),
        )
        .unwrap();
    let second = action.export(&bridge, "0.1.0", None, None).unwrap();
    assert!(!first.bundle.contains("127.0.0.1"));

    for issued in [&first, &second] {
        let receipt = store.store(&issued.bundle).unwrap();
        assert_eq!(receipt.sequence, issued.receipt.sequence);
        assert_eq!(receipt.bytes, issued.receipt.bytes);
        assert_eq!(store.read(receipt.sequence).unwrap(), issued.bundle);
    }
    assert_eq!(store.retained_count(), 2);
    assert_eq!(store.next_sequence(), 2);

    // The on-disk store survives independently of the in-memory action.
    drop(store);
    let recovered = ExportStore::open(&dir, policy).unwrap();
    assert_eq!(recovered.read(0).unwrap(), first.bundle);
    assert_eq!(recovered.read(1).unwrap(), second.bundle);
    assert_eq!(recovered.next_sequence(), 2);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pipeline_publishes_jitter_water_level_after_receive_and_decode() {
    let counters = Arc::new(StreamCounters::default());
    let mut bridge = bridge();
    let token = authenticated(&mut bridge);
    let mut pipeline = pipeline_for(1, 1, Arc::clone(&counters));
    let mut sealer = sealer();

    start_one(
        &mut bridge,
        token,
        1,
        1,
        Some(Arc::clone(&counters)),
        &mut pipeline,
    );

    // Admit three consecutive packets without decoding: every authenticated
    // frame stays buffered, so the current level and the high-water mark
    // climb together.
    for sequence in 0_u32..3 {
        let packet = sealer
            .seal(
                0,
                u64::from(sequence) * u64::try_from(BLOCK).unwrap(),
                &[0x2A; 8],
            )
            .unwrap();
        let mut plaintext = [0_u8; 8];
        let outcome = pipeline.receive(&packet.datagram, &mut plaintext).unwrap();
        assert!(
            !matches!(
                outcome,
                sonimesh_audio::InsertOutcome::Resynchronized { .. }
            ),
            "the first three packets must not resynchronize the jitter buffer"
        );
    }
    let snapshot = counters.snapshot();
    assert_eq!(snapshot.jitter_frames, 3);
    assert_eq!(snapshot.jitter_high_water_frames, 3);

    // Decoding drains exactly one buffered frame. The high-water mark is a
    // per-epoch maximum and must not decrease.
    assert!(matches!(
        pipeline.decode_next().unwrap(),
        sonimesh_audio::DecodeStep::Decoded { .. }
    ));
    let snapshot = counters.snapshot();
    assert_eq!(snapshot.jitter_frames, 2);
    assert_eq!(snapshot.jitter_high_water_frames, 3);

    // The exported snapshot carries the same water level through the bridge.
    let mut metrics = Vec::new();
    let exported = bridge.export_snapshot("0.1.0", None, None, &mut metrics);
    assert_eq!(exported.streams.len(), 1);
    assert_eq!(exported.streams[0].jitter_frames, 2);
    assert_eq!(exported.streams[0].jitter_high_water_frames, 3);
}

#[test]
fn pipeline_retires_jitter_epoch_on_hard_resynchronize() {
    let counters = Arc::new(StreamCounters::default());
    let mut bridge = bridge();
    let token = authenticated(&mut bridge);
    let mut pipeline = pipeline_for(1, 1, Arc::clone(&counters));
    let mut sealer = sealer();

    start_one(
        &mut bridge,
        token,
        1,
        1,
        Some(Arc::clone(&counters)),
        &mut pipeline,
    );

    for sequence in 0_u32..3 {
        let packet = sealer
            .seal(
                0,
                u64::from(sequence) * u64::try_from(BLOCK).unwrap(),
                &[0x2A; 8],
            )
            .unwrap();
        let mut plaintext = [0_u8; 8];
        let _ = pipeline.receive(&packet.datagram, &mut plaintext).unwrap();
    }
    let before = counters.snapshot();
    assert_eq!(before.jitter_frames, 3);
    assert_eq!(before.jitter_high_water_frames, 3);

    pipeline.hard_resynchronize();

    let after = counters.snapshot();
    assert_eq!(after.jitter_frames, 0);
    assert_eq!(after.jitter_high_water_frames, 0);
    // Admission counters survive the stream-local reset; only the jitter
    // epoch is retired, and the reset itself is recorded.
    assert_eq!(after.authenticated_packets, before.authenticated_packets);
    assert_eq!(after.stream_resets, before.stream_resets + 1);
}

static NEXT_EXPORT_DIR: AtomicU64 = AtomicU64::new(0);

fn export_temp_dir() -> PathBuf {
    let base = std::env::temp_dir().join("sonimesh-export-flow-tests");
    let id = NEXT_EXPORT_DIR.fetch_add(1, Ordering::Relaxed);
    let path = base.join(format!("{}-{id}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create temporary directory");
    path
}

#[test]
fn desktop_snapshot_projects_idle_inactive_from_a_fresh_bridge() {
    let mut bridge = bridge_with(SessionState::IDLE);
    let mut pairing = PairingInvitationController::<1>::new();

    let snapshot = bridge.desktop_snapshot_at(&mut pairing, 100).unwrap();

    assert_eq!(snapshot.session(), SessionPhase::Idle);
    assert_eq!(snapshot.pairing(), PairingSnapshotState::Inactive);
    assert_eq!(snapshot.output(), bridge.snapshot().output());
    assert_eq!(snapshot.output().health(), OutputHealth::Healthy);
    assert!(!snapshot.output().selection_retained());
    assert!(!snapshot.output().stream_active());
    assert!(!snapshot.output().fault_reported());
}

#[test]
fn desktop_snapshot_projects_an_active_pairing_invitation() {
    let mut bridge = bridge_with(SessionState::IDLE);
    let mut pairing = PairingInvitationController::<1>::new();
    bridge
        .coordinator_mut()
        .begin_pairing(&mut pairing, PairingId::new([3; 16]), 100, 50, 1)
        .unwrap();

    let snapshot = bridge.desktop_snapshot_at(&mut pairing, 101).unwrap();

    assert_eq!(snapshot.session(), SessionPhase::Pairing);
    assert_eq!(snapshot.pairing(), PairingSnapshotState::Active);
}

#[test]
fn desktop_snapshot_expiry_returns_the_bridge_session_to_idle() {
    let mut bridge = bridge_with(SessionState::IDLE);
    let mut pairing = PairingInvitationController::<1>::new();
    bridge
        .coordinator_mut()
        .begin_pairing(&mut pairing, PairingId::new([5; 16]), 100, 50, 1)
        .unwrap();

    let snapshot = bridge.desktop_snapshot_at(&mut pairing, 150).unwrap();

    assert_eq!(snapshot.session(), SessionPhase::Idle);
    assert_eq!(snapshot.pairing(), PairingSnapshotState::Expired);
    assert_eq!(bridge.state().phase(), SessionPhase::Idle);
}

#[test]
fn desktop_snapshot_after_trusted_session_projects_ready_consumed() {
    let mut session = SessionState::IDLE;
    let mut pairing = PairingInvitationController::<1>::new();
    pairing
        .begin(&mut session, PairingId::new([7; 16]), 100, 50, 1)
        .unwrap();
    assert_eq!(
        pairing.verify_proof(&mut session, 101, PairingSource::new([9; 16]), || {
            PairingProofVerification::Verified
        }),
        Ok(PairingAttemptOutcome::Paired)
    );
    let mut bridge = bridge_with(session);
    authenticated(&mut bridge);

    let snapshot = bridge.desktop_snapshot_at(&mut pairing, 102).unwrap();

    assert_eq!(snapshot.session(), SessionPhase::Ready);
    assert_eq!(snapshot.pairing(), PairingSnapshotState::Consumed);
    assert_eq!(snapshot.output(), bridge.snapshot().output());
}

#[test]
fn desktop_snapshot_projects_streaming_with_live_output_lifecycle() {
    let counters = Arc::new(StreamCounters::default());
    let mut session = SessionState::IDLE;
    let mut pairing = PairingInvitationController::<1>::new();
    pairing
        .begin(&mut session, PairingId::new([11; 16]), 100, 50, 1)
        .unwrap();
    assert_eq!(
        pairing.verify_proof(&mut session, 101, PairingSource::new([13; 16]), || {
            PairingProofVerification::Verified
        }),
        Ok(PairingAttemptOutcome::Paired)
    );
    let mut bridge = bridge_with(session);
    let token = authenticated(&mut bridge);
    let mut pipeline = pipeline_for(1, 1, Arc::clone(&counters));
    start_one(&mut bridge, token, 1, 1, None, &mut pipeline);

    let snapshot = bridge.desktop_snapshot_at(&mut pairing, 102).unwrap();

    assert_eq!(snapshot.session(), SessionPhase::Streaming);
    assert_eq!(snapshot.pairing(), PairingSnapshotState::Consumed);
    assert!(snapshot.output().selection_retained());
    assert!(snapshot.output().stream_active());
    assert!(!snapshot.output().fault_reported());
    assert_eq!(snapshot.output().health(), OutputHealth::Healthy);
}

#[test]
fn desktop_snapshot_projects_output_unavailable_after_a_fault() {
    let counters = Arc::new(StreamCounters::default());
    let mut session = SessionState::IDLE;
    let mut pairing = PairingInvitationController::<1>::new();
    pairing
        .begin(&mut session, PairingId::new([17; 16]), 100, 50, 1)
        .unwrap();
    assert_eq!(
        pairing.verify_proof(&mut session, 101, PairingSource::new([19; 16]), || {
            PairingProofVerification::Verified
        }),
        Ok(PairingAttemptOutcome::Paired)
    );
    let mut bridge = bridge_with(session);
    let token = authenticated(&mut bridge);
    let mut pipeline = pipeline_for(1, 1, Arc::clone(&counters));
    start_one(&mut bridge, token, 1, 1, None, &mut pipeline);

    bridge.mixer_mut().output_mut().health = OutputHealth::StreamError;
    assert!(matches!(
        bridge.monitor(),
        Ok(Some(OutputHealth::StreamError))
    ));

    let snapshot = bridge.desktop_snapshot_at(&mut pairing, 102).unwrap();

    assert_eq!(snapshot.session(), SessionPhase::OutputUnavailable);
    assert_eq!(snapshot.pairing(), PairingSnapshotState::Consumed);
    assert!(snapshot.output().selection_retained());
    assert!(!snapshot.output().stream_active());
    assert!(snapshot.output().fault_reported());
    assert_eq!(snapshot.output().health(), OutputHealth::StreamError);
}

#[test]
fn desktop_snapshot_projects_reconnecting_after_connection_loss() {
    let counters = Arc::new(StreamCounters::default());
    let mut session = SessionState::IDLE;
    let mut pairing = PairingInvitationController::<1>::new();
    pairing
        .begin(&mut session, PairingId::new([23; 16]), 100, 50, 1)
        .unwrap();
    assert_eq!(
        pairing.verify_proof(&mut session, 101, PairingSource::new([29; 16]), || {
            PairingProofVerification::Verified
        }),
        Ok(PairingAttemptOutcome::Paired)
    );
    let mut bridge = bridge_with(session);
    let token = authenticated(&mut bridge);
    let mut pipeline = pipeline_for(1, 1, Arc::clone(&counters));
    start_one(&mut bridge, token, 1, 1, None, &mut pipeline);

    bridge
        .acknowledge_platform_event(SessionEvent::ConnectionLost)
        .unwrap();

    let snapshot = bridge.desktop_snapshot_at(&mut pairing, 103).unwrap();

    assert_eq!(snapshot.session(), SessionPhase::Reconnecting);
    assert_eq!(snapshot.pairing(), PairingSnapshotState::Consumed);
    assert!(
        snapshot.output().stream_active(),
        "output stays alive while the bridge reconnects"
    );
}

#[test]
fn snapshot_polling_alone_retires_the_reconnect_window() {
    // A control-surface caller that only polls desktop snapshots must observe
    // the bounded reconnect window counting down through the composition entry
    // point and then retiring to idle, without a separate reconnect-clock
    // observation or event. This is the exact path a low-rate bridge uses.
    let counters = Arc::new(StreamCounters::default());
    let mut session = SessionState::IDLE;
    let mut pairing = PairingInvitationController::<1>::new();
    pairing
        .begin(&mut session, PairingId::new([31; 16]), 100, 50, 1)
        .unwrap();
    assert_eq!(
        pairing.verify_proof(&mut session, 101, PairingSource::new([37; 16]), || {
            PairingProofVerification::Verified
        }),
        Ok(PairingAttemptOutcome::Paired)
    );
    let mut bridge = bridge_with(session);
    let token = authenticated(&mut bridge);
    let mut pipeline = pipeline_for(1, 1, Arc::clone(&counters));
    start_one(&mut bridge, token, 1, 1, None, &mut pipeline);

    bridge
        .acknowledge_platform_event(SessionEvent::ConnectionLost)
        .unwrap();
    bridge.begin_reconnect_grace(200, 2_000).unwrap();

    let reconnecting = bridge.desktop_snapshot_at(&mut pairing, 500).unwrap();
    assert_eq!(reconnecting.session(), SessionPhase::Reconnecting);
    assert_eq!(reconnecting.pairing(), PairingSnapshotState::Consumed);
    assert_eq!(reconnecting.reconnect_remaining_ms(), Some(1_700));

    let expired = bridge.desktop_snapshot_at(&mut pairing, 2_200).unwrap();
    assert_eq!(expired.session(), SessionPhase::Idle);
    assert_eq!(expired.reconnect_remaining_ms(), None);
    assert_eq!(bridge.state().phase(), SessionPhase::Idle);
    assert_eq!(
        bridge.coordinator().reconnect_grace_state(),
        sonimesh_app::ReconnectGraceState::Idle
    );
}
