//! Realtime-safe channel mapping tests using the bounded PCM source.

use std::sync::Arc;

use sonimesh_app::{
    ChannelMappedRenderSource, ChannelMappedRenderSourceError, GainControlHandle, PcmRenderSource,
};
use sonimesh_audio::{ChannelMappingError, realtime_pcm_queue};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::{MAX_CALLBACK_FRAMES, RenderSource};

fn mapped_source(
    samples: &[f32],
    input_channels: u16,
    output_channels: u16,
) -> ChannelMappedRenderSource {
    let (mut producer, consumer) = realtime_pcm_queue::<8>(4).unwrap();
    producer.push(samples);
    let source = PcmRenderSource::new(
        consumer,
        Arc::new(StreamCounters::default()),
        GainControlHandle::new(),
    );
    ChannelMappedRenderSource::new(
        Box::new(source),
        input_channels,
        output_channels,
        MAX_CALLBACK_FRAMES,
    )
    .unwrap()
}

#[test]
fn maps_mono_pcm_to_stereo_without_allocating_in_render() {
    let mut source = mapped_source(&[1.0, -0.5], 1, 2);
    let mut output = [f32::NAN; 4];

    source.render(&mut output);

    assert_samples(&output, &[1.0, 1.0, -0.5, -0.5]);
}

#[test]
fn zero_extends_stereo_pcm_for_a_multichannel_output() {
    let mut source = mapped_source(&[1.0, 2.0, 3.0, 4.0], 2, 4);
    let mut output = [f32::NAN; 8];

    source.render(&mut output);

    assert_samples(&output, &[1.0, 2.0, 0.0, 0.0, 3.0, 4.0, 0.0, 0.0]);
}

#[test]
fn callback_tail_is_silence_when_output_is_not_frame_aligned() {
    let mut source = mapped_source(&[1.0, 2.0], 1, 2);
    let mut output = [f32::NAN; 3];

    source.render(&mut output);

    assert_samples(&output, &[1.0, 1.0, 0.0]);
}

#[test]
fn oversized_callback_is_silence_without_reading_the_wrapped_source() {
    let mut source = mapped_source(&[1.0, 2.0], 1, 2);
    let mut output = vec![f32::NAN; (MAX_CALLBACK_FRAMES as usize + 1) * 2];

    source.render(&mut output);

    assert!(output.iter().all(|sample| sample.abs() < f32::EPSILON));
}

#[test]
fn construction_rejects_invalid_layout_and_callback_bounds() {
    let source = Box::new(EmptySource) as Box<dyn RenderSource>;
    assert!(matches!(
        ChannelMappedRenderSource::new(source, 0, 2, 480),
        Err(ChannelMappedRenderSourceError::Mapping(
            ChannelMappingError::InvalidInputChannelCount
        ))
    ));

    let source = Box::new(EmptySource) as Box<dyn RenderSource>;
    assert!(matches!(
        ChannelMappedRenderSource::new(source, 2, 2, MAX_CALLBACK_FRAMES + 1),
        Err(ChannelMappedRenderSourceError::InvalidMaximumCallbackFrames)
    ));
}

struct EmptySource;

impl RenderSource for EmptySource {
    fn render(&mut self, output: &mut [f32]) {
        output.fill(0.0);
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
