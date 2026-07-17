//! Receiver PCM endpoint creation and source-format composition tests.

use std::sync::Arc;

use sonimesh_app::{ReceiverPcmEndpointError, ReceiverPcmEndpointFactory, SourcePcmFormat};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::{OutputConfig, SampleFormat};

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
fn endpoint_factory_pairs_a_new_producer_with_a_format_adapted_source() {
    let counters = Arc::new(StreamCounters::default());
    let factory = ReceiverPcmEndpointFactory::<4>::new(
        4,
        SourcePcmFormat::new(48_000, 1).unwrap(),
        Arc::clone(&counters),
    );
    let endpoint = factory.create(&output_config()).unwrap();
    let (mut producer, mut source) = endpoint.into_parts();
    producer.push(&[0.0, 1.0, 2.0, 3.0]);
    let mut output = [f32::NAN; 6];

    source.render(&mut output);

    assert_samples(&output, &[0.0, 0.0, 0.5, 0.5, 1.0, 1.0]);
    let snapshot = counters.snapshot();
    assert_eq!(snapshot.nominal_input_sample_rate_hz, 48_000);
    assert_eq!(snapshot.nominal_output_sample_rate_hz, 96_000);
}

#[test]
fn endpoint_factory_rejects_invalid_queue_capacity_before_output_start() {
    let factory = ReceiverPcmEndpointFactory::<4>::new(
        0,
        SourcePcmFormat::new(48_000, 1).unwrap(),
        Arc::new(StreamCounters::default()),
    );

    assert!(matches!(
        factory.create(&output_config()),
        Err(ReceiverPcmEndpointError::Queue(_))
    ));
}

#[test]
fn endpoint_factory_validates_output_before_creating_the_queue() {
    let factory = ReceiverPcmEndpointFactory::<4>::new(
        0,
        SourcePcmFormat::new(48_000, 1).unwrap(),
        Arc::new(StreamCounters::default()),
    );
    let invalid_output = OutputConfig {
        channels: 0,
        ..output_config()
    };

    assert!(matches!(
        factory.create(&invalid_output),
        Err(ReceiverPcmEndpointError::Source(
            sonimesh_platform::RenderSourceFactoryError::InvalidConfig(_)
        ))
    ));
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
