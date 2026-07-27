//! Queue-backed realtime render-source tests.

use std::sync::Arc;

use sonimesh_app::PcmRenderSource;
use sonimesh_audio::{MAX_PCM_QUEUE_BLOCKS, realtime_pcm_queue};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::RenderSource;

#[test]
fn renders_across_blocks_without_losing_the_cursor() {
    let (mut producer, consumer) = realtime_pcm_queue::<4>(4).unwrap();
    producer.push(&[1.0, 2.0, 3.0, 4.0]);
    producer.push(&[5.0, 6.0]);
    let counters = Arc::new(StreamCounters::default());
    let mut source = PcmRenderSource::new(consumer, Arc::clone(&counters));

    let mut first = [f32::NAN; 3];
    source.render(&mut first);
    assert_samples(&first, &[1.0, 2.0, 3.0]);

    let mut second = [f32::NAN; 3];
    source.render(&mut second);
    assert_samples(&second, &[4.0, 5.0, 6.0]);
    assert_eq!(counters.snapshot().pcm_underflows, 0);
}

#[test]
fn underflow_fills_silence_and_records_one_callback_event() {
    let (_producer, consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let counters = Arc::new(StreamCounters::default());
    let mut source = PcmRenderSource::new(consumer, Arc::clone(&counters));
    let mut output = [f32::NAN; 4];

    source.render(&mut output);

    assert!(output.iter().all(|sample| sample.abs() < f32::EPSILON));
    assert_eq!(counters.snapshot().pcm_underflows, 1);
}

#[test]
fn empty_blocks_cannot_make_the_callback_loop_unbounded() {
    let (mut producer, consumer) = realtime_pcm_queue::<4>(4).unwrap();
    producer.push(&[]);
    let counters = Arc::new(StreamCounters::default());
    let mut source = PcmRenderSource::new(consumer, Arc::clone(&counters));
    let mut output = [f32::NAN; 2];

    source.render(&mut output);

    assert!(output.iter().all(|sample| sample.abs() < f32::EPSILON));
    assert_eq!(counters.snapshot().pcm_underflows, 1);
}

#[test]
fn refill_limit_emits_silence_and_records_a_distinct_callback_event() {
    let (mut producer, consumer) = realtime_pcm_queue::<1>(MAX_PCM_QUEUE_BLOCKS).unwrap();
    for _ in 0..MAX_PCM_QUEUE_BLOCKS {
        producer.push(&[1.0]);
    }
    let counters = Arc::new(StreamCounters::default());
    let mut source = PcmRenderSource::new(consumer, Arc::clone(&counters));
    let mut output = [f32::NAN; MAX_PCM_QUEUE_BLOCKS + 1];

    source.render(&mut output);

    assert!(
        output[..MAX_PCM_QUEUE_BLOCKS]
            .iter()
            .all(|sample| (*sample - 1.0).abs() < f32::EPSILON)
    );
    assert!(output[MAX_PCM_QUEUE_BLOCKS].abs() < f32::EPSILON);
    let snapshot = counters.snapshot();
    assert_eq!(snapshot.pcm_refill_limit_hits, 1);
    assert_eq!(snapshot.pcm_underflows, 0);
}

#[test]
fn sequence_gap_fades_pcm_back_in_and_records_the_discontinuity() {
    let (mut producer, consumer) = realtime_pcm_queue::<4>(1).unwrap();
    producer.push(&[1.0; 4]);
    producer.push(&[1.0; 4]);
    let counters = Arc::new(StreamCounters::default());
    let mut source = PcmRenderSource::new(consumer, Arc::clone(&counters));
    let mut first = [f32::NAN; 2];
    let mut second = [f32::NAN; 2];

    source.render(&mut first);
    source.render(&mut second);

    assert_samples(&first, &[1.0 / 128.0, 2.0 / 128.0]);
    assert_samples(&second, &[3.0 / 128.0, 4.0 / 128.0]);
    assert_eq!(counters.snapshot().pcm_discontinuities, 1);
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
