//! End-to-end loopback tests over real UDP sockets through the receiver output
//! controller into the render callback.
//!
//! The decoded stream passes through the nominal sample-rate converter even at
//! a 48 kHz-to-48 kHz match, so the tests drive a constant-value stream and
//! assert the rendered callback carries that value after the converter's block
//! phase. Per-frame ordering through the UDP path is proven separately in
//! `media_session_sender_loopback`.

use core::convert::Infallible;
use std::sync::Arc;

use sonimesh_app::{
    OutputSelection, ReceiverOutputController, ReceiverPcmEndpointFactory, SourcePcmFormat,
    UdpPollOutcome, UdpReceiverError, UdpReceiverPipeline,
};
use sonimesh_audio::{
    DecodeStep, FrameDecoder, InsertOutcome, MAX_PCM_QUEUE_BLOCKS, RealtimePcmPushOutcome,
};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_network::{MediaSessionSendOutcome, MediaSessionSender};
use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceInfo, OutputError, OutputHealth, RenderSource,
    SampleFormat,
};
use sonimesh_protocol::{MediaDirection, MediaKeyContext, SessionSecret};

/// Frames to queue before one 480-sample render (120 decoded blocks).
const PHASE_FRAMES: u32 = 128;
const CALLBACK_FRAMES: usize = 480;

/// Decoder that fills one whole PCM block with the payload byte's value.
struct BlockDecoder;

impl FrameDecoder for BlockDecoder {
    type Error = Infallible;

    fn decode(&mut self, encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
        output.fill(f32::from(encoded[0]));
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

fn secret() -> SessionSecret {
    SessionSecret::new([0xA5; 32])
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

type Pipeline = UdpReceiverPipeline<BlockDecoder, 4, 8, 4>;

struct Harness {
    pipeline: Pipeline,
    controller: ReceiverOutputController<TestOutput, 4>,
    counters: Arc<StreamCounters>,
}

fn harness() -> Harness {
    let counters = Arc::new(StreamCounters::default());
    let (producer, _consumer) = sonimesh_audio::realtime_pcm_queue::<4>(4).unwrap();
    let pipeline = Pipeline::bind(
        "127.0.0.1:0".parse().unwrap(),
        &secret(),
        context(),
        BlockDecoder,
        producer,
        Arc::clone(&counters),
        0,
    )
    .unwrap();
    let controller = ReceiverOutputController::new(
        TestOutput {
            health: OutputHealth::Healthy,
            start_calls: 0,
            stop_calls: 0,
            source: None,
        },
        ReceiverPcmEndpointFactory::new(
            MAX_PCM_QUEUE_BLOCKS,
            SourcePcmFormat::new(48_000, 1).unwrap(),
            Arc::clone(&counters),
        ),
    );
    Harness {
        pipeline,
        controller,
        counters,
    }
}

fn sender(harness: &Harness) -> MediaSessionSender {
    MediaSessionSender::bind(
        "127.0.0.1:0".parse().unwrap(),
        &secret(),
        context(),
        harness.pipeline.local_addr().unwrap(),
    )
    .unwrap()
}

fn poll_until_processed(
    pipeline: &mut Pipeline,
) -> Result<UdpPollOutcome, UdpReceiverError<Infallible>> {
    for _ in 0..1_000 {
        match pipeline.poll() {
            Ok(UdpPollOutcome::Empty) => std::thread::yield_now(),
            result => return result,
        }
    }
    panic!("loopback datagram was not delivered");
}

/// Sends `count` frames of one constant value over UDP and decodes each.
fn send_phase(
    sender: &mut MediaSessionSender,
    harness: &mut Harness,
    start_frame: u32,
    value: u8,
    count: u32,
) {
    for offset in 0..count {
        let frame = start_frame + offset;
        let outcome = sender
            .send_frame(0, u64::from(frame) * 960, &[value])
            .unwrap();
        assert_eq!(outcome, MediaSessionSendOutcome::Sent { sequence: frame });

        let UdpPollOutcome::Processed { admission, .. } =
            poll_until_processed(&mut harness.pipeline).unwrap()
        else {
            panic!("loopback media frame {frame} was not processed");
        };
        assert_eq!(admission, InsertOutcome::Inserted);

        match harness.pipeline.decode_next().unwrap() {
            DecodeStep::Decoded {
                sequence,
                samples,
                queue,
            } => {
                assert_eq!(sequence, frame);
                assert_eq!(samples, 4);
                assert_eq!(queue, RealtimePcmPushOutcome::Inserted);
            }
            other => panic!("loopback frame {frame} must decode, found {other:?}"),
        }
    }
}

fn render_and_expect(harness: &mut Harness, expected: f32) {
    let mut callback = [f32::NAN; CALLBACK_FRAMES];
    harness.controller.output_mut().render(&mut callback);
    assert!(
        callback
            .iter()
            .all(|sample| (*sample - expected).abs() < f32::EPSILON),
        "rendered callback did not carry value {expected}: first={:?} last={:?}",
        &callback[..8],
        &callback[CALLBACK_FRAMES - 8..]
    );
}

#[test]
fn udp_stream_renders_through_the_active_output() {
    let mut harness = harness();
    harness
        .controller
        .start(selection(), &mut harness.pipeline)
        .unwrap();
    let mut sender = sender(&harness);

    send_phase(&mut sender, &mut harness, 0, 0x01, PHASE_FRAMES);
    render_and_expect(&mut harness, 1.0);

    assert_eq!(
        harness.counters.snapshot().authenticated_packets,
        u64::from(PHASE_FRAMES)
    );
    assert_eq!(harness.counters.snapshot().rejected_packets, 0);
    assert_eq!(harness.controller.output().start_calls, 1);
}

#[test]
fn udp_stream_keeps_rendering_after_output_recovery() {
    let mut harness = harness();
    harness
        .controller
        .start(selection(), &mut harness.pipeline)
        .unwrap();
    let mut sender = sender(&harness);

    send_phase(&mut sender, &mut harness, 0, 0x01, PHASE_FRAMES);
    render_and_expect(&mut harness, 1.0);

    // A backend fault retires the active output and its paired endpoint.
    harness.controller.output_mut().health = OutputHealth::StreamError;
    assert_eq!(
        harness.controller.monitor(),
        Some(OutputHealth::StreamError)
    );
    harness.controller.recover(&mut harness.pipeline).unwrap();

    // The same pinned source keeps streaming into the recreated source.
    assert_eq!(
        harness.pipeline.pinned_source(),
        Some(sender.local_addr().unwrap())
    );
    send_phase(&mut sender, &mut harness, PHASE_FRAMES, 0x02, PHASE_FRAMES);
    render_and_expect(&mut harness, 2.0);

    assert_eq!(
        harness.counters.snapshot().authenticated_packets,
        u64::from(PHASE_FRAMES) * 2
    );
    assert_eq!(harness.controller.output().start_calls, 2);
    assert_eq!(harness.controller.output().stop_calls, 1);
}

#[test]
fn key_epoch_rotation_keeps_the_active_output_streaming() {
    let mut harness = harness();
    harness
        .controller
        .start(selection(), &mut harness.pipeline)
        .unwrap();
    let mut sender = sender(&harness);

    send_phase(&mut sender, &mut harness, 0, 0x01, PHASE_FRAMES);
    render_and_expect(&mut harness, 1.0);
    assert_eq!(
        harness.pipeline.pinned_source(),
        Some(sender.local_addr().unwrap())
    );

    // Rotate both sides to a strictly newer epoch with a fresh secret; the
    // pinned source and the active output survive without a platform restart.
    let epoch_two = MediaKeyContext::new(
        0x0102_0304_0506_0708,
        9,
        2,
        MediaDirection::AndroidToDesktop,
    )
    .unwrap();
    let rotated_secret = SessionSecret::new([0xB5; 32]);
    sender.rotate(&rotated_secret, epoch_two).unwrap();
    harness
        .pipeline
        .rotate(&rotated_secret, epoch_two, 0)
        .unwrap();

    // Retire the epoch-one blocks still queued after the first callback, then
    // send the epoch-two phase; the new frames restart at sequence zero.
    let mut drain = [0.0_f32; CALLBACK_FRAMES];
    harness.controller.output_mut().render(&mut drain);
    send_phase(&mut sender, &mut harness, 0, 0x02, PHASE_FRAMES);

    // The first post-rotation callback intentionally fades over the bounded
    // discontinuity window instead of clicking, then reaches the new value.
    let mut callback = [f32::NAN; CALLBACK_FRAMES];
    harness.controller.output_mut().render(&mut callback);
    assert!(
        callback[CALLBACK_FRAMES - 200..]
            .iter()
            .all(|sample| (sample - 2.0).abs() < f32::EPSILON),
        "rotated stream did not reach value 2.0: first={:?} tail={:?}",
        &callback[..8],
        &callback[CALLBACK_FRAMES - 8..]
    );
    assert!(
        harness.counters.snapshot().pcm_discontinuities >= 1,
        "rotation must count one bounded PCM discontinuity"
    );

    assert_eq!(
        harness.pipeline.pinned_source(),
        Some(sender.local_addr().unwrap())
    );
    assert_eq!(
        harness.counters.snapshot().authenticated_packets,
        u64::from(PHASE_FRAMES) * 2
    );
    assert_eq!(harness.counters.snapshot().rejected_packets, 0);
    assert_eq!(harness.controller.output().start_calls, 1);
}
