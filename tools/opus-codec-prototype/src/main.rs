//! Windows native libopus build and decode/PLC feasibility probe for SoniMesh.
//!
//! This prototype produces ADR 0009 evidence only: it proves the candidate
//! opus binding builds on this host, encodes one deterministic frame for
//! each negotiated shape (10 ms and 20 ms stereo), decodes it through the
//! sonimesh-audio StreamDecodeWorker boundary, synthesizes one
//! deterministic PLC block per shape (PLC fills the reserved block, as
//! libopus synthesizes the full requested frame size), checks the decoded
//! and concealed blocks for plausible finite energy, and sweeps bounded
//! deterministic malformed payloads that must surface as stream-local
//! errors or bounded decodes. It is not a quality, power, latency,
//! Android ABI, or coverage-guided fuzz result.

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
/// Negotiated frame durations probed through the native decoder.
const FRAME_DURATIONS_MS: [u16; 2] = [10, 20];
/// Deterministic malformed-payload sweep size.
const MALFORMED_SWEEP_PAYLOADS: usize = 1_000;
/// Lowest plausible RMS for a decoded 0.25-amplitude tone block.
const DECODED_RMS_MIN: f32 = 0.05;
/// Highest plausible RMS for a decoded 0.25-amplitude tone block.
const DECODED_RMS_MAX: f32 = 0.35;
/// PLC concealment must remain non-silent for the first concealed block.
const PLC_RMS_MIN: f32 = 1.0e-3;

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

/// Runs the encode/decode/PLC path once for one negotiated frame duration.
fn run_frame_path(frame_duration_ms: u16) -> ProbeResult<String> {
    let frame = OpusFrameConfig::new(SAMPLE_RATE_HZ, 2, frame_duration_ms)
        .map_err(|error| error.to_string())?;
    let expected_samples = frame.interleaved_samples();

    let mut encoder = opus::Encoder::new(
        SAMPLE_RATE_HZ,
        opus::Channels::Stereo,
        opus::Application::Audio,
    )
    .map_err(|error| error.to_string())?;
    let mut packet = [0_u8; MAX_ENCODED_BYTES];
    let encoded_len = encoder
        .encode_float(&tone(expected_samples), &mut packet)
        .map_err(|error| error.to_string())?;
    if encoded_len == 0 || encoded_len > MAX_ENCODED_BYTES {
        return Err(format!("encoder produced an invalid length {encoded_len}"));
    }

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
        DecodeStep::Decoded { samples, .. } if samples == expected_samples => {}
        other => return Err(format!("expected a decoded frame, got {other:?}")),
    }
    assert_popped_pcm(&mut consumer, &mut block, expected_samples)?;
    let decoded_rms = block_rms(&block[..expected_samples]);
    if !(DECODED_RMS_MIN..=DECODED_RMS_MAX).contains(&decoded_rms) {
        return Err(format!(
            "decoded tone RMS {decoded_rms:.3} outside the expected band"
        ));
    }

    // With no further frame buffered, the next playout decision is
    // deterministic decoder-side PLC for the confirmed missing sequence.
    // libopus synthesizes frame_size = block capacity / channels for an empty
    // packet, so PLC fills the reserved 20 ms block even for the 10 ms shape;
    // the worker accepts that bounded length and the consumer pops it whole.
    match worker.decode_next().map_err(|error| error.to_string())? {
        DecodeStep::Concealed { samples, .. } if samples == PCM_SAMPLES_PER_BLOCK => {}
        other => return Err(format!("expected a concealed frame, got {other:?}")),
    }
    assert_popped_pcm(&mut consumer, &mut block, PCM_SAMPLES_PER_BLOCK)?;
    let plc_rms = block_rms(&block[..PCM_SAMPLES_PER_BLOCK]);
    if !plc_rms.is_finite() || plc_rms < PLC_RMS_MIN {
        return Err(format!(
            "PLC block RMS {plc_rms:.3} is silent or non-finite"
        ));
    }

    Ok(format!(
        "opus_{frame_duration_ms}ms: ok (encode {encoded_len} bytes; decode {expected_samples} / PLC {PCM_SAMPLES_PER_BLOCK} interleaved samples; rms {decoded_rms:.3}/{plc_rms:.3})"
    ))
}

/// Returns the root-mean-square of a finite PCM block.
#[must_use]
#[allow(clippy::cast_precision_loss)]
fn block_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum = samples.iter().map(|sample| sample * sample).sum::<f32>();
    (sum / samples.len() as f32).sqrt()
}

/// xorshift32 PRNG used only to make the malformed sweep deterministic.
fn next_random(state: &mut u32) -> u32 {
    let mut value = *state;
    value ^= value << 13;
    value ^= value >> 17;
    value ^= value << 5;
    *state = value;
    value
}

/// Sweeps deterministic malformed payloads through one native decoder.
///
/// Every payload must either be rejected as a stream-local decoder error or
/// decode to a bounded sample count; none may panic or exceed the block.
fn sweep_malformed_payloads() -> ProbeResult<(usize, usize)> {
    let mut rng = 0x5eed_0007_u32;
    let mut rejected = 0_usize;
    let mut accepted = 0_usize;
    let mut block = [0.0_f32; PCM_SAMPLES_PER_BLOCK];
    let mut decoder = NativeOpusDecoder {
        decoder: opus::Decoder::new(SAMPLE_RATE_HZ, opus::Channels::Stereo)
            .map_err(|error| error.to_string())?,
    };
    for _ in 0..MALFORMED_SWEEP_PAYLOADS {
        let length = (next_random(&mut rng) as usize) % (MAX_ENCODED_BYTES + 1);
        let mut payload = [0_u8; MAX_ENCODED_BYTES];
        for byte in &mut payload[..length] {
            *byte = u8::try_from(next_random(&mut rng) & 0xFF).expect("masked byte fits u8");
        }
        match decoder.decode(&payload[..length], &mut block) {
            Ok(samples) if (1..=PCM_SAMPLES_PER_BLOCK).contains(&samples) => accepted += 1,
            Ok(samples) => {
                return Err(format!(
                    "malformed payload accepted with out-of-bounds {samples} samples"
                ));
            }
            Err(_) => rejected += 1,
        }
    }
    Ok((rejected, accepted))
}

/// Runs every probe step and returns the ordered success lines.
fn run_probe() -> ProbeResult<Vec<String>> {
    let mut lines = Vec::new();
    for duration_ms in FRAME_DURATIONS_MS {
        lines.push(run_frame_path(duration_ms)?);
    }

    // A malformed authenticated payload surfaces as a bounded stream-local
    // decoder error and never reaches the PCM queue.
    let mut block = [0.0_f32; PCM_SAMPLES_PER_BLOCK];
    let malformed = [0xFF_u8; 16];
    let mut bad_decoder = NativeOpusDecoder {
        decoder: opus::Decoder::new(SAMPLE_RATE_HZ, opus::Channels::Stereo)
            .map_err(|error| error.to_string())?,
    };
    if bad_decoder.decode(&malformed, &mut block).is_ok() {
        return Err("malformed payload decoded successfully".to_owned());
    }
    lines.push("opus_malformed_payload: rejected".to_owned());

    let (rejected, accepted) = sweep_malformed_payloads()?;
    lines.push(format!(
        "opus_malformed_sweep: ok ({MALFORMED_SWEEP_PAYLOADS} payloads; {rejected} rejected; {accepted} accepted-bounded)"
    ));

    lines.push("opus_native_build: ok".to_owned());
    Ok(lines)
}

/// Builds one deterministic stereo tone block with `samples` interleaved samples.
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
    expected_samples: usize,
) -> ProbeResult<()> {
    match consumer.pop_into(block) {
        RealtimePcmPopOutcome::Filled { samples, .. } if samples == expected_samples => Ok(()),
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
