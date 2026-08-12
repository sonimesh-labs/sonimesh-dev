//! Per-stream decode scheduling tests outside the realtime callback.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use sonimesh_audio::{
    DecodeStep, DecodeWorkerError, FrameDecoder, FrameError, InsertOutcome, OpusFrameConfig,
    OpusFrameConfigError, RealtimePcmPopOutcome, RealtimePcmPushOutcome, StreamDecodeWorker,
    realtime_pcm_queue,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TestDecodeError;

impl fmt::Display for TestDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("test decoder failure")
    }
}

impl std::error::Error for TestDecodeError {}

struct SensitiveDecoderError;

impl fmt::Debug for SensitiveDecoderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("encoded frame bytes: [0xDE, 0xAD, 0xBE, 0xEF]")
    }
}

#[test]
fn decoder_error_debug_omits_arbitrary_decoder_details() {
    let error = DecodeWorkerError::Decoder(SensitiveDecoderError);

    let debug = format!("{error:?}");

    assert_eq!(debug, "Decoder");
    assert!(!debug.contains("0xDE"));
}

struct TestDecoder;

impl FrameDecoder for TestDecoder {
    type Error = TestDecodeError;

    fn decode(&mut self, encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
        output[..2].copy_from_slice(&[f32::from(encoded[0]), 0.5]);
        Ok(2)
    }

    fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
        output[..2].copy_from_slice(&[-1.0, -1.0]);
        Ok(2)
    }
}

struct CountingDecoder {
    decodes: Arc<AtomicUsize>,
    plc_calls: Arc<AtomicUsize>,
}

impl FrameDecoder for CountingDecoder {
    type Error = TestDecodeError;

    fn decode(&mut self, _encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
        self.decodes.fetch_add(1, Ordering::Relaxed);
        output[0] = 1.0;
        Ok(1)
    }

    fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
        self.plc_calls.fetch_add(1, Ordering::Relaxed);
        output[0] = -1.0;
        Ok(1)
    }
}

#[test]
fn zero_capacity_jitter_does_not_decode_or_schedule_plc() {
    let decodes = Arc::new(AtomicUsize::new(0));
    let plc_calls = Arc::new(AtomicUsize::new(0));
    let (producer, mut consumer) = realtime_pcm_queue::<4>(2).unwrap();
    let mut worker = StreamDecodeWorker::<CountingDecoder, 0, 8, 4>::new(
        CountingDecoder {
            decodes: Arc::clone(&decodes),
            plc_calls: Arc::clone(&plc_calls),
        },
        producer,
        10,
    );

    assert_eq!(
        worker.admit(10, 9_600, &[3]).unwrap(),
        InsertOutcome::Disabled
    );
    assert_eq!(worker.decode_next().unwrap(), DecodeStep::NotReady);
    assert_eq!(worker.queued_blocks(), 0);
    assert_eq!(decodes.load(Ordering::Relaxed), 0);
    assert_eq!(plc_calls.load(Ordering::Relaxed), 0);

    let mut output = [f32::NAN; 4];
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Underflow { .. }
    ));
}

#[test]
fn decodes_frames_and_runs_plc_before_realtime_consumption() {
    let (producer, mut consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let mut worker = StreamDecodeWorker::<TestDecoder, 4, 8, 4>::new(TestDecoder, producer, 10);
    worker.admit(10, 9_600, &[3]).unwrap();
    worker.admit(12, 11_520, &[5]).unwrap();

    assert_eq!(
        worker.decode_next().unwrap(),
        DecodeStep::Decoded {
            sequence: 10,
            samples: 2,
            queue: RealtimePcmPushOutcome::Inserted,
        }
    );
    assert_eq!(
        worker.decode_next().unwrap(),
        DecodeStep::Concealed {
            sequence: 11,
            samples: 2,
            queue: RealtimePcmPushOutcome::Inserted,
        }
    );
    assert_eq!(
        worker.decode_next().unwrap(),
        DecodeStep::Decoded {
            sequence: 12,
            samples: 2,
            queue: RealtimePcmPushOutcome::Inserted,
        }
    );
    assert_eq!(worker.queued_blocks(), 3);

    let mut output = [f32::NAN; 4];
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled { samples: 2, .. }
    ));
    assert_eq!(&output[..2], &[3.0, 0.5]);
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled { samples: 2, .. }
    ));
    assert_eq!(&output[..2], &[-1.0, -1.0]);
}

struct OversizedDecoder;

impl FrameDecoder for OversizedDecoder {
    type Error = TestDecodeError;

    fn decode(&mut self, _encoded: &[u8], _output: &mut [f32]) -> Result<usize, Self::Error> {
        Ok(5)
    }

    fn decode_plc(&mut self, _output: &mut [f32]) -> Result<usize, Self::Error> {
        Ok(5)
    }
}

#[test]
fn rejects_decoder_lengths_beyond_the_pcm_capacity() {
    let (producer, mut consumer) = realtime_pcm_queue::<4>(2).unwrap();
    let mut worker =
        StreamDecodeWorker::<OversizedDecoder, 2, 8, 4>::new(OversizedDecoder, producer, 0);
    worker.admit(0, 0, &[1]).unwrap();

    assert_eq!(
        worker.decode_next(),
        Err(DecodeWorkerError::InvalidOutputLength)
    );
    worker.admit(2, 1_920, &[1]).unwrap();
    assert_eq!(
        worker.decode_next(),
        Err(DecodeWorkerError::InvalidOutputLength)
    );
    let mut output = [f32::NAN; 4];
    assert_eq!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Underflow {
            discarded_stale_blocks: 0,
        }
    );
    assert!(output.iter().all(|sample| sample.abs() < f32::EPSILON));
}

struct EmptyDecoder;

impl FrameDecoder for EmptyDecoder {
    type Error = TestDecodeError;

    fn decode(&mut self, _encoded: &[u8], _output: &mut [f32]) -> Result<usize, Self::Error> {
        Ok(0)
    }

    fn decode_plc(&mut self, _output: &mut [f32]) -> Result<usize, Self::Error> {
        Ok(0)
    }
}

#[test]
fn rejects_empty_encoded_payload_before_decoder_work() {
    let (producer, mut consumer) = realtime_pcm_queue::<4>(2).unwrap();
    let mut worker = StreamDecodeWorker::<TestDecoder, 2, 8, 4>::new(TestDecoder, producer, 0);

    assert_eq!(
        worker.admit(0, 0, &[]),
        Err(DecodeWorkerError::Frame(FrameError::EmptyPayload))
    );
    assert_eq!(worker.queued_blocks(), 0);

    let mut output = [f32::NAN; 4];
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Underflow { .. }
    ));
    assert!(output.iter().all(|sample| sample.abs() < f32::EPSILON));
}

#[test]
fn rejects_empty_decoder_output_without_queuing_a_block() {
    let (producer, mut consumer) = realtime_pcm_queue::<4>(2).unwrap();
    let mut worker = StreamDecodeWorker::<EmptyDecoder, 2, 8, 4>::new(EmptyDecoder, producer, 0);
    worker.admit(0, 0, &[1]).unwrap();

    assert_eq!(
        worker.decode_next(),
        Err(DecodeWorkerError::InvalidOutputLength)
    );
    let mut output = [f32::NAN; 4];
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Underflow { .. }
    ));
    assert!(output.iter().all(|sample| sample.abs() < f32::EPSILON));
}

struct FailFirstDecoder {
    fail_decode: bool,
    fail_plc: bool,
}

impl FrameDecoder for FailFirstDecoder {
    type Error = TestDecodeError;

    fn decode(&mut self, encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
        if self.fail_decode {
            self.fail_decode = false;
            return Err(TestDecodeError);
        }
        output[..2].copy_from_slice(&[f32::from(encoded[0]), 0.5]);
        Ok(2)
    }

    fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
        if self.fail_plc {
            self.fail_plc = false;
            return Err(TestDecodeError);
        }
        output[..2].copy_from_slice(&[-1.0, -1.0]);
        Ok(2)
    }
}

#[test]
fn decoder_failure_does_not_queue_pcm_and_the_worker_recovers() {
    let (producer, mut consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let mut worker = StreamDecodeWorker::<FailFirstDecoder, 4, 8, 4>::new(
        FailFirstDecoder {
            fail_decode: true,
            fail_plc: false,
        },
        producer,
        10,
    );
    worker.admit(10, 9_600, &[3]).unwrap();
    worker.admit(11, 10_560, &[4]).unwrap();

    assert_eq!(
        worker.decode_next(),
        Err(DecodeWorkerError::Decoder(TestDecodeError))
    );
    assert_eq!(worker.queued_blocks(), 0);

    let mut output = [f32::NAN; 4];
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Underflow { .. }
    ));
    assert!(output.iter().all(|sample| sample.abs() < f32::EPSILON));

    assert!(matches!(
        worker.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: 11, .. }
    ));
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled { samples: 2, .. }
    ));
    assert_eq!(&output[..2], &[4.0, 0.5]);
}

#[test]
fn plc_failure_does_not_queue_pcm_and_the_worker_recovers() {
    let (producer, mut consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let mut worker = StreamDecodeWorker::<FailFirstDecoder, 4, 8, 4>::new(
        FailFirstDecoder {
            fail_decode: false,
            fail_plc: true,
        },
        producer,
        10,
    );
    worker.admit(10, 9_600, &[3]).unwrap();
    worker.admit(12, 11_520, &[5]).unwrap();

    assert!(matches!(
        worker.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: 10, .. }
    ));
    assert_eq!(
        worker.decode_next(),
        Err(DecodeWorkerError::Decoder(TestDecodeError))
    );
    assert_eq!(worker.queued_blocks(), 1);
    assert!(matches!(
        worker.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: 12, .. }
    ));

    let mut output = [f32::NAN; 4];
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled { samples: 2, .. }
    ));
    assert_eq!(&output[..2], &[3.0, 0.5]);
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled { samples: 2, .. }
    ));
    assert_eq!(&output[..2], &[5.0, 0.5]);
}

#[test]
fn new_decode_epoch_discards_retired_jitter_and_pcm() {
    let (producer, mut consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let mut worker = StreamDecodeWorker::<TestDecoder, 4, 8, 4>::new(TestDecoder, producer, 0);
    worker.admit(0, 0, &[1]).unwrap();
    assert!(matches!(
        worker.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: 0, .. }
    ));

    worker.begin_epoch(0);
    worker.admit(0, 0, &[2]).unwrap();
    assert!(matches!(
        worker.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: 0, .. }
    ));

    let mut output = [f32::NAN; 4];
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled {
            sequence: 0,
            discontinuity: true,
            discarded_stale_blocks: 1,
            ..
        }
    ));
    assert_eq!(&output[..2], &[2.0, 0.5]);
}

struct ResetTrackingDecoder {
    resets: Arc<AtomicUsize>,
}

impl FrameDecoder for ResetTrackingDecoder {
    type Error = TestDecodeError;

    fn decode(&mut self, encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
        output[0] = f32::from(encoded[0]);
        Ok(1)
    }

    fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
        output[0] = -1.0;
        Ok(1)
    }

    fn reset(&mut self) {
        self.resets.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn hard_resync_clears_jitter_decoder_history_and_retired_pcm() {
    let resets = Arc::new(AtomicUsize::new(0));
    let (producer, mut consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let mut worker = StreamDecodeWorker::<ResetTrackingDecoder, 4, 8, 4>::new(
        ResetTrackingDecoder {
            resets: Arc::clone(&resets),
        },
        producer,
        10,
    );
    worker.admit(10, 9_600, &[1]).unwrap();
    worker.admit(11, 10_560, &[2]).unwrap();
    assert!(matches!(
        worker.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: 10, .. }
    ));

    worker.hard_resynchronize();

    assert_eq!(resets.load(Ordering::Relaxed), 1);
    assert_eq!(worker.queued_blocks(), 0);
    assert_eq!(worker.decode_next().unwrap(), DecodeStep::NotReady);
    let mut output = [f32::NAN; 4];
    assert_eq!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Underflow {
            discarded_stale_blocks: 1,
        }
    );

    worker.admit(20, 19_200, &[3]).unwrap();
    assert!(matches!(
        worker.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: 20, .. }
    ));
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled {
            sequence: 0,
            discontinuity: true,
            discarded_stale_blocks: 0,
            ..
        }
    ));
    assert!((output[0] - 3.0).abs() < f32::EPSILON);
}

#[test]
fn jitter_resync_preserves_the_new_frame_and_retires_previous_pcm() {
    let resets = Arc::new(AtomicUsize::new(0));
    let (producer, mut consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let mut worker = StreamDecodeWorker::<ResetTrackingDecoder, 2, 8, 4>::new(
        ResetTrackingDecoder {
            resets: Arc::clone(&resets),
        },
        producer,
        0,
    );
    worker.admit(0, 0, &[1]).unwrap();
    assert!(matches!(
        worker.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: 0, .. }
    ));
    worker.admit(1, 960, &[2]).unwrap();

    assert_eq!(
        worker.admit(3, 2_880, &[3]).unwrap(),
        InsertOutcome::Resynchronized { dropped_frames: 1 }
    );
    worker.reset_after_jitter_resynchronization();

    assert_eq!(resets.load(Ordering::Relaxed), 1);
    assert_eq!(worker.queued_blocks(), 0);
    assert!(matches!(
        worker.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: 3, .. }
    ));
    let mut output = [f32::NAN; 4];
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled {
            discontinuity: true,
            discarded_stale_blocks: 1,
            ..
        }
    ));
    assert!((output[0] - 3.0).abs() < f32::EPSILON);
}

#[test]
fn replace_pcm_producer_clears_decode_state_and_uses_the_new_queue() {
    let resets = Arc::new(AtomicUsize::new(0));
    let (producer_a, mut consumer_a) = realtime_pcm_queue::<4>(4).unwrap();
    let mut worker = StreamDecodeWorker::<ResetTrackingDecoder, 4, 8, 4>::new(
        ResetTrackingDecoder {
            resets: Arc::clone(&resets),
        },
        producer_a,
        10,
    );
    worker.admit(10, 9_600, &[1]).unwrap();
    worker.admit(11, 10_560, &[2]).unwrap();
    assert!(matches!(
        worker.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: 10, .. }
    ));
    assert_eq!(worker.queued_blocks(), 1);

    let (producer_b, mut consumer_b) = realtime_pcm_queue::<4>(4).unwrap();
    worker.replace_pcm_producer(producer_b);

    assert_eq!(worker.queued_blocks(), 0);
    assert_eq!(resets.load(Ordering::Relaxed), 1);
    assert_eq!(worker.decode_next().unwrap(), DecodeStep::NotReady);

    worker.admit(20, 19_200, &[3]).unwrap();
    assert!(matches!(
        worker.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: 20, .. }
    ));
    let mut output = [f32::NAN; 4];
    assert!(matches!(
        consumer_b.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled { samples: 1, .. }
    ));
    assert!((output[0] - 3.0).abs() < f32::EPSILON);

    assert!(matches!(
        consumer_a.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled { samples: 1, .. }
    ));
    assert!((output[0] - 1.0).abs() < f32::EPSILON);
    assert!(matches!(
        consumer_a.pop_into(&mut output),
        RealtimePcmPopOutcome::Underflow { .. }
    ));
}

#[test]
fn opus_constructor_accepts_exact_twenty_ms_pcm_capacity() {
    let (producer, _consumer) = realtime_pcm_queue::<1_920>(2).unwrap();
    let frame = OpusFrameConfig::new(48_000, 2, 20).unwrap();

    let worker = StreamDecodeWorker::<TestDecoder, 2, 8, 1_920>::new_for_opus(
        TestDecoder,
        producer,
        0,
        frame,
    );

    assert!(worker.is_ok());
}

#[test]
fn opus_constructor_rejects_insufficient_pcm_capacity_before_decode() {
    let (producer, _consumer) = realtime_pcm_queue::<1_919>(1).unwrap();
    let frame = OpusFrameConfig::new(48_000, 2, 20).unwrap();

    let worker = StreamDecodeWorker::<TestDecoder, 2, 8, 1_919>::new_for_opus(
        TestDecoder,
        producer,
        0,
        frame,
    );

    assert!(matches!(
        worker,
        Err(OpusFrameConfigError::PcmBlockTooSmall)
    ));
}
