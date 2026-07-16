//! Cross-thread behavioral tests for the realtime PCM queue boundary.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use sonimesh_audio::{
    MAX_PCM_QUEUE_BLOCKS, RealtimePcmPopOutcome, RealtimePcmPushOutcome, RealtimeQueueConfigError,
    realtime_pcm_queue,
};

#[test]
fn rejects_invalid_capacities() {
    assert!(matches!(
        realtime_pcm_queue::<4>(0),
        Err(RealtimeQueueConfigError::ZeroCapacity)
    ));
    assert!(matches!(
        realtime_pcm_queue::<4>(MAX_PCM_QUEUE_BLOCKS + 1),
        Err(RealtimeQueueConfigError::CapacityTooLarge)
    ));
    assert!(matches!(
        realtime_pcm_queue::<0>(1),
        Err(RealtimeQueueConfigError::ZeroSamplesPerBlock)
    ));
}

#[test]
fn full_queue_replaces_oldest_and_marks_gap() {
    let (mut producer, mut consumer) = realtime_pcm_queue::<2>(2).unwrap();
    assert_eq!(producer.push(&[0.0]), RealtimePcmPushOutcome::Inserted);
    assert_eq!(producer.push(&[1.0]), RealtimePcmPushOutcome::Inserted);
    assert_eq!(
        producer.push(&[2.0]),
        RealtimePcmPushOutcome::ReplacedOldest
    );
    let mut output = [0.0; 2];

    assert_eq!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled {
            samples: 1,
            sequence: 1,
            discontinuity: true,
            discarded_stale_blocks: 0,
        }
    );
    assert!((output[0] - 1.0).abs() < f32::EPSILON);
}

#[test]
fn new_epoch_discards_queued_stale_blocks_before_playing_current_pcm() {
    let (mut producer, mut consumer) = realtime_pcm_queue::<1>(2).unwrap();
    producer.push(&[1.0]);
    producer.push(&[1.5]);
    producer.begin_epoch();
    producer.push(&[2.0]);
    let mut output = [0.0];
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled {
            discontinuity: true,
            discarded_stale_blocks: 1,
            ..
        }
    ));
    assert!((output[0] - 2.0).abs() < f32::EPSILON);
}

#[test]
fn stale_only_epoch_queue_emits_silence_and_reports_discards() {
    let (mut producer, mut consumer) = realtime_pcm_queue::<1>(2).unwrap();
    producer.push(&[1.0]);
    producer.begin_epoch();
    let mut output = [f32::NAN];

    assert_eq!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Underflow {
            discarded_stale_blocks: 1,
        }
    );
    assert!(output.iter().all(|sample| sample.abs() < f32::EPSILON));
}

#[test]
fn concurrent_transfer_preserves_order_and_accounts_for_overwrite() {
    const TOTAL: u64 = 50_000;
    let (mut producer, mut consumer) = realtime_pcm_queue::<1>(32).unwrap();
    let done = Arc::new(AtomicBool::new(false));
    let producer_done = Arc::clone(&done);
    let producer_thread = thread::spawn(move || {
        let mut overwritten = 0_u64;
        for _ in 0..TOTAL {
            if producer.push(&[1.0]) == RealtimePcmPushOutcome::ReplacedOldest {
                overwritten += 1;
            }
        }
        producer_done.store(true, Ordering::Release);
        overwritten
    });

    let mut output = [0.0];
    let mut received_count = 0_u64;
    let mut expected = 0_u64;
    loop {
        match consumer.pop_into(&mut output) {
            RealtimePcmPopOutcome::Filled {
                sequence,
                discontinuity,
                ..
            } => {
                assert_eq!(discontinuity, sequence != expected);
                assert!(sequence >= expected);
                expected = sequence + 1;
                received_count += 1;
            }
            RealtimePcmPopOutcome::Underflow { .. } => {
                if done.load(Ordering::Acquire) && consumer.is_empty() {
                    break;
                }
                thread::yield_now();
            }
        }
    }

    let overwritten = producer_thread.join().unwrap();
    assert_eq!(received_count + overwritten, TOTAL);
}
