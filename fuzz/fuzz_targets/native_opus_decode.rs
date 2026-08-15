#![no_main]

//! Fuzz the native libopus decoder through the bounded decode worker.
//!
//! Arbitrary post-authentication bytes are admitted into a fixed-capacity
//! jitter buffer and decoded by the bundled libopus binding outside the
//! realtime callback. The target proves adversarial payloads never panic the
//! worker or the native decoder, never exceed the fixed encoded-frame or PCM
//! bounds, and leave every consumer-visible block initialized to finite
//! samples (PLC or silence when a frame is missing or rejected).

use libfuzzer_sys::fuzz_target;
use sonimesh_audio::{
    DecodeStep, FrameDecoder, OpusFrameConfig, RealtimePcmPopOutcome, StreamDecodeWorker,
    realtime_pcm_queue,
};

/// Fixed per-stream encoded-frame capacity in frames, matching the probe.
const JITTER_CAPACITY: usize = 8;
/// Fixed upper bound on one authenticated Opus payload, matching the probe.
const MAX_ENCODED_BYTES: usize = 1_200;
/// Caller-owned PCM block for the negotiated 48 kHz stereo 10 ms shape.
const PCM_SAMPLES_PER_BLOCK: usize = 960;
/// Bounded realtime PCM queue capacity in blocks.
const QUEUE_CAPACITY: usize = 8;
const SAMPLE_RATE_HZ: u32 = 48_000;
const CHANNELS: u16 = 2;

/// Native libopus decoder behind the stable per-stream [`FrameDecoder`] boundary.
struct NativeOpusDecoder {
    decoder: opus::Decoder,
}

impl FrameDecoder for NativeOpusDecoder {
    type Error = opus::Error;

    fn decode(&mut self, encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
        // libopus reports samples per channel; the trait requires the total
        // interleaved samples initialized in the caller-owned block.
        let per_channel = self.decoder.decode_float(encoded, output, false)?;
        Ok(per_channel.saturating_mul(CHANNELS as usize))
    }

    fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
        // An empty packet makes libopus synthesize exactly one missing frame.
        let per_channel = self.decoder.decode_float(&[], output, false)?;
        Ok(per_channel.saturating_mul(CHANNELS as usize))
    }
}

fuzz_target!(|data: &[u8]| {
    let (producer, mut consumer) =
        realtime_pcm_queue::<PCM_SAMPLES_PER_BLOCK>(QUEUE_CAPACITY).expect("fixed queue is valid");
    let decoder = NativeOpusDecoder {
        decoder: opus::Decoder::new(SAMPLE_RATE_HZ, opus::Channels::Stereo)
            .expect("native opus decoder is valid"),
    };
    let frame = OpusFrameConfig::new(SAMPLE_RATE_HZ, CHANNELS, 10).expect("10 ms shape is valid");
    let mut worker = StreamDecodeWorker::<
        NativeOpusDecoder,
        JITTER_CAPACITY,
        MAX_ENCODED_BYTES,
        PCM_SAMPLES_PER_BLOCK,
    >::new_for_opus(decoder, producer, 0, frame)
    .expect("PCM block fits the negotiated frame");

    // Admit up to one frame per input byte with arbitrary sequences and
    // timestamps, then drive playout a bounded number of times. Every decoder
    // failure is stream-local and must not panic or disturb other streams.
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
        match worker.decode_next() {
            Ok(DecodeStep::NotReady) => {}
            Ok(DecodeStep::Decoded { samples, .. } | DecodeStep::Concealed { samples, .. }) => {
                assert!((1..=PCM_SAMPLES_PER_BLOCK).contains(&samples));
            }
            Err(_) => {} // Malformed payloads surface as bounded stream-local errors.
        }
    }

    let mut block = [f32::NAN; PCM_SAMPLES_PER_BLOCK];
    match consumer.pop_into(&mut block) {
        RealtimePcmPopOutcome::Filled { samples, .. } => {
            assert!((1..=PCM_SAMPLES_PER_BLOCK).contains(&samples));
            assert!(block[..samples].iter().all(|sample| sample.is_finite()));
        }
        RealtimePcmPopOutcome::Underflow { .. } => {
            // The callback-owned block is initialized to silence on underflow.
            assert!(block.iter().all(|sample| sample.is_finite()));
        }
    }
    assert!(worker.queued_blocks() <= QUEUE_CAPACITY);
    assert!(worker.jitter_len() <= JITTER_CAPACITY);
});
