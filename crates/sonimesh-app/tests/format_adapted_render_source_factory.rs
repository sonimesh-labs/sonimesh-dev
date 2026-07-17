//! Control-domain composition tests for source format adaptation.

use std::sync::Arc;

use sonimesh_app::{FormatAdaptedRenderSourceFactory, SourcePcmFormat, SourcePcmFormatError};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::{
    OutputConfig, RenderSource, RenderSourceFactory, RenderSourceFactoryError, SampleFormat,
};

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

struct RampFactory;

impl RenderSourceFactory for RampFactory {
    fn create(
        &mut self,
        _config: &OutputConfig,
    ) -> Result<Box<dyn RenderSource>, RenderSourceFactoryError> {
        Ok(Box::new(RampSource { next: 0.0 }))
    }
}

fn output_config() -> OutputConfig {
    OutputConfig {
        sample_rate_hz: 96_000,
        channels: 2,
        sample_format: SampleFormat::Float32,
        requested_callback_frames: Some(480),
        maximum_callback_frames: 480,
    }
}

#[test]
fn factory_preserves_drift_control_across_recreated_format_adapters() {
    let counters = Arc::new(StreamCounters::default());
    let format = SourcePcmFormat::new(48_000, 1).unwrap();
    let mut factory =
        FormatAdaptedRenderSourceFactory::new(RampFactory, format, Some(Arc::clone(&counters)));
    let adjustment = factory.adjustment_handle();
    adjustment.set_ppm(1_000).unwrap();

    let mut first = factory.create(&output_config()).unwrap();
    let mut first_output = [f32::NAN; 6];
    first.render(&mut first_output);
    assert_samples(&first_output, &[0.0, 0.0, 0.5005, 0.5005, 1.001, 1.001]);

    let mut recreated = factory.create(&output_config()).unwrap();
    let mut recreated_output = [f32::NAN; 6];
    recreated.render(&mut recreated_output);
    assert_samples(&recreated_output, &[0.0, 0.0, 0.5005, 0.5005, 1.001, 1.001]);

    let snapshot = counters.snapshot();
    assert_eq!(snapshot.nominal_input_sample_rate_hz, 48_000);
    assert_eq!(snapshot.nominal_output_sample_rate_hz, 96_000);
    assert_eq!(snapshot.drift_adjustment_ppm, 1_000);
}

#[test]
fn factory_rejects_invalid_output_config_before_creating_the_source() {
    let format = SourcePcmFormat::new(48_000, 1).unwrap();
    let mut factory = FormatAdaptedRenderSourceFactory::new(RampFactory, format, None);
    let mut invalid = output_config();
    invalid.channels = 0;

    assert!(matches!(
        factory.create(&invalid),
        Err(RenderSourceFactoryError::InvalidConfig(_))
    ));
    assert_eq!(
        SourcePcmFormat::new(7_999, 1),
        Err(SourcePcmFormatError::InvalidSampleRate)
    );
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
