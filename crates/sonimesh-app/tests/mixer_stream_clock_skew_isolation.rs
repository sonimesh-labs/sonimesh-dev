//! Real-component multi-stream clock-skew isolation tests for the mixer session.
//!
//! Two admitted streams share one fixed-slot mixer and one output device.
//! Stream A runs a skew beyond the per-stream drift correction cap and
//! therefore triggers repeated hard resynchronizations; stream B runs a
//! within-cap skew. Each control tick decodes 10 ms of source audio for both
//! streams, samples both adapter source queues, applies both drift decisions,
//! pumps both adapters, and renders the single mixed output block. The tests
//! prove that stream A's emergency resynchronizations never disturb stream
//! B's occupancy, correction, or rendering, and that both streams stay inside
//! the bounded queues.

use core::convert::Infallible;
use std::sync::Arc;

use sonimesh_app::{
    MediaStreamIdentity, MixerStreamDriftController, MixerStreamSession, StreamStartRequest,
};
use sonimesh_audio::{
    DriftConfig, DriftDecision, DriftResyncDirection, FrameDecoder, realtime_pcm_queue,
};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceInfo, OutputError, OutputHealth, RenderSource,
    SampleFormat,
};
use sonimesh_protocol::{MediaDirection, MediaKeyContext, MediaSealer, SessionSecret};

/// Two streams share the mixer; each occupies one slot.
const SLOTS: usize = 2;
/// One output block is 1 ms at 48 kHz; each source block is 1 ms at 8 kHz.
const BLOCK: usize = 48;
/// The decode pipeline compiles for the 8-frame source block.
const PIPELINE_BLOCK: usize = 8;
/// Source frames per source block at 8 kHz and a 1 ms frame duration.
const SOURCE_BLOCK_FRAMES: usize = 8;
/// Bounded source queue capacity for each stream, in blocks.
const QUEUE_CAPACITY: usize = 32;
/// Source ms decoded, adapter pumps, and rendered output blocks per control tick.
const MS_PER_TICK: usize = 10;
/// Control ticks simulated; 3_000 ticks at 10 ms is 30 s of audio.
const TICKS: usize = 3_000;

/// Bounded occupancy PI configuration shared by both streams.
const DRIFT: DriftConfig = DriftConfig {
    target_blocks: 10,
    low_resync_blocks: 4,
    high_resync_blocks: 22,
    proportional_ppm_per_block: 25,
    integral_ppm_per_block_tick: 5,
    max_adjustment_ppm: 2_000,
    max_step_ppm: 100,
    max_integral_block_ticks: 500,
};

type Session = MixerStreamSession<TestOutput, SLOTS, BLOCK>;
type Pipeline = sonimesh_app::ReceiverStreamPipeline<ConstantDecoder, 4, 8, PIPELINE_BLOCK>;

/// Stream A identity (session 1) and stream B identity (session 2).
const IDENTITY_A: MediaStreamIdentity = MediaStreamIdentity {
    session_id: 1,
    stream_id: 1,
    key_epoch: 0,
};
const IDENTITY_B: MediaStreamIdentity = MediaStreamIdentity {
    session_id: 2,
    stream_id: 1,
    key_epoch: 0,
};

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

fn context(session_id: u64) -> MediaKeyContext {
    MediaKeyContext::new(session_id, 1, 0, MediaDirection::AndroidToDesktop).unwrap()
}

fn request(identity: MediaStreamIdentity) -> StreamStartRequest {
    StreamStartRequest {
        identity,
        // 8_000 Hz x 1 ms = 8 frames per block, matching PIPELINE_BLOCK.
        sample_rate_hz: 8_000,
        channels: 1,
        frame_duration_ms: 1,
    }
}

/// Per-stream deterministic closed-loop summary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StreamRun {
    hard_resyncs: usize,
    min_occupancy: usize,
    max_occupancy: usize,
    max_output_blocks: usize,
    mean_adjustment_ppm: i64,
    final_adjustment_ppm: i32,
}

/// Mixed-output render summary (both streams sum into one mono block).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RenderRun {
    nonzero: usize,
    silence: usize,
    nonfinite: usize,
}

#[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
/// Runs one deterministic two-stream closed-loop skew scenario.
///
/// Stream A uses `skew_a_ppm` and stream B uses `skew_b_ppm`. Each tick
/// decodes both bursts from their virtual clocks, samples both occupancies,
/// applies both drift decisions, pumps both adapters, and renders once.
fn simulate_two_streams(skew_a: i64, skew_b: i64) -> (StreamRun, StreamRun, RenderRun) {
    let mut session: Session = MixerStreamSession::new(
        TestOutput {
            health: OutputHealth::Healthy,
            start_calls: 0,
            stop_calls: 0,
            fail_start: false,
            source: None,
        },
        QUEUE_CAPACITY,
    )
    .expect("bounded test session is valid");
    let mut pipeline_a: Pipeline = pipeline(1, Arc::new(StreamCounters::default()));
    let mut pipeline_b: Pipeline = pipeline(2, Arc::new(StreamCounters::default()));
    let mut sealer_a = sealer(1);
    let mut sealer_b = sealer(2);

    session
        .start_output(selection())
        .expect("test output must start");
    session
        .start_stream::<_, PIPELINE_BLOCK>(
            request(IDENTITY_A),
            QUEUE_CAPACITY,
            None,
            &mut pipeline_a,
        )
        .expect("stream A must be admitted");
    session
        .start_stream::<_, PIPELINE_BLOCK>(
            request(IDENTITY_B),
            QUEUE_CAPACITY,
            None,
            &mut pipeline_b,
        )
        .expect("stream B must be admitted");
    assert_eq!(session.stream_count(), 2);
    let mut controller_a = MixerStreamDriftController::new(IDENTITY_A, DRIFT, None)
        .expect("stream A drift config is valid");
    let mut controller_b = MixerStreamDriftController::new(IDENTITY_B, DRIFT, None)
        .expect("stream B drift config is valid");

    let mut virtual_a: f64 = 0.0;
    let mut virtual_b: f64 = 0.0;
    let mut sequence_a: u32 = 0;
    let mut sequence_b: u32 = 0;
    let mut adjustment_sum_a: i64 = 0;
    let mut adjustment_sum_b: i64 = 0;
    let mut adjustment_samples: i64 = 0;
    let mut run_a = StreamRun {
        hard_resyncs: 0,
        min_occupancy: usize::MAX,
        max_occupancy: 0,
        max_output_blocks: 0,
        mean_adjustment_ppm: 0,
        final_adjustment_ppm: 0,
    };
    let mut run_b = run_a;
    let mut render = RenderRun {
        nonzero: 0,
        silence: 0,
        nonfinite: 0,
    };

    for _ in 0..TICKS {
        virtual_a += SOURCE_BLOCK_FRAMES as f64 * MS_PER_TICK as f64 * (1.0 + skew_a as f64 / 1e6);
        while virtual_a >= SOURCE_BLOCK_FRAMES as f64 {
            feed_block(&mut pipeline_a, &mut sealer_a, &mut sequence_a);
            virtual_a -= SOURCE_BLOCK_FRAMES as f64;
        }
        virtual_b += SOURCE_BLOCK_FRAMES as f64 * MS_PER_TICK as f64 * (1.0 + skew_b as f64 / 1e6);
        while virtual_b >= SOURCE_BLOCK_FRAMES as f64 {
            feed_block(&mut pipeline_b, &mut sealer_b, &mut sequence_b);
            virtual_b -= SOURCE_BLOCK_FRAMES as f64;
        }

        let occupancy_a = session
            .queued_source_blocks(IDENTITY_A)
            .expect("stream A must stay admitted");
        let occupancy_b = session
            .queued_source_blocks(IDENTITY_B)
            .expect("stream B must stay admitted");
        run_a.min_occupancy = run_a.min_occupancy.min(occupancy_a);
        run_a.max_occupancy = run_a.max_occupancy.max(occupancy_a);
        run_b.min_occupancy = run_b.min_occupancy.min(occupancy_b);
        run_b.max_occupancy = run_b.max_occupancy.max(occupancy_b);

        match controller_a
            .update(&mut session)
            .expect("stream A drift update must succeed")
        {
            DriftDecision::Adjust { ppm } => {
                adjustment_sum_a += i64::from(ppm);
                adjustment_samples += 1;
            }
            DriftDecision::HardResync {
                direction: DriftResyncDirection::Low | DriftResyncDirection::High,
            } => run_a.hard_resyncs += 1,
        }
        match controller_b
            .update(&mut session)
            .expect("stream B drift update must succeed")
        {
            DriftDecision::Adjust { ppm } => adjustment_sum_b += i64::from(ppm),
            DriftDecision::HardResync {
                direction: DriftResyncDirection::Low | DriftResyncDirection::High,
            } => run_b.hard_resyncs += 1,
        }

        for _ in 0..MS_PER_TICK {
            session.pump_all().expect("adapter pump must succeed");
        }
        run_a.max_output_blocks = run_a.max_output_blocks.max(
            session
                .queued_output_blocks(IDENTITY_A)
                .expect("stream A must stay admitted"),
        );
        run_b.max_output_blocks = run_b.max_output_blocks.max(
            session
                .queued_output_blocks(IDENTITY_B)
                .expect("stream B must stay admitted"),
        );

        let rendered = render_block(&mut session);
        if rendered.iter().all(|sample| sample.is_finite()) {
            if rendered.iter().any(|sample| *sample != 0.0) {
                render.nonzero += 1;
            } else {
                render.silence += 1;
            }
        } else {
            render.nonfinite += 1;
        }
    }

    run_a.final_adjustment_ppm = controller_a.adjustment_ppm();
    run_b.final_adjustment_ppm = controller_b.adjustment_ppm();
    if adjustment_samples > 0 {
        run_a.mean_adjustment_ppm = adjustment_sum_a / adjustment_samples;
        run_b.mean_adjustment_ppm = adjustment_sum_b / adjustment_samples;
    }
    (run_a, run_b, render)
}

/// Seals, receives, and decodes one source block for one stream.
fn feed_block(pipeline: &mut Pipeline, sealer: &mut MediaSealer, sequence: &mut u32) {
    let payload = [0x2A; 8];
    let packet = sealer
        .seal(0, u64::from(*sequence) * 8, &payload)
        .expect("test packet must seal");
    let mut plaintext = [0_u8; 8];
    pipeline
        .receive(&packet.datagram, &mut plaintext)
        .expect("test packet must be accepted");
    pipeline.decode_next().expect("test frame must decode");
    *sequence = sequence.wrapping_add(1);
}

/// Renders one mixed output block through the session's output source.
fn render_block(session: &mut Session) -> [f32; BLOCK] {
    let mut rendered = [0.0_f32; BLOCK];
    session.output_mut().render(&mut rendered);
    rendered
}

fn pipeline(session_id: u64, counters: Arc<StreamCounters>) -> Pipeline {
    let secret = SessionSecret::new([0xA5; 32]);
    let (producer, _consumer) =
        realtime_pcm_queue::<PIPELINE_BLOCK>(4).expect("bounded test queue is valid");
    sonimesh_app::ReceiverStreamPipeline::new(
        &secret,
        context(session_id),
        ConstantDecoder(0.25),
        producer,
        counters,
        0,
    )
    .expect("test pipeline is valid")
}

fn sealer(session_id: u64) -> MediaSealer {
    MediaSealer::new(&SessionSecret::new([0xA5; 32]), context(session_id))
        .expect("test sealer is valid")
}

#[test]
fn uncompensable_stream_resync_does_not_disturb_a_stable_stream() {
    // Stream A skew exceeds the correction cap and keeps triggering hard
    // resynchronizations; stream B runs at nominal and must stay at target.
    let (run_a, run_b, render) = simulate_two_streams(4_000, 0);

    assert!(
        run_a.hard_resyncs >= 10,
        "uncompensable stream A requested only {} resyncs",
        run_a.hard_resyncs
    );
    assert!(run_a.max_occupancy <= QUEUE_CAPACITY);
    assert!(run_a.max_output_blocks <= QUEUE_CAPACITY);

    // Stream B is untouched: no resync, nominal correction, target occupancy.
    assert_eq!(run_b.hard_resyncs, 0);
    assert!(
        run_b.mean_adjustment_ppm.abs() <= 50,
        "stable stream B requested {} ppm on average",
        run_b.mean_adjustment_ppm
    );
    assert_eq!(run_b.final_adjustment_ppm, 0);
    assert!(
        (DRIFT.low_resync_blocks as usize + 1..DRIFT.high_resync_blocks as usize)
            .contains(&run_b.min_occupancy)
            && (DRIFT.low_resync_blocks as usize + 1..DRIFT.high_resync_blocks as usize)
                .contains(&run_b.max_occupancy),
        "stable stream B occupancy escaped the emergency band: [{}, {}]",
        run_b.min_occupancy,
        run_b.max_occupancy
    );

    assert_eq!(render.nonfinite, 0);
    // The mixed output keeps rendering; stream A's resyncs must not silence
    // stream B's contribution.
    assert!(render.nonzero >= TICKS - 10);
    assert!(render.silence <= 10);
}

#[test]
fn resync_stream_does_not_disturb_a_correcting_stream() {
    // Stream A keeps resynchronizing; stream B runs a within-cap fast skew
    // and must keep publishing its positive correction uninterrupted.
    let (run_a, run_b, render) = simulate_two_streams(4_000, 200);

    assert!(run_a.hard_resyncs >= 10);
    assert!(run_a.max_occupancy <= QUEUE_CAPACITY);
    assert!(run_a.max_output_blocks <= QUEUE_CAPACITY);

    assert_eq!(run_b.hard_resyncs, 0);
    assert!(
        run_b.mean_adjustment_ppm >= 800,
        "correcting stream B requested only {} ppm on average",
        run_b.mean_adjustment_ppm
    );
    assert!(
        run_b.max_occupancy < DRIFT.high_resync_blocks as usize,
        "correcting stream B occupancy reached the high threshold: {}",
        run_b.max_occupancy
    );
    assert!(run_b.min_occupancy > DRIFT.low_resync_blocks as usize);

    assert_eq!(render.nonfinite, 0);
    assert!(render.nonzero >= TICKS - 10);
    assert!(render.silence <= 10);
}

#[test]
fn two_stream_run_is_deterministic() {
    let (a1, b1, r1) = simulate_two_streams(4_000, 200);
    let (a2, b2, r2) = simulate_two_streams(4_000, 200);
    assert_eq!((a1, b1, r1), (a2, b2, r2));
}
