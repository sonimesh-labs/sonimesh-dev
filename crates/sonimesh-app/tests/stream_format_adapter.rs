//! Decode-domain format adapter tests: source-to-output rate and channel
//! conversion feeding a mixer slot queue.
//!
//! ADR 0015 leaves per-stream sample-rate and channel adaptation before the
//! mixer queue as open work. These tests drive the real `StreamFormatAdapter`
//! pump against bounded queues and verify passthrough, interpolation,
//! channel mapping, queue overflow, discontinuity handling, and drift-bound
//! behavior, all outside the output callback.
//!
//! The adapter uses the windowed-sinc [`PolyphaseResampler`], whose 33-tap
//! kernel keeps 16 future input frames of lookahead. The first output block
//! is therefore a start-up transient, and tests feed enough source frames
//! (whole 8-frame blocks) and skip the first output block when they assert
//! steady-state values.

use sonimesh_app::{PumpOutcome, SourcePcmFormat, StreamFormatAdapter, StreamFormatAdapterError};
use sonimesh_audio::{
    MAX_DRIFT_ADJUSTMENT_PPM, RealtimePcmConsumer, RealtimePcmPopOutcome, RealtimePcmProducer,
    RealtimePcmPushOutcome, ResamplerError, realtime_pcm_queue,
};

const SOURCE_BLOCK: usize = 8;
const OUTPUT_BLOCK: usize = 8;
const TOLERANCE: f32 = 1e-6;
/// Future input frames the windowed-sinc kernel keeps beyond each output
/// center; they stay retained in the adapter between pumps.
const LOOKAHEAD_FRAMES: usize = 16;

fn format(sample_rate_hz: u32, channels: u16) -> SourcePcmFormat {
    SourcePcmFormat::new(sample_rate_hz, channels).unwrap()
}

fn adapter(
    source_format: SourcePcmFormat,
    output_format: SourcePcmFormat,
    source_capacity: usize,
    output_capacity: usize,
) -> (
    RealtimePcmProducer<SOURCE_BLOCK>,
    StreamFormatAdapter<SOURCE_BLOCK, OUTPUT_BLOCK>,
    RealtimePcmConsumer<OUTPUT_BLOCK>,
) {
    let (source_producer, source_consumer) =
        realtime_pcm_queue::<SOURCE_BLOCK>(source_capacity).unwrap();
    let (output_producer, output_consumer) =
        realtime_pcm_queue::<OUTPUT_BLOCK>(output_capacity).unwrap();
    let adapter = StreamFormatAdapter::new(
        source_consumer,
        output_producer,
        source_format,
        output_format,
    )
    .unwrap();
    (source_producer, adapter, output_consumer)
}

/// Pushes frame-aligned interleaved samples in source-block-sized chunks.
fn push_frames(producer: &mut RealtimePcmProducer<SOURCE_BLOCK>, frames: &[f32], channels: usize) {
    assert_eq!(
        frames.len() % channels,
        0,
        "test frames must be frame-aligned"
    );
    assert_eq!(
        SOURCE_BLOCK % channels,
        0,
        "test block must hold whole frames"
    );
    for chunk in frames.chunks(SOURCE_BLOCK) {
        assert_eq!(
            producer.push(chunk),
            RealtimePcmPushOutcome::Inserted,
            "source queue must accept each test block"
        );
    }
}

/// Pumps until the adapter underflows, returning the produced frame count.
fn pump_until_underflow(
    adapter: &mut StreamFormatAdapter<SOURCE_BLOCK, OUTPUT_BLOCK>,
    max_pumps: usize,
) -> usize {
    let mut produced = 0;
    for _ in 0..max_pumps {
        match adapter.pump().unwrap() {
            PumpOutcome::Pushed { frames, .. } => produced += frames,
            PumpOutcome::Underflow => break,
        }
    }
    produced
}

/// Pops up to `pumps` blocks and returns the initialized samples.
fn drain_blocks(consumer: &mut RealtimePcmConsumer<OUTPUT_BLOCK>, pumps: usize) -> Vec<f32> {
    let mut collected = Vec::new();
    for _ in 0..pumps {
        let mut block = [0.0; OUTPUT_BLOCK];
        match consumer.pop_into(&mut block) {
            RealtimePcmPopOutcome::Filled { samples, .. } => {
                collected.extend_from_slice(&block[..samples]);
            }
            RealtimePcmPopOutcome::Underflow { .. } => {}
        }
    }
    collected
}

fn assert_samples(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    assert!(
        actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| (*actual - *expected).abs() <= TOLERANCE)
    );
}

#[test]
fn equal_rate_and_channels_pass_samples_through() {
    let (mut source, mut adapter, mut output) =
        adapter(format(48_000, 1), format(48_000, 1), 16, 16);
    let input: Vec<f32> = (0..64_u16).map(|index| f32::from(index) * 0.01).collect();
    push_frames(&mut source, &input, 1);

    let produced = pump_until_underflow(&mut adapter, 16);
    let drained = drain_blocks(&mut output, 16);

    // The 16-frame lookahead stays retained in the adapter, so passthrough
    // emits the input prefix in order: output[i] == input[i].
    assert_eq!(drained.len(), produced);
    assert_eq!(drained.len(), input.len() - LOOKAHEAD_FRAMES);
    assert_samples(&drained, &input[..input.len() - LOOKAHEAD_FRAMES]);
}

#[test]
fn upsampling_interpolates_between_source_frames() {
    let (mut source, mut adapter, mut output) =
        adapter(format(48_000, 1), format(96_000, 1), 16, 16);
    let input: Vec<f32> = (0..64_u16).map(|index| f32::from(index) * 0.01).collect();
    push_frames(&mut source, &input, 1);

    let produced = pump_until_underflow(&mut adapter, 16);
    let drained = drain_blocks(&mut output, 16);

    assert_eq!(drained.len(), produced);
    assert_eq!(drained.len(), (input.len() - LOOKAHEAD_FRAMES) * 2);
    // Even output samples land exactly on source samples; odd samples
    // interpolate strictly between their neighbors.
    let source_frames = u16::try_from(input.len() - LOOKAHEAD_FRAMES).unwrap();
    for frame in 0..source_frames {
        let even = usize::from(frame) * 2;
        assert!((drained[even] - f32::from(frame) * 0.01).abs() <= TOLERANCE);
        if frame + 1 < source_frames {
            assert!(drained[even] < drained[even + 1] && drained[even + 1] < drained[even + 2]);
        }
    }
}

#[test]
fn downsampling_selects_output_rate_frames() {
    let (mut source, mut adapter, mut output) =
        adapter(format(96_000, 1), format(48_000, 1), 16, 16);
    let input: Vec<f32> = (0..128_u16).map(|index| f32::from(index) * 0.01).collect();
    push_frames(&mut source, &input, 1);

    let produced = pump_until_underflow(&mut adapter, 16);
    let drained = drain_blocks(&mut output, 16);

    assert_eq!(drained.len(), produced);
    assert_eq!(drained.len(), (input.len() - LOOKAHEAD_FRAMES) / 2);
    // The first output block is a start-up transient; steady state selects
    // every second source frame: drained[n] == 0.02*n for n >= 8.
    let count = u16::try_from(drained.len()).unwrap();
    let expected: Vec<f32> = (8..count).map(|index| f32::from(index) * 0.02).collect();
    assert_samples(&drained[8..], &expected);
}

#[test]
fn mono_source_is_duplicated_to_stereo_output() {
    let (mut source, mut adapter, mut output) =
        adapter(format(48_000, 1), format(48_000, 2), 16, 16);
    let input: Vec<f32> = (0..64_u16).map(|index| f32::from(index) * 0.01).collect();
    push_frames(&mut source, &input, 1);

    let produced = pump_until_underflow(&mut adapter, 16);
    let drained = drain_blocks(&mut output, 16);

    assert_eq!(drained.len(), produced * 2);
    assert_eq!(produced, input.len() - LOOKAHEAD_FRAMES);
    let mut expected = Vec::new();
    for frame in 0..u16::try_from(input.len() - LOOKAHEAD_FRAMES).unwrap() {
        let value = f32::from(frame) * 0.01;
        expected.push(value);
        expected.push(value);
    }
    assert_samples(&drained, &expected);
}

#[test]
fn stereo_source_is_folded_to_mono_output() {
    let (mut source, mut adapter, mut output) =
        adapter(format(48_000, 2), format(48_000, 1), 16, 16);
    let mut input = Vec::new();
    for frame in 0..64_u16 {
        input.push(f32::from(frame) * 0.01);
        input.push(0.1 + f32::from(frame) * 0.01);
    }
    push_frames(&mut source, &input, 2);

    let produced = pump_until_underflow(&mut adapter, 16);
    let drained = drain_blocks(&mut output, 16);

    // Folding averages each stereo pair, so 64 stereo frames become 48 mono
    // frames at (0.01*index + 0.1 + 0.01*index) / 2 = 0.05 + 0.01*index.
    assert_eq!(drained.len(), produced);
    assert_eq!(produced, input.len() / 2 - LOOKAHEAD_FRAMES);
    let count = u16::try_from(produced).unwrap();
    let expected: Vec<f32> = (0..count)
        .map(|index| f32::from(index) * 0.01 + 0.05)
        .collect();
    assert_samples(&drained, &expected);
}

#[test]
fn rate_and_channel_adaptation_compose() {
    let (mut source, mut adapter, mut output) =
        adapter(format(96_000, 2), format(48_000, 1), 16, 16);
    let mut input = Vec::new();
    for frame in 0..64_u16 {
        input.push(f32::from(frame) * 0.01);
        input.push(0.1 + f32::from(frame) * 0.01);
    }
    push_frames(&mut source, &input, 2);

    let produced = pump_until_underflow(&mut adapter, 16);
    let drained = drain_blocks(&mut output, 16);

    // Downsampling 2x then folding averages each pair: output[n] =
    // 0.05 + 0.02*n once the start-up transient has settled.
    assert_eq!(drained.len(), produced);
    assert_eq!(drained.len(), (input.len() / 2 - LOOKAHEAD_FRAMES) / 2);
    let count = u16::try_from(drained.len()).unwrap();
    let expected: Vec<f32> = (8..count)
        .map(|index| f32::from(index) * 0.02 + 0.05)
        .collect();
    assert_samples(&drained[8..], &expected);
}

#[test]
fn empty_source_underflows_without_pushing() {
    let (_, mut adapter, mut output) = adapter(format(48_000, 1), format(48_000, 1), 16, 16);

    assert_eq!(adapter.pump().unwrap(), PumpOutcome::Underflow);
    assert_eq!(adapter.queued_output_blocks(), 0);
    assert!(drain_blocks(&mut output, 1).is_empty());
}

#[test]
fn partial_source_frame_is_retained_across_pumps() {
    let (mut source, mut adapter, mut output) =
        adapter(format(48_000, 1), format(48_000, 1), 16, 16);
    let head: Vec<f32> = (1..=8_u16).map(f32::from).collect();
    push_frames(&mut source, &head, 1);

    // One block is shorter than the 33-tap window; the pump must underflow
    // and retain the partial input instead of dropping it.
    assert_eq!(adapter.pump().unwrap(), PumpOutcome::Underflow);
    assert_eq!(adapter.queued_output_blocks(), 0);

    let tail: Vec<f32> = (9..=40_u16).map(f32::from).collect();
    push_frames(&mut source, &tail, 1);
    let produced = pump_until_underflow(&mut adapter, 16);
    let drained = drain_blocks(&mut output, 16);

    // 40 retained frames emit 24 passthrough frames in order.
    assert_eq!(drained.len(), produced);
    assert_eq!(drained.len(), head.len() + tail.len() - LOOKAHEAD_FRAMES);
    let expected: Vec<f32> = (1..=24_u16).map(f32::from).collect();
    assert_samples(&drained, &expected);
}

#[test]
fn full_output_queue_replaces_the_oldest_block() {
    let (mut source, mut adapter, _) = adapter(format(48_000, 1), format(48_000, 1), 16, 1);
    let input: Vec<f32> = (0..32_u16).map(f32::from).collect();
    push_frames(&mut source, &input, 1);

    let mut replaced = false;
    for _ in 0..8 {
        match adapter.pump().unwrap() {
            PumpOutcome::Pushed {
                queue: RealtimePcmPushOutcome::ReplacedOldest,
                ..
            } => replaced = true,
            PumpOutcome::Pushed { .. } => {}
            PumpOutcome::Underflow => break,
        }
    }
    assert!(
        replaced,
        "a saturated output queue must drop its oldest block"
    );
    assert_eq!(adapter.queued_output_blocks(), 1);
}

#[test]
fn source_discontinuity_discards_stale_partial_input() {
    let (mut source, mut adapter, mut output) =
        adapter(format(48_000, 1), format(48_000, 1), 16, 16);
    let first: Vec<f32> = (0..64_u16).map(|index| f32::from(index) * 0.01).collect();
    push_frames(&mut source, &first, 1);
    let produced = pump_until_underflow(&mut adapter, 16);
    let first_out = drain_blocks(&mut output, 16);
    assert_eq!(first_out.len(), produced);
    assert!(first_out.iter().any(|sample| sample.abs() > TOLERANCE));

    // A new stream epoch retires the retained lookahead frames: old-epoch
    // samples must never blend into the new block.
    source.begin_epoch();
    let second: Vec<f32> = (100..164_u16)
        .map(|index| f32::from(index) * 0.01)
        .collect();
    push_frames(&mut source, &second, 1);
    let produced = pump_until_underflow(&mut adapter, 16);
    let second_out = drain_blocks(&mut output, 16);

    assert_eq!(second_out.len(), produced);
    assert_eq!(second_out.len(), second.len() - LOOKAHEAD_FRAMES);
    let expected: Vec<f32> = (100..148_u16)
        .map(|index| f32::from(index) * 0.01)
        .collect();
    assert_samples(&second_out, &expected);
}

#[test]
fn begin_epoch_retires_queued_output_blocks() {
    let (mut source, mut adapter, _) = adapter(format(48_000, 1), format(48_000, 1), 16, 16);
    push_frames(&mut source, &[1.0; 64], 1);
    adapter.pump().unwrap();
    assert_eq!(adapter.queued_output_blocks(), 1);

    adapter.begin_epoch();

    assert_eq!(
        adapter.queued_output_blocks(),
        0,
        "a stream reset must retire queued output blocks"
    );
}

#[test]
fn drift_correction_is_bounded_and_applies() {
    let (mut source, mut adapter, mut output) =
        adapter(format(48_000, 1), format(48_000, 1), 16, 16);
    assert_eq!(
        adapter.set_rate_adjustment_ppm(MAX_DRIFT_ADJUSTMENT_PPM + 1),
        Err(ResamplerError::InvalidAdjustmentPpm)
    );
    adapter
        .set_rate_adjustment_ppm(MAX_DRIFT_ADJUSTMENT_PPM)
        .unwrap();

    let input: Vec<f32> = (0..64_u16).map(|index| f32::from(index) * 0.01).collect();
    push_frames(&mut source, &input, 1);
    let produced = pump_until_underflow(&mut adapter, 16);
    let drained = drain_blocks(&mut output, 16);

    assert!(
        produced > 0,
        "a corrected adapter must still convert blocks"
    );
    assert_eq!(drained.len(), produced);
    assert!(drained.iter().all(|sample| sample.is_finite()));
    // +2000 ppm consumes input 0.2% faster, so the last emitted frame lands
    // at input position 47 * 1.002 instead of 47.
    let expected_last = f32::from(47_u16) * 0.01 * 1.002;
    assert!(
        (drained[drained.len() - 1] - expected_last).abs() <= 1e-3,
        "the drift correction must actually shift the output phase"
    );
}

#[test]
fn queued_source_blocks_reports_pending_conversion_work() {
    let (mut source, mut adapter, _) = adapter(format(48_000, 1), format(48_000, 1), 16, 16);
    push_frames(&mut source, &[1.0; 16], 1);
    assert_eq!(adapter.queued_source_blocks(), 2);

    adapter.pump().unwrap();
    assert_eq!(
        adapter.queued_source_blocks(),
        0,
        "one pump drains the queued source blocks"
    );
}

#[test]
fn construction_rejects_invalid_formats_and_block_bounds() {
    // A bypassed SourcePcmFormat is still rejected by the adapter's own bounds.
    let invalid_rate = SourcePcmFormat {
        sample_rate_hz: 1,
        channels: 1,
    };
    let (_source_producer, source_consumer) = realtime_pcm_queue::<SOURCE_BLOCK>(8).unwrap();
    let (output_producer, _) = realtime_pcm_queue::<OUTPUT_BLOCK>(8).unwrap();
    assert!(matches!(
        StreamFormatAdapter::<SOURCE_BLOCK, OUTPUT_BLOCK>::new(
            source_consumer,
            output_producer,
            invalid_rate,
            format(48_000, 1),
        ),
        Err(StreamFormatAdapterError::Resampler(_))
    ));

    let invalid_channels = SourcePcmFormat {
        sample_rate_hz: 48_000,
        channels: 0,
    };
    let (_source_producer, source_consumer) = realtime_pcm_queue::<SOURCE_BLOCK>(8).unwrap();
    let (output_producer, _) = realtime_pcm_queue::<OUTPUT_BLOCK>(8).unwrap();
    assert!(matches!(
        StreamFormatAdapter::<SOURCE_BLOCK, OUTPUT_BLOCK>::new(
            source_consumer,
            output_producer,
            invalid_channels,
            format(48_000, 1),
        ),
        Err(StreamFormatAdapterError::Resampler(_))
    ));

    // An output block too small for one output frame fails closed.
    let (_source_producer, source_consumer) = realtime_pcm_queue::<SOURCE_BLOCK>(8).unwrap();
    let (output_producer, _) = realtime_pcm_queue::<1>(8).unwrap();
    assert!(matches!(
        StreamFormatAdapter::<SOURCE_BLOCK, 1>::new(
            source_consumer,
            output_producer,
            format(48_000, 1),
            format(48_000, 2),
        ),
        Err(StreamFormatAdapterError::BlockTooSmallForLayout)
    ));
}
