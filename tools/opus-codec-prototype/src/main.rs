//! Windows native libopus build and decode/PLC feasibility probe for SoniMesh.
//!
//! This prototype produces ADR 0009 evidence only: it proves the candidate
//! opus binding builds on this host, encodes one deterministic 20 ms stereo
//! frame, decodes it through the sonimesh-audio StreamDecodeWorker
//! boundary, synthesizes one deterministic PLC block, and rejects a malformed
//! payload as a bounded stream-local error. It is not a quality, power,
//! latency, Android ABI, or malformed-payload fuzz result.

use sonimesh_audio::{
    DecodeStep, FrameDecoder, OPUS_CHANNELS, OpusFrameConfig, RealtimePcmConsumer,
    RealtimePcmPopOutcome, StreamDecodeWorker, realtime_pcm_queue,
};

/// Fixed per-stream encoded-frame capacity in frames.
const JITTER_CAPACITY: usize = 8;
/// Maximum encoded payload bytes per frame, bounded by the media datagram limit.
const MAX_ENCODED_BYTES: usize = 1_200;
/// Interleaved Float32 samples per decoded 20 ms stereo block at 48 kHz.
const PCM_SAMPLES_PER_BLOCK: usize = 1_920;
/// Cross-thread PCM queue capacity in blocks.
const PCM_QUEUE_CAPACITY: usize = 8;
/// Sample rate shared by the encoder, decoder, and negotiated frame shape.
const SAMPLE_RATE_HZ: u32 = 48_000;
/// Deterministic probe tone frequency in hertz.
const TONE_HZ: f32 = 440.0;

/// Wraps the candidate libopus decoder behind the stable FrameDecoder
/// boundary so decode and PLC stay outside the realtime output callback.
struct NativeOpusDecoder {
    decoder: opus::Decoder,
}

impl FrameDecoder for NativeOpusDecoder {
    type Error = opus::Error;

    fn decode(&mut self, encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
        // libopus reports samples per channel; the trait requires the total
        // interleaved samples initialized in the caller-owned block.
        let per_channel = self.decoder.decode_float(encoded, output, false)?;
        Ok(per_channel.saturating_mul(OPUS_CHANNELS as usize))
    }

    fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
        let per_channel = self.decoder.decode_float(&[], output, false)?;
        Ok(per_channel.saturating_mul(OPUS_CHANNELS as usize))
    }
}

type ProbeResult<T> = Result<T, String>;

/// Runs every probe step and returns the ordered success lines.
fn run_probe() -> ProbeResult<Vec<String>> {
    let mut lines = Vec::new();

    let frame = OpusFrameConfig::new(SAMPLE_RATE_HZ, 2, 20).map_err(|error| error.to_string())?;

    let mut encoder = opus::Encoder::new(
        SAMPLE_RATE_HZ,
        opus::Channels::Stereo,
        opus::Application::Audio,
    )
    .map_err(|error| error.to_string())?;
    let mut packet = [0_u8; MAX_ENCODED_BYTES];
    let encoded_len = encoder
        .encode_float(&tone(PCM_SAMPLES_PER_BLOCK), &mut packet)
        .map_err(|error| error.to_string())?;
    if encoded_len == 0 || encoded_len > MAX_ENCODED_BYTES {
        return Err(format!("encoder produced an invalid length {encoded_len}"));
    }
    lines.push(format!("opus_encode: ok ({encoded_len} bytes)"));

    let (producer, mut consumer) = realtime_pcm_queue::<PCM_SAMPLES_PER_BLOCK>(PCM_QUEUE_CAPACITY)
        .map_err(|error| error.to_string())?;
    let decoder = NativeOpusDecoder {
        decoder: opus::Decoder::new(SAMPLE_RATE_HZ, opus::Channels::Stereo)
            .map_err(|error| error.to_string())?,
    };
    let mut worker: StreamDecodeWorker<
        NativeOpusDecoder,
        JITTER_CAPACITY,
        MAX_ENCODED_BYTES,
        PCM_SAMPLES_PER_BLOCK,
    > = StreamDecodeWorker::new_for_opus(decoder, producer, 0, frame)
        .map_err(|error| error.to_string())?;

    // The negotiated start sequence admits the encoded frame first, so the
    // first playout decision is a deterministic ordinary decode.
    worker
        .admit(0, 0, &packet[..encoded_len])
        .map_err(|error| error.to_string())?;
    let mut block = [0.0_f32; PCM_SAMPLES_PER_BLOCK];
    match worker.decode_next().map_err(|error| error.to_string())? {
        DecodeStep::Decoded { samples, .. } if samples == PCM_SAMPLES_PER_BLOCK => {}
        other => return Err(format!("expected a decoded frame, got {other:?}")),
    }
    assert_popped_pcm(&mut consumer, &mut block)?;
    lines.push("opus_decode: ok (1920 interleaved samples)".to_owned());

    // With no further frame buffered, the next playout decision is
    // deterministic decoder-side PLC for the confirmed missing sequence.
    match worker.decode_next().map_err(|error| error.to_string())? {
        DecodeStep::Concealed { samples, .. } if samples == PCM_SAMPLES_PER_BLOCK => {}
        other => return Err(format!("expected a concealed frame, got {other:?}")),
    }
    assert_popped_pcm(&mut consumer, &mut block)?;
    lines.push("opus_plc: ok (1920 interleaved samples)".to_owned());

    // A malformed authenticated payload surfaces as a bounded stream-local
    // decoder error and never reaches the PCM queue.
    let malformed = [0xFF_u8; 16];
    let mut bad_decoder = NativeOpusDecoder {
        decoder: opus::Decoder::new(SAMPLE_RATE_HZ, opus::Channels::Stereo)
            .map_err(|error| error.to_string())?,
    };
    if bad_decoder.decode(&malformed, &mut block).is_ok() {
        return Err("malformed payload decoded successfully".to_owned());
    }
    lines.push("opus_malformed_payload: rejected".to_owned());

    lines.push("opus_native_build: ok".to_owned());
    Ok(lines)
}

/// Builds one deterministic 20 ms stereo tone block.
#[must_use]
#[allow(clippy::cast_precision_loss)]
fn tone(samples: usize) -> Vec<f32> {
    (0..samples)
        .map(|index| {
            let phase = 2.0 * std::f32::consts::PI * TONE_HZ * index as f32 / SAMPLE_RATE_HZ as f32;
            0.25 * phase.sin()
        })
        .collect()
}

/// Pops one complete PCM block from the consumer or fails the probe.
fn assert_popped_pcm<const N: usize>(
    consumer: &mut RealtimePcmConsumer<N>,
    block: &mut [f32; N],
) -> ProbeResult<()> {
    match consumer.pop_into(block) {
        RealtimePcmPopOutcome::Filled { samples, .. } if samples == N => Ok(()),
        other => Err(format!("expected a filled PCM block, got {other:?}")),
    }
}

fn main() {
    match run_probe() {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
        }
        Err(error) => {
            eprintln!("opus-codec-prototype: failed: {error}");
            std::process::exit(1);
        }
    }
}
