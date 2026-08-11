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

#[test]
fn gain_and_mute_choices_survive_output_recreation() {
    let counters = Arc::new(StreamCounters::default());
    let factory = ReceiverPcmEndpointFactory::<4>::new(
        128,
        SourcePcmFormat::new(48_000, 1).unwrap(),
        Arc::clone(&counters),
    );

    // First output lifetime: publish a bounded gain through the shared handle.
    let first = factory.create(&output_config()).unwrap();
    first.gain_handle().set_gain(0.5).unwrap();
    let (mut producer, mut source) = first.into_parts();
    let mut callback = [f32::NAN; 480 * 2];
    for _ in 0..128 {
        producer.push(&[1.0; 4]);
    }
    // The 128-sample ramp completes inside the first 480-frame callback; the
    // second callback must be fully scaled by the published gain.
    source.render(&mut callback);
    source.render(&mut callback);
    assert!(
        callback
            .iter()
            .all(|sample| (*sample - 0.5).abs() < f32::EPSILON),
        "first lifetime callback was not fully scaled: {callback:?}"
    );

    // A recreated endpoint keeps the same published gain without replay.
    let second = factory.create(&output_config()).unwrap();
    let (mut producer, mut source) = second.into_parts();
    for _ in 0..128 {
        producer.push(&[1.0; 4]);
    }
    source.render(&mut callback);
    source.render(&mut callback);
    assert!(
        callback
            .iter()
            .all(|sample| (*sample - 0.5).abs() < f32::EPSILON),
        "recreated lifetime lost the published gain: {callback:?}"
    );

    // Mute also survives recreation: the recreated source ramps to silence.
    factory.gain_handle().set_muted(true);
    for _ in 0..128 {
        producer.push(&[1.0; 4]);
    }
    source.render(&mut callback);
    source.render(&mut callback);
    assert!(
        callback.iter().all(|sample| sample.abs() < f32::EPSILON),
        "recreated lifetime lost the muted state: {callback:?}"
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
