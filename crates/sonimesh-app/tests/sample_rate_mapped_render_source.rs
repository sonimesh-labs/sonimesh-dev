//! Nominal sample-rate conversion tests over the bounded PCM source.

use std::sync::Arc;

use sonimesh_app::{
    DriftAdjustmentError, PcmRenderSource, SampleRateMappedRenderSource,
    SampleRateMappedRenderSourceError,
};
use sonimesh_audio::{
    DriftDecision, DriftResyncDirection, MAX_DRIFT_ADJUSTMENT_PPM, ResamplerError,
    realtime_pcm_queue,
};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::{MAX_CALLBACK_FRAMES, RenderSource};

fn resampled_source(
    samples: &[f32],
    input_sample_rate_hz: u32,
    output_sample_rate_hz: u32,
) -> SampleRateMappedRenderSource {
    let (mut producer, consumer) = realtime_pcm_queue::<32>(4).unwrap();
    producer.push(samples);
    let source = PcmRenderSource::new(consumer, Arc::new(StreamCounters::default()));
    SampleRateMappedRenderSource::new(
        Box::new(source),
        input_sample_rate_hz,
        output_sample_rate_hz,
        1,
        MAX_CALLBACK_FRAMES,
    )
    .unwrap()
}

#[test]
fn equal_rates_preserve_the_source_pull_sequence() {
    let mut source = resampled_source(&[0.0, 1.0, 2.0, 3.0, 4.0], 48_000, 48_000);
    let mut output = [f32::NAN; 4];

    source.render(&mut output);

    assert_samples(&output, &[0.0, 1.0, 2.0, 3.0]);
}

#[test]
fn upsampling_interpolates_without_reordering_frames() {
    let mut source = resampled_source(&[0.0, 1.0, 2.0, 3.0], 48_000, 96_000);
    let mut output = [f32::NAN; 6];

    source.render(&mut output);

    assert_samples(&output, &[0.0, 0.5, 1.0, 1.5, 2.0, 2.5]);
}

#[test]
fn downsampling_keeps_nominal_sample_positions() {
    let mut source = resampled_source(&[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0], 96_000, 48_000);
    let mut output = [f32::NAN; 4];

    source.render(&mut output);

    assert_samples(&output, &[0.0, 2.0, 4.0, 6.0]);
}

#[test]
fn long_callback_preserves_phase_across_internal_chunks() {
    let mut source = SampleRateMappedRenderSource::new(
        Box::new(RampSource { next: 0.0 }),
        48_000,
        48_000,
        1,
        MAX_CALLBACK_FRAMES,
    )
    .unwrap();
    let mut output = vec![f32::NAN; 600];

    source.render(&mut output);

    for (index, sample) in output.iter().enumerate() {
        let expected = f32::from(u16::try_from(index).unwrap());
        assert!((*sample - expected).abs() < f32::EPSILON);
    }
}

#[test]
fn drift_decisions_reach_the_resampler_without_moving_controller_state_into_callback() {
    let (mut source, handle) = SampleRateMappedRenderSource::new_with_drift_control(
        Box::new(RampSource { next: 0.0 }),
        48_000,
        48_000,
        1,
        MAX_CALLBACK_FRAMES,
    )
    .unwrap();
    handle
        .apply_decision(DriftDecision::Adjust { ppm: 1_000 })
        .unwrap();
    let mut output = [f32::NAN; 3];

    source.render(&mut output);

    assert_samples(&output, &[0.0, 1.001, 2.002]);
    assert_eq!(
        handle.apply_decision(DriftDecision::HardResync {
            direction: DriftResyncDirection::High,
        }),
        Err(DriftAdjustmentError::HardResyncRequired(
            DriftResyncDirection::High
        ))
    );
    assert_eq!(
        handle.set_ppm(MAX_DRIFT_ADJUSTMENT_PPM + 1),
        Err(DriftAdjustmentError::InvalidPpm)
    );
}

#[test]
fn output_tail_and_oversized_callbacks_are_silence() {
    let mut source = resampled_source(&[1.0, 2.0], 48_000, 48_000);
    let mut tail = [f32::NAN; 2];
    source.render(&mut tail);
    assert_samples(&tail, &[1.0, 2.0]);

    let mut oversized = vec![f32::NAN; MAX_CALLBACK_FRAMES as usize + 1];
    source.render(&mut oversized);
    assert!(oversized.iter().all(|sample| sample.abs() < f32::EPSILON));
}

#[test]
fn construction_rejects_invalid_rate_and_callback_bounds() {
    let source = Box::new(EmptySource) as Box<dyn RenderSource>;
    assert!(matches!(
        SampleRateMappedRenderSource::new(source, 7_999, 48_000, 1, 480),
        Err(SampleRateMappedRenderSourceError::Resampler(
            ResamplerError::InvalidInputSampleRate
        ))
    ));

    let source = Box::new(EmptySource) as Box<dyn RenderSource>;
    assert!(matches!(
        SampleRateMappedRenderSource::new(source, 48_000, 48_000, 1, MAX_CALLBACK_FRAMES + 1),
        Err(SampleRateMappedRenderSourceError::InvalidMaximumCallbackFrames)
    ));
}

struct EmptySource;

impl RenderSource for EmptySource {
    fn render(&mut self, output: &mut [f32]) {
        output.fill(0.0);
    }
}

struct RampSource {
    next: f32,
}

impl RenderSource for RampSource {
    fn render(&mut self, output: &mut [f32]) {
        for sample in output {
            *sample = self.next;
            self.next += 1.0;
        }
    }
}

fn assert_samples(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    assert!(
        actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| (*actual - *expected).abs() < f32::EPSILON)
    );
}
