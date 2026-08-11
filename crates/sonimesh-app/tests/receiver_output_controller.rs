//! Receiver output recovery tests with paired pipeline endpoint installation.

use core::convert::Infallible;
use std::sync::Arc;

use sonimesh_app::{
    OutputSelection, ReceiverOutputController, ReceiverOutputLifecycleError,
    ReceiverPcmEndpointFactory, ReceiverStreamPipeline, SourcePcmFormat, UdpReceiverPipeline,
};
use sonimesh_audio::{DecodeStep, FrameDecoder, MAX_PCM_QUEUE_BLOCKS, realtime_pcm_queue};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceInfo, OutputError, OutputHealth, RenderSource,
    SampleFormat,
};
use sonimesh_protocol::{MediaDirection, MediaKeyContext, MediaSealer, SessionSecret};

struct ByteDecoder;

impl FrameDecoder for ByteDecoder {
    type Error = Infallible;

    fn decode(&mut self, encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
        output[0] = f32::from(encoded[0]);
        Ok(1)
    }

    fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
        output[0] = 0.0;
        Ok(1)
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

fn context() -> MediaKeyContext {
    MediaKeyContext::new(
        0x0102_0304_0506_0708,
        9,
        1,
        MediaDirection::AndroidToDesktop,
    )
    .unwrap()
}

fn selection() -> OutputSelection {
    OutputSelection {
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

fn controller(counters: Arc<StreamCounters>) -> ReceiverOutputController<TestOutput, 4> {
    ReceiverOutputController::new(
        TestOutput {
            health: OutputHealth::Healthy,
            start_calls: 0,
            stop_calls: 0,
            fail_start: false,
            source: None,
        },
        ReceiverPcmEndpointFactory::new(4, SourcePcmFormat::new(48_000, 1).unwrap(), counters),
    )
}

fn pipeline(counters: Arc<StreamCounters>) -> ReceiverStreamPipeline<ByteDecoder, 4, 8, 4> {
    let secret = SessionSecret::new([0xA5; 32]);
    let (producer, _consumer) = realtime_pcm_queue::<4>(4).unwrap();
    ReceiverStreamPipeline::new(&secret, context(), ByteDecoder, producer, counters, 0).unwrap()
}

fn udp_pipeline(counters: Arc<StreamCounters>) -> UdpReceiverPipeline<ByteDecoder, 4, 8, 4> {
    let secret = SessionSecret::new([0xA5; 32]);
    let (producer, _consumer) = realtime_pcm_queue::<4>(4).unwrap();
    UdpReceiverPipeline::bind(
        "127.0.0.1:0".parse().unwrap(),
        &secret,
        context(),
        ByteDecoder,
        producer,
        counters,
        0,
    )
    .unwrap()
}

fn enqueue_decoded_block(
    pipeline: &mut ReceiverStreamPipeline<ByteDecoder, 4, 8, 4>,
    sealer: &mut MediaSealer,
    sequence: u32,
) {
    let payload = [u8::try_from(sequence).expect("test sequence fits in one byte")];
    let packet = sealer.seal(0, u64::from(sequence) * 960, &payload).unwrap();
    let mut plaintext = [0_u8; 8];

    pipeline.receive(&packet.datagram, &mut plaintext).unwrap();
    assert!(matches!(
        pipeline.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: actual, .. } if actual == sequence
    ));
}

fn large_controller(counters: Arc<StreamCounters>) -> ReceiverOutputController<TestOutput, 4> {
    ReceiverOutputController::new(
        TestOutput {
            health: OutputHealth::Healthy,
            start_calls: 0,
            stop_calls: 0,
            fail_start: false,
            source: None,
        },
        ReceiverPcmEndpointFactory::new(
            MAX_PCM_QUEUE_BLOCKS,
            SourcePcmFormat::new(48_000, 1).unwrap(),
            counters,
        ),
    )
}

/// Decoder that fills each whole PCM block with constant 1.0 samples.
///
/// [`ByteDecoder`] yields one sample per authenticated packet, which cannot
/// fill the 128-sample bounded gain ramp inside the 128-block queue. This
/// decoder yields four samples per packet so a full ramp plus a settled tail
/// fits within one queue lifetime.
struct FullBlockDecoder;

impl FrameDecoder for FullBlockDecoder {
    type Error = Infallible;

    fn decode(&mut self, _encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
        output.fill(1.0);
        Ok(output.len())
    }

    fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
        output.fill(0.0);
        Ok(output.len())
    }
}

fn full_block_pipeline(
    counters: Arc<StreamCounters>,
) -> ReceiverStreamPipeline<FullBlockDecoder, 4, 8, 4> {
    let secret = SessionSecret::new([0xA5; 32]);
    let (producer, _consumer) = realtime_pcm_queue::<4>(4).unwrap();
    ReceiverStreamPipeline::new(&secret, context(), FullBlockDecoder, producer, counters, 0)
        .unwrap()
}

/// Feeds `count` authenticated packets that each decode into one full 1.0 block.
fn enqueue_full_blocks(
    pipeline: &mut ReceiverStreamPipeline<FullBlockDecoder, 4, 8, 4>,
    sealer: &mut MediaSealer,
    start_sequence: u32,
    count: usize,
) {
    for offset in 0..count {
        let sequence = start_sequence + u32::try_from(offset).unwrap();
        let packet = sealer.seal(0, u64::from(sequence) * 960, &[1]).unwrap();
        let mut plaintext = [0_u8; 8];

        pipeline.receive(&packet.datagram, &mut plaintext).unwrap();
        assert!(matches!(
            pipeline.decode_next().unwrap(),
            DecodeStep::Decoded {
                sequence: actual,
                samples: 4,
                ..
            } if actual == sequence
        ));
    }
}

#[test]
fn controller_gain_handle_is_the_factory_handle_shared_with_endpoints() {
    let counters = Arc::new(StreamCounters::default());
    let factory = ReceiverPcmEndpointFactory::<4>::new(
        4,
        SourcePcmFormat::new(48_000, 1).unwrap(),
        Arc::clone(&counters),
    );
    let factory_handle = factory.gain_handle();
    let controller = ReceiverOutputController::new(
        TestOutput {
            health: OutputHealth::Healthy,
            start_calls: 0,
            stop_calls: 0,
            fail_start: false,
            source: None,
        },
        factory,
    );

    let controller_handle = controller.gain_handle();
    factory_handle.set_gain(0.5).unwrap();
    assert!(
        (controller_handle.gain() - 0.5).abs() < f32::EPSILON,
        "controller handle did not share the published gain"
    );
    controller_handle.set_muted(true);
    assert!(factory_handle.is_muted());
}

#[test]
fn gain_and_mute_published_through_the_controller_reach_the_active_source() {
    let counters = Arc::new(StreamCounters::default());
    let mut pipeline = full_block_pipeline(Arc::clone(&counters));
    let mut controller = large_controller(Arc::clone(&counters));
    controller.start(selection(), &mut pipeline).unwrap();
    let gain = controller.gain_handle();

    let mut sealer = MediaSealer::new(&SessionSecret::new([0xA5; 32]), context()).unwrap();
    enqueue_full_blocks(&mut pipeline, &mut sealer, 0, MAX_PCM_QUEUE_BLOCKS);

    gain.set_gain(0.5).unwrap();
    let mut callback = [f32::NAN; 480];
    controller.output_mut().render(&mut callback);
    assert!(
        callback[476..]
            .iter()
            .all(|sample| (*sample - 0.5).abs() < f32::EPSILON),
        "published gain did not reach the active source: {:?}",
        &callback[476..]
    );

    enqueue_full_blocks(&mut pipeline, &mut sealer, 128, MAX_PCM_QUEUE_BLOCKS);
    gain.set_muted(true);
    controller.output_mut().render(&mut callback);
    assert!(
        callback[476..]
            .iter()
            .all(|sample| sample.abs() < f32::EPSILON),
        "published mute did not reach the active source: {:?}",
        &callback[476..]
    );
}

#[test]
fn gain_and_mute_handle_survive_output_recovery_through_the_controller() {
    let counters = Arc::new(StreamCounters::default());
    let mut pipeline = full_block_pipeline(Arc::clone(&counters));
    let mut controller = large_controller(Arc::clone(&counters));
    controller.start(selection(), &mut pipeline).unwrap();
    let gain = controller.gain_handle();
    let mut sealer = MediaSealer::new(&SessionSecret::new([0xA5; 32]), context()).unwrap();

    enqueue_full_blocks(&mut pipeline, &mut sealer, 0, MAX_PCM_QUEUE_BLOCKS);
    gain.set_gain(0.5).unwrap();
    let mut callback = [f32::NAN; 480];
    controller.output_mut().render(&mut callback);
    assert!(
        callback[476..]
            .iter()
            .all(|sample| (*sample - 0.5).abs() < f32::EPSILON),
        "gain did not reach the source before recovery: {:?}",
        &callback[476..]
    );

    controller.output_mut().health = OutputHealth::StreamError;
    assert_eq!(controller.monitor(), Some(OutputHealth::StreamError));
    controller.recover(&mut pipeline).unwrap();

    // The recreated source keeps the published gain without a replay command.
    enqueue_full_blocks(&mut pipeline, &mut sealer, 128, MAX_PCM_QUEUE_BLOCKS);
    controller.output_mut().render(&mut callback);
    assert!(
        callback[476..]
            .iter()
            .all(|sample| (*sample - 0.5).abs() < f32::EPSILON),
        "gain did not survive output recovery: {:?}",
        &callback[476..]
    );

    gain.set_muted(true);
    enqueue_full_blocks(&mut pipeline, &mut sealer, 256, MAX_PCM_QUEUE_BLOCKS);
    controller.output_mut().render(&mut callback);
    assert!(
        callback[476..]
            .iter()
            .all(|sample| sample.abs() < f32::EPSILON),
        "mute did not survive output recovery: {:?}",
        &callback[476..]
    );
}

#[test]
fn healthy_monitor_keeps_the_active_output_and_format_published() {
    let counters = Arc::new(StreamCounters::default());
    let mut pipeline = pipeline(Arc::clone(&counters));
    let mut controller = controller(Arc::clone(&counters));

    controller.start(selection(), &mut pipeline).unwrap();

    assert_eq!(controller.monitor(), None);
    assert_eq!(controller.output().start_calls, 1);
    assert_eq!(controller.output().stop_calls, 0);
    assert!(matches!(
        controller.recover(&mut pipeline),
        Err(ReceiverOutputLifecycleError::StreamAlreadyActive)
    ));
    assert_eq!(controller.output().start_calls, 1);
    let snapshot = counters.snapshot();
    assert_eq!(snapshot.nominal_input_sample_rate_hz, 48_000);
    assert_eq!(snapshot.nominal_output_sample_rate_hz, 48_000);
}

#[test]
fn failed_start_keeps_the_existing_pipeline_producer() {
    let counters = Arc::new(StreamCounters::default());
    let mut pipeline = pipeline(Arc::clone(&counters));
    let mut sealer = MediaSealer::new(&SessionSecret::new([0xA5; 32]), context()).unwrap();
    enqueue_decoded_block(&mut pipeline, &mut sealer, 0);
    assert_eq!(pipeline.queued_pcm_blocks(), 1);

    let mut controller = controller(Arc::clone(&counters));
    controller.output_mut().fail_start = true;

    assert!(matches!(
        controller.start(selection(), &mut pipeline),
        Err(ReceiverOutputLifecycleError::Output(OutputError::Backend(
            _
        )))
    ));
    assert_eq!(controller.output().start_calls, 1);
    assert_eq!(pipeline.queued_pcm_blocks(), 1);
    let snapshot = counters.snapshot();
    assert_eq!(snapshot.stream_resets, 0);
    assert_eq!(snapshot.nominal_input_sample_rate_hz, 0);
    assert_eq!(snapshot.nominal_output_sample_rate_hz, 0);
}

#[test]
fn recovery_replaces_the_pipeline_producer_only_after_platform_start() {
    let counters = Arc::new(StreamCounters::default());
    let mut pipeline = pipeline(Arc::clone(&counters));
    let mut sealer = MediaSealer::new(&SessionSecret::new([0xA5; 32]), context()).unwrap();
    enqueue_decoded_block(&mut pipeline, &mut sealer, 0);
    let mut controller = controller(Arc::clone(&counters));

    controller.start(selection(), &mut pipeline).unwrap();
    assert_eq!(pipeline.queued_pcm_blocks(), 0);
    let started = counters.snapshot();
    assert_eq!(started.stream_resets, 1);
    assert_eq!(started.nominal_input_sample_rate_hz, 48_000);
    assert_eq!(started.nominal_output_sample_rate_hz, 48_000);
    assert_eq!(started.queued_pcm_high_water_blocks, 0);

    enqueue_decoded_block(&mut pipeline, &mut sealer, 1);
    assert_eq!(pipeline.queued_pcm_blocks(), 1);
    controller.output_mut().health = OutputHealth::StreamError;
    assert_eq!(controller.monitor(), Some(OutputHealth::StreamError));
    assert_eq!(controller.output().stop_calls, 1);
    let stopped = counters.snapshot();
    assert_eq!(stopped.nominal_input_sample_rate_hz, 0);
    assert_eq!(stopped.nominal_output_sample_rate_hz, 0);

    controller.output_mut().fail_start = true;
    assert!(matches!(
        controller.recover(&mut pipeline),
        Err(ReceiverOutputLifecycleError::Output(OutputError::Backend(
            _
        )))
    ));
    assert_eq!(pipeline.queued_pcm_blocks(), 1);
    let failed_recovery = counters.snapshot();
    assert_eq!(failed_recovery.stream_resets, 1);
    assert_eq!(failed_recovery.queued_pcm_high_water_blocks, 1);
    assert_eq!(failed_recovery.nominal_input_sample_rate_hz, 0);
    assert_eq!(failed_recovery.nominal_output_sample_rate_hz, 0);

    controller.output_mut().fail_start = false;
    controller.recover(&mut pipeline).unwrap();
    assert_eq!(controller.output().start_calls, 3);
    assert_eq!(pipeline.queued_pcm_blocks(), 0);
    let recovered = counters.snapshot();
    assert_eq!(recovered.stream_resets, 2);
    assert_eq!(recovered.queued_pcm_high_water_blocks, 0);
    assert_eq!(recovered.nominal_input_sample_rate_hz, 48_000);
    assert_eq!(recovered.nominal_output_sample_rate_hz, 48_000);
}

#[test]
fn explicit_stop_clears_format_disarms_stale_health_and_allows_recovery() {
    let counters = Arc::new(StreamCounters::default());
    let mut pipeline = pipeline(Arc::clone(&counters));
    let mut sealer = MediaSealer::new(&SessionSecret::new([0xA5; 32]), context()).unwrap();
    let mut controller = controller(Arc::clone(&counters));

    controller.start(selection(), &mut pipeline).unwrap();
    enqueue_decoded_block(&mut pipeline, &mut sealer, 0);
    controller.output_mut().health = OutputHealth::StreamError;

    controller.stop();

    assert_eq!(controller.monitor(), None);
    assert_eq!(controller.output().stop_calls, 1);
    assert_eq!(pipeline.queued_pcm_blocks(), 1);
    let stopped = counters.snapshot();
    assert_eq!(stopped.stream_resets, 1);
    assert_eq!(stopped.nominal_input_sample_rate_hz, 0);
    assert_eq!(stopped.nominal_output_sample_rate_hz, 0);

    controller.output_mut().health = OutputHealth::Healthy;
    controller.recover(&mut pipeline).unwrap();

    assert_eq!(controller.output().start_calls, 2);
    assert_eq!(pipeline.queued_pcm_blocks(), 0);
    let recovered = counters.snapshot();
    assert_eq!(recovered.stream_resets, 2);
    assert_eq!(recovered.nominal_input_sample_rate_hz, 48_000);
    assert_eq!(recovered.nominal_output_sample_rate_hz, 48_000);
}

#[test]
fn duplicate_start_and_recovery_preserve_the_active_receiver_pipeline() {
    let counters = Arc::new(StreamCounters::default());
    let mut pipeline = pipeline(Arc::clone(&counters));
    let mut sealer = MediaSealer::new(&SessionSecret::new([0xA5; 32]), context()).unwrap();
    let mut controller = controller(Arc::clone(&counters));

    controller.start(selection(), &mut pipeline).unwrap();
    enqueue_decoded_block(&mut pipeline, &mut sealer, 0);
    assert_eq!(pipeline.queued_pcm_blocks(), 1);

    assert!(matches!(
        controller.start(selection(), &mut pipeline),
        Err(ReceiverOutputLifecycleError::StreamAlreadyActive)
    ));
    assert!(matches!(
        controller.recover(&mut pipeline),
        Err(ReceiverOutputLifecycleError::StreamAlreadyActive)
    ));
    assert_eq!(controller.output().start_calls, 1);
    assert_eq!(pipeline.queued_pcm_blocks(), 1);
    assert_eq!(counters.snapshot().stream_resets, 1);
}

#[test]
fn controller_accepts_the_udp_receiver_pipeline_boundary() {
    let counters = Arc::new(StreamCounters::default());
    let mut pipeline = udp_pipeline(Arc::clone(&counters));
    let mut controller = controller(Arc::clone(&counters));

    controller.start(selection(), &mut pipeline).unwrap();

    let snapshot = counters.snapshot();
    assert_eq!(snapshot.stream_resets, 1);
    assert_eq!(snapshot.nominal_input_sample_rate_hz, 48_000);
    assert_eq!(snapshot.nominal_output_sample_rate_hz, 48_000);
}

#[test]
fn output_snapshot_tracks_idle_streaming_and_reported_faults() {
    let counters = Arc::new(StreamCounters::default());
    let mut output = controller(Arc::clone(&counters));
    let mut pipeline = pipeline(counters);

    let idle = output.snapshot();
    assert!(!idle.selection_retained());
    assert!(!idle.stream_active());
    assert!(!idle.fault_reported());
    assert_eq!(idle.health(), OutputHealth::Healthy);

    output.start(selection(), &mut pipeline).unwrap();
    let streaming = output.snapshot();
    assert!(streaming.selection_retained());
    assert!(streaming.stream_active());
    assert!(!streaming.fault_reported());

    output.output_mut().health = OutputHealth::StreamError;
    assert_eq!(output.monitor(), Some(OutputHealth::StreamError));
    let failed = output.snapshot();
    assert!(failed.selection_retained());
    assert!(!failed.stream_active());
    assert!(failed.fault_reported());
    assert_eq!(failed.health(), OutputHealth::StreamError);
}
