//! Queue-backed realtime render-source tests.

use std::sync::Arc;

use sonimesh_app::{GainControlHandle, PcmRenderSource};
use sonimesh_audio::{MAX_PCM_QUEUE_BLOCKS, realtime_pcm_queue};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::RenderSource;

#[test]
fn renders_across_blocks_without_losing_the_cursor() {
    let (mut producer, consumer) = realtime_pcm_queue::<4>(4).unwrap();
    producer.push(&[1.0, 2.0, 3.0, 4.0]);
    producer.push(&[5.0, 6.0]);
    let counters = Arc::new(StreamCounters::default());
    let mut source =
        PcmRenderSource::new(consumer, Arc::clone(&counters), GainControlHandle::new());

    let mut first = [f32::NAN; 3];
    source.render(&mut first);
    assert_samples(&first, &[1.0, 2.0, 3.0]);

    let mut second = [f32::NAN; 3];
    source.render(&mut second);
    assert_samples(&second, &[4.0, 5.0, 6.0]);
    assert_eq!(counters.snapshot().pcm_underflows, 0);
}

#[test]
fn empty_callback_does_not_consume_pcm_or_record_underflow() {
    let (mut producer, consumer) = realtime_pcm_queue::<4>(4).unwrap();
    producer.push(&[1.0, 2.0]);
    let counters = Arc::new(StreamCounters::default());
    let mut source =
        PcmRenderSource::new(consumer, Arc::clone(&counters), GainControlHandle::new());
    let mut empty = [];

    source.render(&mut empty);

    assert_eq!(producer.len(), 1);
    assert_eq!(counters.snapshot().pcm_underflows, 0);
    let mut output = [f32::NAN; 2];
    source.render(&mut output);
    assert_samples(&output, &[1.0, 2.0]);
}

#[test]
fn underflow_fills_silence_and_records_one_callback_event() {
    let (_producer, consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let counters = Arc::new(StreamCounters::default());
    let mut source =
        PcmRenderSource::new(consumer, Arc::clone(&counters), GainControlHandle::new());
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
    let mut source =
        PcmRenderSource::new(consumer, Arc::clone(&counters), GainControlHandle::new());
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
    let mut source =
        PcmRenderSource::new(consumer, Arc::clone(&counters), GainControlHandle::new());
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
    let mut source =
        PcmRenderSource::new(consumer, Arc::clone(&counters), GainControlHandle::new());
    let mut first = [f32::NAN; 2];
    let mut second = [f32::NAN; 2];

    source.render(&mut first);
    source.render(&mut second);

    assert_samples(&first, &[1.0 / 128.0, 2.0 / 128.0]);
    assert_samples(&second, &[3.0 / 128.0, 4.0 / 128.0]);
    assert_eq!(counters.snapshot().pcm_discontinuities, 1);
}

#[test]
fn published_gain_ramps_across_blocks_then_holds_the_scaled_pcm() {
    let (mut producer, consumer) = realtime_pcm_queue::<4>(48).unwrap();
    for _ in 0..48 {
        producer.push(&[1.0; 4]);
    }
    let counters = Arc::new(StreamCounters::default());
    let handle = GainControlHandle::new();
    let mut source = PcmRenderSource::new(consumer, Arc::clone(&counters), handle.clone());

    let mut unity = [f32::NAN; 4];
    source.render(&mut unity);
    assert_samples(&unity, &[1.0; 4]);

    handle.set_gain(0.5).unwrap();

    // The first post-change block starts the bounded ramp instead of jumping.
    let mut ramping = [f32::NAN; 4];
    source.render(&mut ramping);
    assert!(
        (ramping[0] - (1.0 - 0.5 / 128.0)).abs() < f32::EPSILON,
        "first ramp sample was {ramping:?}"
    );

    // After 128 ramp samples the gain holds exactly 0.5. 32 more blocks of
    // four samples complete the ramp, so the final render must be scaled.
    let mut settled = [f32::NAN; 4];
    for _ in 0..40 {
        source.render(&mut settled);
    }
    assert!(
        settled
            .iter()
            .all(|sample| (*sample - 0.5).abs() < f32::EPSILON),
        "settled samples were {settled:?}"
    );
}

#[test]
fn mute_ramps_to_silence_and_unmute_restores_the_published_gain() {
    // The 128-sample bounded ramp needs 32 blocks of four samples, so keep
    // enough queued PCM for both ramps plus the initial unity block.
    let (mut producer, consumer) = realtime_pcm_queue::<4>(96).unwrap();
    for _ in 0..96 {
        producer.push(&[1.0; 4]);
    }
    let counters = Arc::new(StreamCounters::default());
    let handle = GainControlHandle::new();
    let mut source = PcmRenderSource::new(consumer, Arc::clone(&counters), handle.clone());

    let mut scratch = [f32::NAN; 4];
    source.render(&mut scratch);
    assert_samples(&scratch, &[1.0; 4]);

    handle.set_muted(true);
    // The 128-sample ramp completes only at the end of the 32nd block, so the
    // 33rd block is the first one rendered entirely at silence.
    for _ in 0..33 {
        source.render(&mut scratch);
    }
    assert!(
        scratch.iter().all(|sample| sample.abs() < f32::EPSILON),
        "muted samples were {scratch:?}"
    );

    handle.set_muted(false);
    handle.set_gain(0.25).unwrap();
    for _ in 0..33 {
        source.render(&mut scratch);
    }
    assert!(
        scratch
            .iter()
            .all(|sample| (*sample - 0.25).abs() < f32::EPSILON),
        "unmuted samples were {scratch:?}"
    );
}

#[test]
fn gain_changes_do_not_reset_an_unchanged_target_ramp() {
    let (mut producer, consumer) = realtime_pcm_queue::<4>(64).unwrap();
    for _ in 0..64 {
        producer.push(&[1.0; 4]);
    }
    let counters = Arc::new(StreamCounters::default());
    let handle = GainControlHandle::new();
    let mut source = PcmRenderSource::new(consumer, Arc::clone(&counters), handle.clone());

    let mut scratch = [f32::NAN; 4];
    source.render(&mut scratch);
    handle.set_gain(0.5).unwrap();

    // The ramp starts at the first post-change block instead of jumping.
    let mut first = [f32::NAN; 4];
    source.render(&mut first);
    assert!(
        (first[0] - (1.0 - 0.5 / 128.0)).abs() < f32::EPSILON,
        "first ramp sample was {first:?}"
    );

    // Repeatedly rendering with the same target must advance the ramp rather
    // than resetting it, so the first sample of each block keeps moving
    // toward the lower target.
    let mut previous = first[0];
    let mut converged = false;
    for _ in 0..32 {
        source.render(&mut scratch);
        assert!(
            scratch[0] <= previous,
            "ramp must advance toward the lower target"
        );
        previous = scratch[0];
        if (scratch[0] - 0.5).abs() < f32::EPSILON {
            converged = true;
            break;
        }
    }
    assert!(converged, "ramp should converge within 128 samples");
    assert!(
        scratch
            .iter()
            .all(|sample| (*sample - 0.5).abs() < f32::EPSILON),
        "settled samples were {scratch:?}"
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
