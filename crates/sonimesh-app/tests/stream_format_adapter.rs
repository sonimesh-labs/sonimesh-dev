//! Decode-domain format adapter tests: source-to-output rate and channel
//! conversion feeding a mixer slot queue.
//!
//! ADR 0015 leaves per-stream sample-rate and channel adaptation before the
//! mixer queue as open work. These tests drive the real `StreamFormatAdapter`
//! pump against bounded queues and verify passthrough, interpolation,
//! channel mapping, queue overflow, discontinuity handling, and drift-bound
//! behavior, all outside the output callback.

use sonimesh_app::{PumpOutcome, SourcePcmFormat, StreamFormatAdapter, StreamFormatAdapterError};
use sonimesh_audio::{
    MAX_DRIFT_ADJUSTMENT_PPM, RealtimePcmConsumer, RealtimePcmPopOutcome, RealtimePcmProducer,
    RealtimePcmPushOutcome, ResamplerError, realtime_pcm_queue,
};

const SOURCE_BLOCK: usize = 8;
const OUTPUT_BLOCK: usize = 8;
const TOLERANCE: f32 = 1e-6;

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
    let (mut source, mut adapter, mut output) = adapter(format(48_000, 1), format(48_000, 1), 8, 8);
    let input: Vec<f32> = (0..32_u16).map(f32::from).collect();
    push_frames(&mut source, &input, 1);

    let mut produced = 0;
    for _ in 0..5 {
        match adapter.pump().unwrap() {
            PumpOutcome::Pushed { frames, .. } => produced += frames,
            PumpOutcome::Underflow => break,
        }
    }
    let drained = drain_blocks(&mut output, 5);

    // One trailing input frame stays retained for the next interpolation
    // step, so passthrough emits the input prefix in order.
    assert_eq!(produced, 31);
    assert_eq!(drained.len(), produced);
    for (actual, expected) in drained.iter().zip(input.iter()) {
        assert!((*actual - *expected).abs() <= TOLERANCE);
    }
}

#[test]
fn upsampling_interpolates_between_source_frames() {
    let (mut source, mut adapter, mut output) = adapter(format(48_000, 1), format(96_000, 1), 8, 8);
    let input: Vec<f32> = (0..8_u16).map(f32::from).collect();
    push_frames(&mut source, &input, 1);

    let outcome = adapter.pump().unwrap();
    assert_eq!(
        outcome,
        PumpOutcome::Pushed {
            frames: 8,
            queue: RealtimePcmPushOutcome::Inserted,
        }
    );
    let drained = drain_blocks(&mut output, 1);
    assert_samples(&drained, &[0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5]);
}

#[test]
fn downsampling_selects_output_rate_frames() {
    let (mut source, mut adapter, mut output) = adapter(format(96_000, 1), format(48_000, 1), 8, 8);
    let input: Vec<f32> = (0..16_u16).map(f32::from).collect();
    push_frames(&mut source, &input, 1);

    let outcome = adapter.pump().unwrap();
    assert_eq!(
        outcome,
        PumpOutcome::Pushed {
            frames: 8,
            queue: RealtimePcmPushOutcome::Inserted,
        }
    );
    let drained = drain_blocks(&mut output, 1);
    assert_samples(&drained, &[0.0, 2.0, 4.0, 6.0, 8.0, 10.0, 12.0, 14.0]);
}

#[test]
fn mono_source_is_duplicated_to_stereo_output() {
    let (mut source, mut adapter, mut output) = adapter(format(48_000, 1), format(48_000, 2), 8, 8);
    let input: Vec<f32> = (0..8_u16).map(f32::from).collect();
    push_frames(&mut source, &input, 1);

    let outcome = adapter.pump().unwrap();
    // An eight-sample output block holds four stereo frames per push.
    assert_eq!(
        outcome,
        PumpOutcome::Pushed {
            frames: 4,
            queue: RealtimePcmPushOutcome::Inserted,
        }
    );
    let drained = drain_blocks(&mut output, 1);
    let mut expected = Vec::new();
    for value in 0..4_u16 {
        expected.push(f32::from(value));
        expected.push(f32::from(value));
    }
    assert_samples(&drained, &expected);
}

#[test]
fn stereo_source_is_folded_to_mono_output() {
    let (mut source, mut adapter, mut output) = adapter(format(48_000, 2), format(48_000, 1), 8, 8);
    let mut input = Vec::new();
    for frame in 0..8_u16 {
        input.push(f32::from(frame));
        input.push(f32::from(frame + 100));
    }
    push_frames(&mut source, &input, 2);

    let outcome = adapter.pump().unwrap();
    assert_eq!(
        outcome,
        PumpOutcome::Pushed {
            frames: 7,
            queue: RealtimePcmPushOutcome::Inserted,
        }
    );
    let drained = drain_blocks(&mut output, 1);
    let expected: Vec<f32> = (0..7_u16).map(|frame| f32::from(frame) + 50.0).collect();
    assert_samples(&drained, &expected);
}

#[test]
fn rate_and_channel_adaptation_compose() {
    let (mut source, mut adapter, mut output) = adapter(format(96_000, 2), format(48_000, 1), 8, 8);
    let mut input = Vec::new();
    for frame in 0..16_u16 {
        input.push(f32::from(frame));
        input.push(f32::from(frame + 100));
    }
    push_frames(&mut source, &input, 2);

    let outcome = adapter.pump().unwrap();
    assert_eq!(
        outcome,
        PumpOutcome::Pushed {
            frames: 8,
            queue: RealtimePcmPushOutcome::Inserted,
        }
    );
    let drained = drain_blocks(&mut output, 1);
    // Downsampling selects input frames 0,2,...,14; folding averages each pair.
    let expected: Vec<f32> = (0..8_u16).map(|index| f32::from(index * 2) + 50.0).collect();
    assert_samples(&drained, &expected);
}

#[test]
fn empty_source_underflows_without_pushing() {
    let (_, mut adapter, mut output) = adapter(format(48_000, 1), format(48_000, 1), 8, 8);

    assert_eq!(adapter.pump().unwrap(), PumpOutcome::Underflow);
    assert_eq!(adapter.queued_output_blocks(), 0);
    assert!(drain_blocks(&mut output, 1).is_empty());
}

#[test]
fn partial_source_frame_is_retained_across_pumps() {
    let (mut source, mut adapter, mut output) = adapter(format(48_000, 1), format(48_000, 1), 8, 8);
    assert_eq!(source.push(&[1.0]), RealtimePcmPushOutcome::Inserted);
    assert_eq!(adapter.pump().unwrap(), PumpOutcome::Underflow);
    assert_eq!(adapter.queued_output_blocks(), 0);

    let tail: Vec<f32> = (2..=8_u16).map(f32::from).collect();
    assert_eq!(source.push(&tail), RealtimePcmPushOutcome::Inserted);
    let outcome = adapter.pump().unwrap();
    assert_eq!(
        outcome,
        PumpOutcome::Pushed {
            frames: 7,
            queue: RealtimePcmPushOutcome::Inserted,
        }
    );
    let drained = drain_blocks(&mut output, 1);
    assert_samples(&drained, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
}

#[test]
fn full_output_queue_replaces_the_oldest_block() {
    let (mut source, mut adapter, _) = adapter(format(48_000, 1), format(48_000, 1), 8, 1);
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
    let (mut source, mut adapter, mut output) = adapter(format(48_000, 1), format(48_000, 1), 8, 8);
    let first: Vec<f32> = (0..8_u16).map(f32::from).collect();
    push_frames(&mut source, &first, 1);
    adapter.pump().unwrap();

    let first_out = drain_blocks(&mut output, 1);
    assert_samples(&first_out, &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);

    // A new stream epoch retires the retained interpolation frame: frame 7 of
    // the old epoch must never blend into the new block.
    source.begin_epoch();
    let second: Vec<f32> = (100..108_u16).map(f32::from).collect();
    push_frames(&mut source, &second, 1);
    let outcome = adapter.pump().unwrap();
    assert_eq!(
        outcome,
        PumpOutcome::Pushed {
            frames: 7,
            queue: RealtimePcmPushOutcome::Inserted,
        }
    );
    let second_out = drain_blocks(&mut output, 1);
    let expected: Vec<f32> = (100..107_u16).map(f32::from).collect();
    assert_samples(&second_out, &expected);
}

#[test]
fn begin_epoch_retires_queued_output_blocks() {
    let (mut source, mut adapter, _) = adapter(format(48_000, 1), format(48_000, 1), 8, 8);
    push_frames(&mut source, &[1.0; 8], 1);
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
    let (mut source, mut adapter, mut output) = adapter(format(48_000, 1), format(48_000, 1), 8, 8);
    assert_eq!(
        adapter.set_rate_adjustment_ppm(MAX_DRIFT_ADJUSTMENT_PPM + 1),
        Err(ResamplerError::InvalidAdjustmentPpm)
    );
    adapter
        .set_rate_adjustment_ppm(MAX_DRIFT_ADJUSTMENT_PPM)
        .unwrap();

    let input: Vec<f32> = (0..16_u16).map(f32::from).collect();
    push_frames(&mut source, &input, 1);
    let frames = match adapter.pump().unwrap() {
        PumpOutcome::Pushed { frames, .. } => frames,
        PumpOutcome::Underflow => 0,
    };
    assert!(frames > 0, "a corrected adapter must still convert blocks");
    let drained = drain_blocks(&mut output, 1);
    assert_eq!(drained.len(), frames);
    assert!(drained.iter().all(|sample| sample.is_finite()));
}

#[test]
fn queued_source_blocks_reports_pending_conversion_work() {
    let (mut source, mut adapter, _) = adapter(format(48_000, 1), format(48_000, 1), 8, 8);
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
