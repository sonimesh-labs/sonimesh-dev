//! Per-stream decode scheduling tests outside the realtime callback.

use std::fmt;

use sonimesh_audio::{
    DecodeStep, DecodeWorkerError, FrameDecoder, RealtimePcmPopOutcome, RealtimePcmPushOutcome,
    StreamDecodeWorker, realtime_pcm_queue,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TestDecodeError;

impl fmt::Display for TestDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("test decoder failure")
    }
}

impl std::error::Error for TestDecodeError {}

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
    let mut output = [f32::NAN; 4];
    assert_eq!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Underflow {
            discarded_stale_blocks: 0,
        }
    );
    assert!(output.iter().all(|sample| sample.abs() < f32::EPSILON));
}
