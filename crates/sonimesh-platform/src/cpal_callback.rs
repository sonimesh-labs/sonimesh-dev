use std::sync::Arc;

use cpal::traits::DeviceTrait;
use cpal::{Sample, Stream, StreamConfig};

use crate::RenderSource;
use crate::cpal_status::CallbackStatus;

pub(crate) fn build_float32_stream(
    device: &cpal::Device,
    config: StreamConfig,
    maximum_samples: usize,
    channels: u16,
    mut source: Box<dyn RenderSource>,
    status: &Arc<CallbackStatus>,
) -> Result<Stream, cpal::Error> {
    let callback_status = Arc::clone(status);
    let error_status = Arc::clone(status);
    device.build_output_stream::<f32, _, _>(
        config,
        move |output, _| {
            render_float32(
                source.as_mut(),
                output,
                maximum_samples,
                channels,
                &callback_status,
            );
        },
        move |_| error_status.stream_error(),
        None,
    )
}

pub(crate) fn build_signed16_stream(
    device: &cpal::Device,
    config: StreamConfig,
    maximum_samples: usize,
    channels: u16,
    mut source: Box<dyn RenderSource>,
    status: &Arc<CallbackStatus>,
) -> Result<Stream, cpal::Error> {
    let callback_status = Arc::clone(status);
    let error_status = Arc::clone(status);
    let mut scratch = vec![0.0_f32; maximum_samples];
    device.build_output_stream::<i16, _, _>(
        config,
        move |output, _| {
            render_signed16(
                source.as_mut(),
                output,
                &mut scratch,
                channels,
                &callback_status,
            );
        },
        move |_| error_status.stream_error(),
        None,
    )
}

fn render_float32(
    source: &mut dyn RenderSource,
    output: &mut [f32],
    maximum_samples: usize,
    channels: u16,
    status: &CallbackStatus,
) {
    status.callback(output.len(), frames(output.len(), channels));
    if output.len() > maximum_samples {
        output.fill(0.0);
        status.oversized();
        return;
    }
    output.fill(0.0);
    source.render(output);
    for sample in output {
        *sample = sanitize_float32(*sample);
    }
}

fn render_signed16(
    source: &mut dyn RenderSource,
    output: &mut [i16],
    scratch: &mut [f32],
    channels: u16,
    status: &CallbackStatus,
) {
    status.callback(output.len(), frames(output.len(), channels));
    if output.len() > scratch.len() {
        output.fill(0);
        status.oversized();
        return;
    }

    let float_output = &mut scratch[..output.len()];
    float_output.fill(0.0);
    source.render(float_output);
    for (destination, sample) in output.iter_mut().zip(float_output) {
        *destination = i16::from_sample(sanitize_float32(*sample));
    }
}

fn sanitize_float32(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

fn frames(samples: usize, channels: u16) -> u32 {
    u32::try_from(samples / usize::from(channels)).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ConstantSource(f32);

    impl RenderSource for ConstantSource {
        fn render(&mut self, output: &mut [f32]) {
            output.fill(self.0);
        }
    }

    struct CountingSource {
        calls: usize,
    }

    impl RenderSource for CountingSource {
        fn render(&mut self, output: &mut [f32]) {
            self.calls += 1;
            output.fill(1.0);
        }
    }

    #[test]
    fn oversized_float_callback_fails_closed() {
        let status = CallbackStatus::default();
        let mut source = ConstantSource(1.0);
        let mut output = [f32::NAN; 4];

        render_float32(&mut source, &mut output, 2, 2, &status);

        assert!(output.iter().all(|sample| sample.abs() < f32::EPSILON));
        assert_eq!(status.snapshot().oversized_callbacks, 1);
    }

    #[test]
    fn float_callback_sanitizes_and_bounds_source_samples() {
        struct InvalidSource;

        impl RenderSource for InvalidSource {
            fn render(&mut self, output: &mut [f32]) {
                output.copy_from_slice(&[f32::NAN, f32::INFINITY, -2.0, 2.0]);
            }
        }

        let status = CallbackStatus::default();
        let mut source = InvalidSource;
        let mut output = [f32::NAN; 4];

        render_float32(&mut source, &mut output, 4, 2, &status);

        assert_eq!(
            output.map(f32::to_bits),
            [
                0.0_f32.to_bits(),
                0.0_f32.to_bits(),
                (-1.0_f32).to_bits(),
                1.0_f32.to_bits()
            ]
        );
        assert_eq!(status.snapshot().maximum_callback_samples, 4);
    }

    #[test]
    fn float_callback_keeps_an_underfilled_source_tail_silent() {
        struct PartialSource;

        impl RenderSource for PartialSource {
            fn render(&mut self, output: &mut [f32]) {
                output[..2].copy_from_slice(&[f32::NAN, 2.0]);
            }
        }

        let status = CallbackStatus::default();
        let mut source = PartialSource;
        let mut output = [f32::NAN; 4];

        render_float32(&mut source, &mut output, 4, 2, &status);

        assert_eq!(
            output.map(f32::to_bits),
            [
                0.0_f32.to_bits(),
                1.0_f32.to_bits(),
                0.0_f32.to_bits(),
                0.0_f32.to_bits()
            ]
        );
    }

    #[test]
    fn signed_conversion_sanitizes_non_finite_source() {
        let status = CallbackStatus::default();
        let mut source = ConstantSource(f32::NAN);
        let mut output = [1_i16; 2];
        let mut scratch = [0.0; 2];

        render_signed16(&mut source, &mut output, &mut scratch, 2, &status);

        assert_eq!(output, [0, 0]);
        assert_eq!(status.snapshot().maximum_callback_samples, 2);
    }

    #[test]
    fn oversized_signed_callback_fails_closed_without_rendering_the_source() {
        let status = CallbackStatus::default();
        let mut source = CountingSource { calls: 0 };
        let mut output = [i16::MAX; 3];
        let mut scratch = [0.0; 2];

        render_signed16(&mut source, &mut output, &mut scratch, 2, &status);

        assert_eq!(source.calls, 0);
        assert_eq!(output, [0, 0, 0]);
        let snapshot = status.snapshot();
        assert_eq!(snapshot.callback_count, 1);
        assert_eq!(snapshot.oversized_callbacks, 1);
        assert_eq!(snapshot.maximum_callback_samples, 3);
    }
}
