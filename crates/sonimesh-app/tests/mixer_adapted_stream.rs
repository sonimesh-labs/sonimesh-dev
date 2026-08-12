//! Phase 2 mixer wiring tests for per-stream format adaptation before the
//! mixer queue.
//!
//! ADR 0015 requires each decoded stream to arrive in the mixer's output
//! format. `MixerMembershipController::admit_adapted_stream` installs a
//! `StreamFormatAdapter` between the stream's decode-domain source queue and
//! its mixer slot queue; these tests drive that path through the real
//! `MixerRenderSource` and verify converted audio reaches the output, removal
//! silence, fail-closed admission, and per-slot underflow isolation.

use std::sync::Arc;

use sonimesh_app::{
    AdmittedAdaptedStream, GainControlHandle, MixerAdmissionError, MixerMembershipController,
    MixerRenderSource, PumpOutcome, SourcePcmFormat,
};
use sonimesh_audio::{MAX_PCM_QUEUE_BLOCKS, RealtimePcmProducer, RealtimePcmPushOutcome};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::{OutputConfig, RenderSource, SampleFormat};

const SLOTS: usize = 4;
const SOURCE_BLOCK: usize = 8;
const OUTPUT_BLOCK: usize = 8;
const TOLERANCE: f32 = 1e-6;

fn output_config() -> OutputConfig {
    OutputConfig {
        sample_rate_hz: 48_000,
        channels: 1,
        sample_format: SampleFormat::Float32,
        requested_callback_frames: Some(480),
        maximum_callback_frames: 480,
    }
}

fn output_format() -> SourcePcmFormat {
    SourcePcmFormat::new(48_000, 1).unwrap()
}

fn source_format() -> SourcePcmFormat {
    SourcePcmFormat::new(96_000, 2).unwrap()
}

fn controller(
    source: &MixerRenderSource<SLOTS, OUTPUT_BLOCK>,
) -> MixerMembershipController<SLOTS, OUTPUT_BLOCK> {
    MixerMembershipController::new(source.membership(), 16, output_format()).unwrap()
}

fn admit_adapted(
    membership: &mut MixerMembershipController<SLOTS, OUTPUT_BLOCK>,
) -> AdmittedAdaptedStream<SOURCE_BLOCK, OUTPUT_BLOCK> {
    membership
        .admit_adapted_stream::<SOURCE_BLOCK>(source_format(), 16, GainControlHandle::new(), None)
        .unwrap()
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

/// Feeds `blocks` whole blocks of constant `value` through a slot producer.
fn push_constant(producer: &mut RealtimePcmProducer<OUTPUT_BLOCK>, value: f32, blocks: usize) {
    for _ in 0..blocks {
        assert_eq!(
            producer.push(&[value; OUTPUT_BLOCK]),
            RealtimePcmPushOutcome::Inserted
        );
    }
}

/// Renders `blocks` blocks through the mixer source and returns the samples.
fn render(source: &mut MixerRenderSource<SLOTS, OUTPUT_BLOCK>, blocks: usize) -> Vec<f32> {
    let mut output = vec![0.0; blocks * OUTPUT_BLOCK];
    source.render(&mut output);
    output
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
fn adapted_stream_renders_rate_and_channel_converted_audio() {
    let mut source = MixerRenderSource::new(&output_config()).unwrap();
    let mut membership = controller(&source);
    let mut admitted = admit_adapted(&mut membership);
    assert_eq!(admitted.slot, 0);
    assert_eq!(admitted.adapter.source_channels(), 2);
    assert_eq!(admitted.adapter.output_channels(), 1);

    // 96 kHz stereo: frame i carries (0.01*i, 0.1 + 0.01*i). The adapter
    // downsamples to 48 kHz (selecting frames 0,2,...,14) and folds stereo to
    // mono by averaging each pair: 0.05 + 0.02*index.
    let mut input = Vec::new();
    for frame in 0..16_u16 {
        input.push(f32::from(frame) * 0.01);
        input.push(0.1 + f32::from(frame) * 0.01);
    }
    push_frames(&mut admitted.source_producer, &input, 2);
    admitted.adapter.pump().unwrap();

    let output = render(&mut source, 1);
    let expected: Vec<f32> = (0..8_u16)
        .map(|index| 0.05 + f32::from(index) * 0.02)
        .collect();
    assert_samples(&output, &expected);
}

#[test]
fn removing_an_adapted_stream_renders_silence() {
    let mut source = MixerRenderSource::new(&output_config()).unwrap();
    let mut membership = controller(&source);
    let mut admitted = admit_adapted(&mut membership);
    let mut input = Vec::new();
    for frame in 0..16_u16 {
        input.push(f32::from(frame) * 0.01);
        input.push(0.1 + f32::from(frame) * 0.01);
    }
    push_frames(&mut admitted.source_producer, &input, 2);
    admitted.adapter.pump().unwrap();
    assert!(
        render(&mut source, 1)
            .iter()
            .any(|sample| sample.abs() > TOLERANCE),
        "converted audio must reach the output before removal"
    );

    membership.remove_stream(admitted.slot).unwrap();
    assert!(
        render(&mut source, 1)
            .iter()
            .all(|sample| sample.abs() <= TOLERANCE),
        "removed adapted slot must render silence"
    );
    assert!(!membership.is_active(admitted.slot));
}

#[test]
fn adapted_admission_fails_closed_on_invalid_queue_capacity() {
    let source = MixerRenderSource::new(&output_config()).unwrap();
    let mut membership = controller(&source);

    assert!(matches!(
        membership.admit_adapted_stream::<SOURCE_BLOCK>(
            source_format(),
            0,
            GainControlHandle::new(),
            None,
        ),
        Err(MixerAdmissionError::Queue(_))
    ));
    assert!(matches!(
        membership.admit_adapted_stream::<SOURCE_BLOCK>(
            source_format(),
            MAX_PCM_QUEUE_BLOCKS + 1,
            GainControlHandle::new(),
            None,
        ),
        Err(MixerAdmissionError::Queue(_))
    ));
    assert_eq!(
        membership.active(),
        0,
        "failed admission must not reserve a slot"
    );
}

#[test]
fn adapted_admission_fails_closed_on_invalid_source_format() {
    let source = MixerRenderSource::new(&output_config()).unwrap();
    let mut membership = controller(&source);
    let invalid = SourcePcmFormat {
        sample_rate_hz: 48_000,
        channels: 0,
    };

    assert!(matches!(
        membership.admit_adapted_stream::<SOURCE_BLOCK>(
            invalid,
            16,
            GainControlHandle::new(),
            None,
        ),
        Err(MixerAdmissionError::Adapter(_))
    ));
    assert_eq!(
        membership.active(),
        0,
        "failed adaptation must not reserve a slot"
    );
}

#[test]
fn adapted_stream_underflow_leaves_other_slots_rendering() {
    let mut source = MixerRenderSource::new(&output_config()).unwrap();
    let mut membership = controller(&source);
    let counters_a = Arc::new(StreamCounters::default());
    let mut admitted_a = membership
        .admit_adapted_stream::<SOURCE_BLOCK>(
            source_format(),
            16,
            GainControlHandle::new(),
            Some(Arc::clone(&counters_a)),
        )
        .unwrap();
    assert_eq!(admitted_a.slot, 0);
    assert_eq!(
        admitted_a.adapter.pump().unwrap(),
        PumpOutcome::Underflow,
        "an unfed source must underflow at the adapter"
    );

    let mut admitted_b = membership
        .admit_stream(output_format(), GainControlHandle::new(), None)
        .unwrap();
    assert_eq!(admitted_b.slot, 1);
    push_constant(&mut admitted_b.producer, 0.5, 2);

    let output = render(&mut source, 2);
    assert!(
        output
            .iter()
            .all(|sample| (*sample - 0.5).abs() <= TOLERANCE),
        "the starved adapted slot must contribute silence"
    );
    let snapshot = counters_a.snapshot();
    assert!(
        snapshot.pcm_underflows >= 1,
        "the starved adapted slot must record underflow"
    );
}

#[test]
fn adapted_stream_slot_is_reused_after_removal() {
    let mut source = MixerRenderSource::new(&output_config()).unwrap();
    let mut membership = controller(&source);
    let first = admit_adapted(&mut membership);
    assert_eq!(first.slot, 0);

    membership.remove_stream(first.slot).unwrap();
    render(&mut source, 1);

    let second = admit_adapted(&mut membership);
    assert_eq!(
        second.slot, 0,
        "a freed adapted slot must be reused by the next admission"
    );
}
