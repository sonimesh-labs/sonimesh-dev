//! Real-component clock-skew closed-loop tests for the mixer session.
//!
//! These tests drive a virtual source clock at a fixed offset from the output
//! clock through the actual receiver pipeline, per-stream polyphase format
//! adapter, fixed-slot mixer, and `MixerStreamDriftController`. One control
//! tick decodes 10 ms of source audio (10 source blocks), samples the adapter
//! source occupancy, applies one drift decision, pumps the 10 output blocks,
//! and renders them. This is the deterministic closed loop that the integer
//! `tools/network-simulator` drift scenarios abstract: here every step runs
//! through the real components, including the 16-frame windowed-sinc lookahead
//! retained inside the adapter.
//!
//! The evidence level matches the model scenarios: sustained skew must keep
//! occupancy inside the bounded PCM queue, publish a correction with the
//! correct sign for a within-cap skew, avoid emergency resynchronizations for
//! a correctable skew, and degrade to bounded hard resynchronizations when the
//! skew exceeds the correction cap. The controller works in whole-block
//! occupancy units and the adapter consumes whole input frames per pump, so
//! ppm authority is quantized and convergence is slow; the tests therefore
//! assert boundedness and direction, not exact ppm convergence.

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

/// One stream occupies the only mixer slot.
const SLOTS: usize = 1;
/// One output block is 1 ms at 48 kHz; each source block is 1 ms at 8 kHz.
const BLOCK: usize = 48;
/// The decode pipeline compiles for the 8-frame source block.
const PIPELINE_BLOCK: usize = 8;
/// Source frames per source block at 8 kHz and a 1 ms frame duration.
const SOURCE_BLOCK_FRAMES: usize = 8;
/// Bounded source and output queue capacity in blocks.
const QUEUE_CAPACITY: usize = 32;
/// Source ms decoded, adapter pumps, and rendered output blocks per control tick.
const MS_PER_TICK: usize = 10;
/// Control ticks simulated; 3_000 ticks at 10 ms is 30 s of audio.
const TICKS: usize = 3_000;

/// Bounded occupancy PI configuration used by every skew scenario.
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
        // 8_000 Hz x 1 ms = 8 frames per block, matching PIPELINE_BLOCK.
        sample_rate_hz: 8_000,
        channels: 1,
        frame_duration_ms: 1,
    }
}

/// Deterministic summary of one sustained clock-skew run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SkewRun {
    hard_resyncs: usize,
    min_occupancy: usize,
    max_occupancy: usize,
    max_output_blocks: usize,
    mean_adjustment_ppm: i64,
    final_adjustment_ppm: i32,
    nonzero_renders: usize,
    silence_renders: usize,
    nonfinite_renders: usize,
}

#[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
/// Runs one deterministic closed-loop skew scenario through the real components.
///
/// A virtual source clock produces `MS_PER_TICK` source blocks per tick scaled
/// by `skew_ppm`; fractional frames accumulate so the average rate is exact.
/// Each tick decodes the burst, samples occupancy, applies one drift decision,
/// pumps the output blocks, and renders them. The summary covers the whole
/// measured phase (after the first tick, which includes the start-up render
/// transient).
fn simulate_skew(skew_ppm: i64) -> SkewRun {
    let counters = Arc::new(StreamCounters::default());
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
    let mut pipeline: Pipeline = pipeline(Arc::clone(&counters));
    let mut sealer = sealer();
    let identity = request(1, 1).identity;

    session
        .start_output(selection())
        .expect("test output must start");
    session
        .start_stream::<_, PIPELINE_BLOCK>(
            request(1, 1),
            QUEUE_CAPACITY,
            Some(Arc::clone(&counters)),
            &mut pipeline,
        )
        .expect("test stream must be admitted");
    let mut controller =
        MixerStreamDriftController::new(identity, DRIFT, None).expect("test drift config is valid");

    let mut frame_accum: f64 = 0.0;
    let mut sequence: u32 = 0;
    let mut adjustment_sum: i64 = 0;
    let mut adjustment_samples: i64 = 0;
    let mut run = SkewRun {
        hard_resyncs: 0,
        min_occupancy: usize::MAX,
        max_occupancy: 0,
        max_output_blocks: 0,
        mean_adjustment_ppm: 0,
        final_adjustment_ppm: 0,
        nonzero_renders: 0,
        silence_renders: 0,
        nonfinite_renders: 0,
    };

    for _ in 0..TICKS {
        // 1) Decode one control-tick burst from the virtual source clock.
        frame_accum +=
            SOURCE_BLOCK_FRAMES as f64 * MS_PER_TICK as f64 * (1.0 + skew_ppm as f64 / 1e6);
        while frame_accum >= SOURCE_BLOCK_FRAMES as f64 {
            let payload = [0x2A; 8];
            let packet = sealer
                .seal(0, u64::from(sequence) * 8, &payload)
                .expect("test packet must seal");
            let mut plaintext = [0_u8; 8];
            pipeline
                .receive(&packet.datagram, &mut plaintext)
                .expect("test packet must be accepted");
            pipeline.decode_next().expect("test frame must decode");
            sequence = sequence.wrapping_add(1);
            frame_accum -= SOURCE_BLOCK_FRAMES as f64;
        }

        // 2) Sample the adapter source occupancy and apply one drift decision.
        let occupancy = session
            .queued_source_blocks(identity)
            .expect("stream must stay admitted");
        run.min_occupancy = run.min_occupancy.min(occupancy);
        run.max_occupancy = run.max_occupancy.max(occupancy);
        match controller
            .update(&mut session)
            .expect("drift update must succeed")
        {
            DriftDecision::Adjust { ppm } => {
                adjustment_sum += i64::from(ppm);
                adjustment_samples += 1;
            }
            DriftDecision::HardResync {
                direction: DriftResyncDirection::Low | DriftResyncDirection::High,
            } => run.hard_resyncs += 1,
        }

        // 3) Pump one 1 ms output block per decoded source ms.
        for _ in 0..MS_PER_TICK {
            session.pump_all().expect("adapter pump must succeed");
        }
        run.max_output_blocks = run.max_output_blocks.max(
            session
                .queued_output_blocks(identity)
                .expect("stream must stay admitted"),
        );

        // 4) Render one output block per pump.
        for _ in 0..MS_PER_TICK {
            let mut rendered = [0.0_f32; BLOCK];
            session.output_mut().render(&mut rendered);
            if rendered.iter().all(|sample| sample.is_finite()) {
                if rendered.iter().any(|sample| *sample != 0.0) {
                    run.nonzero_renders += 1;
                } else {
                    run.silence_renders += 1;
                }
            } else {
                run.nonfinite_renders += 1;
            }
        }
    }

    run.final_adjustment_ppm = controller.adjustment_ppm();
    if adjustment_samples > 0 {
        run.mean_adjustment_ppm = adjustment_sum / adjustment_samples;
    } else {
        run.mean_adjustment_ppm = 0;
    }
    run
}

fn pipeline(counters: Arc<StreamCounters>) -> Pipeline {
    let secret = SessionSecret::new([0xA5; 32]);
    let (producer, _consumer) =
        realtime_pcm_queue::<PIPELINE_BLOCK>(4).expect("bounded test queue is valid");
    sonimesh_app::ReceiverStreamPipeline::new(
        &secret,
        context(1, 1),
        ConstantDecoder(0.25),
        producer,
        counters,
        0,
    )
    .expect("test pipeline is valid")
}

fn sealer() -> MediaSealer {
    MediaSealer::new(&SessionSecret::new([0xA5; 32]), context(1, 1)).expect("test sealer is valid")
}

#[test]
fn zero_skew_stays_at_target_without_resync_or_correction() {
    let run = simulate_skew(0);
    assert_eq!(run.hard_resyncs, 0);
    // The natural operating point sits at the target; a nominal clock must not
    // request a sustained correction.
    assert!(
        run.mean_adjustment_ppm.abs() <= 50,
        "nominal clock requested {} ppm on average",
        run.mean_adjustment_ppm
    );
    assert_eq!(run.final_adjustment_ppm, 0);
    // Occupancy stays inside the emergency band around the target.
    assert!(
        (DRIFT.low_resync_blocks as usize + 1..DRIFT.high_resync_blocks as usize)
            .contains(&run.min_occupancy)
            && (DRIFT.low_resync_blocks as usize + 1..DRIFT.high_resync_blocks as usize)
                .contains(&run.max_occupancy),
        "occupancy escaped the emergency band: [{}, {}]",
        run.min_occupancy,
        run.max_occupancy
    );
    assert_eq!(run.max_output_blocks, MS_PER_TICK);
    assert_eq!(run.nonfinite_renders, 0);
    // Only the start-up lookahead fill may underflow.
    assert!(
        run.silence_renders <= 10,
        "{} silent renders",
        run.silence_renders
    );
    assert!(run.nonzero_renders >= TICKS * MS_PER_TICK - 10);
}

#[test]
fn fast_source_publishes_positive_correction_and_stays_bounded() {
    let run = simulate_skew(200);
    assert_eq!(run.hard_resyncs, 0);
    assert!(
        run.mean_adjustment_ppm >= 800,
        "fast source requested only {} ppm on average",
        run.mean_adjustment_ppm
    );
    assert!(
        run.max_occupancy < DRIFT.high_resync_blocks as usize,
        "fast source occupancy reached the high threshold: {}",
        run.max_occupancy
    );
    assert!(run.min_occupancy > DRIFT.low_resync_blocks as usize);
    assert!(run.max_occupancy <= QUEUE_CAPACITY);
    assert_eq!(run.nonfinite_renders, 0);
    assert!(run.silence_renders <= 10);
    assert!(run.nonzero_renders >= TICKS * MS_PER_TICK - 10);
}

#[test]
fn slow_source_publishes_negative_correction_and_stays_bounded() {
    let run = simulate_skew(-800);
    assert_eq!(run.hard_resyncs, 0);
    assert!(
        run.mean_adjustment_ppm <= -20,
        "slow source requested {} ppm on average",
        run.mean_adjustment_ppm
    );
    assert!(
        run.min_occupancy > DRIFT.low_resync_blocks as usize,
        "slow source occupancy reached the low threshold: {}",
        run.min_occupancy
    );
    assert!(run.max_occupancy < DRIFT.high_resync_blocks as usize);
    assert_eq!(run.nonfinite_renders, 0);
    assert!(run.silence_renders <= 10);
    assert!(run.nonzero_renders >= TICKS * MS_PER_TICK - 10);
}

#[test]
fn uncompensable_skew_stays_bounded_with_hard_resyncs() {
    let run = simulate_skew(4_000);
    assert!(
        run.hard_resyncs >= 10,
        "uncompensable skew requested only {} resyncs",
        run.hard_resyncs
    );
    assert!(
        run.max_occupancy <= QUEUE_CAPACITY,
        "occupancy escaped the bounded queue: {}",
        run.max_occupancy
    );
    assert!(run.max_output_blocks <= QUEUE_CAPACITY);
    assert_eq!(run.nonfinite_renders, 0);
    // Emergency resynchronizations retire output but must not stop rendering.
    assert!(run.nonzero_renders >= TICKS * MS_PER_TICK - 10);
}

#[test]
fn clock_skew_run_is_deterministic() {
    let first = simulate_skew(200);
    let second = simulate_skew(200);
    assert_eq!(first, second);
}
