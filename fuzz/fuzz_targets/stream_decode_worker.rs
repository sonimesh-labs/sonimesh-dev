#![no_main]

//! Fuzz the authenticated decode-worker boundary.
//!
//! The worker admits already-authenticated payloads into a fixed-capacity
//! jitter buffer and schedules ordinary decode or PLC outside the realtime
//! callback. Arbitrary post-authentication bytes must never panic the worker,
//! exceed the fixed encoded-frame capacity, or grow the PCM queue past its
//! bound; the consumer must always observe initialized finite samples.

use libfuzzer_sys::fuzz_target;
use sonimesh_audio::{FrameDecoder, RealtimePcmPopOutcome, StreamDecodeWorker, realtime_pcm_queue};

const JITTER_CAPACITY: usize = 16;
const MAX_ENCODED_BYTES: usize = 8;
const PCM_SAMPLES_PER_BLOCK: usize = 1;
const QUEUE_CAPACITY: usize = 4;

/// Stateless decoder that accepts any bounded payload and writes one sample.
struct FuzzDecoder;

impl FrameDecoder for FuzzDecoder {
    type Error = core::convert::Infallible;

    fn decode(&mut self, encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
        output[0] = f32::from(encoded.first().copied().unwrap_or_default());
        Ok(1)
    }

    fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
        output[0] = 0.0;
        Ok(1)
    }
}

fuzz_target!(|data: &[u8]| {
    let (producer, mut consumer) =
        realtime_pcm_queue::<PCM_SAMPLES_PER_BLOCK>(QUEUE_CAPACITY).expect("fixed queue is valid");
    let mut worker = StreamDecodeWorker::<
        FuzzDecoder,
        JITTER_CAPACITY,
        MAX_ENCODED_BYTES,
        PCM_SAMPLES_PER_BLOCK,
    >::new(FuzzDecoder, producer, 0);

    // Admit up to one frame per input byte with arbitrary sequences and
    // timestamps, then drive the playout a bounded number of times. Every
    // outcome must stay inside the fixed jitter and PCM bounds.
    let frame_count = data.len().min(JITTER_CAPACITY);
    for index in 0..frame_count {
        let byte = data[index];
        let sequence = u32::from(byte)
            ^ u32::try_from(index)
                .expect("small index")
                .wrapping_mul(0x0100_0000);
        let timestamp = u64::from(byte) * 960;
        let payload = &data[index..data.len().min(index + MAX_ENCODED_BYTES)];
        let _ = worker.admit(sequence, timestamp, payload);
    }
    for _ in 0..=JITTER_CAPACITY {
        let _ = worker.decode_next();
    }

    let mut sample = [f32::NAN];
    match consumer.pop_into(&mut sample) {
        RealtimePcmPopOutcome::Filled { samples, .. } => {
            assert!(samples <= PCM_SAMPLES_PER_BLOCK);
            assert!(sample[0].is_finite());
        }
        RealtimePcmPopOutcome::Underflow { .. } => {
            assert_eq!(sample[0], 0.0);
        }
    }
    assert!(worker.queued_blocks() <= QUEUE_CAPACITY);
});
