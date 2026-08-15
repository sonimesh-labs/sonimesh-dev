//! Windows native libopus build and decode/PLC feasibility probe for SoniMesh.
//!
//! This prototype produces ADR 0009 evidence only: it proves the candidate
//! opus binding builds on this host, encodes a deterministic tone, decodes
//! a steady-state history of frames for each negotiated shape (10 ms and
//! 20 ms stereo) through the sonimesh-audio StreamDecodeWorker boundary,
//! compares decoded and PLC-concealed blocks against the source, and sweeps
//! bounded deterministic malformed payloads that must surface as
//! stream-local errors or bounded decodes. It is not a quality, power,
//! latency, Android ABI, or coverage-guided fuzz result.

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
/// Tone frames decoded before one frame is dropped to probe PLC.
const HISTORY_FRAMES: usize = 10;
/// Deterministic malformed-payload sweep size.
const MALFORMED_SWEEP_PAYLOADS: usize = 1_000;
/// Lowest plausible RMS for a decoded 0.25-amplitude tone block.
const DECODED_RMS_MIN: f32 = 0.05;
/// Highest plausible RMS for a decoded 0.25-amplitude tone block.
const DECODED_RMS_MAX: f32 = 0.35;
/// PLC concealment must remain non-silent for the first concealed block.
const PLC_RMS_MIN: f32 = 1.0e-3;
/// A steady-state decoded tone must retain at least this SNR against the source.
const DECODE_SNR_MIN: f32 = 10.0;

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

/// Runs the steady-state encode/decode/PLC path for one negotiated duration.
fn run_frame_path(frame_duration_ms: u16) -> ProbeResult<String> {
    let frame = OpusFrameConfig::new(SAMPLE_RATE_HZ, 2, frame_duration_ms)
        .map_err(|error| error.to_string())?;
    let expected_samples = frame.interleaved_samples();

    // A continuous tone long enough for the history plus one dropped frame.
    let tone_block = tone((HISTORY_FRAMES + 1) * expected_samples);

    let mut encoder = opus::Encoder::new(
        SAMPLE_RATE_HZ,
        opus::Channels::Stereo,
        opus::Application::Audio,
    )
    .map_err(|error| error.to_string())?;
    // libopus adds an encoder-lookahead delay (312 samples/channel at 48 kHz);
    // decoded blocks are aligned to the source only after skipping it.
    let lookahead = usize::try_from(encoder.get_lookahead().map_err(|error| error.to_string())?)
        .expect("lookahead is non-negative");
    let delay = lookahead * OPUS_CHANNELS as usize;

    let mut packets = [[0_u8; MAX_ENCODED_BYTES]; HISTORY_FRAMES];
    let mut encoded_lens = [0_usize; HISTORY_FRAMES];
    for index in 0..HISTORY_FRAMES {
        let start = index * expected_samples;
        encoded_lens[index] = encoder
            .encode_float(
                &tone_block[start..start + expected_samples],
                &mut packets[index],
            )
            .map_err(|error| error.to_string())?;
        if encoded_lens[index] == 0 || encoded_lens[index] > MAX_ENCODED_BYTES {
            return Err(format!(
                "encoder produced an invalid length {}",
                encoded_lens[index]
            ));
        }
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

    let mut block = [0.0_f32; PCM_SAMPLES_PER_BLOCK];
    for index in 0..HISTORY_FRAMES {
        worker
            .admit(
                u32::try_from(index).expect("history frame index fits u32"),
                0,
                &packets[index][..encoded_lens[index]],
            )
            .map_err(|error| error.to_string())?;
        match worker.decode_next().map_err(|error| error.to_string())? {
            DecodeStep::Decoded { samples, .. } if samples == expected_samples => {}
            other => return Err(format!("expected a decoded frame, got {other:?}")),
        }
        assert_popped_pcm(&mut consumer, &mut block, expected_samples)?;
    }

    // Steady-state decode quality from the last decoded frame, compensated
    // for the codec lookahead delay.
    let decoded_slice = &block[..expected_samples];
    let decoded_rms = block_rms(decoded_slice);
    if !(DECODED_RMS_MIN..=DECODED_RMS_MAX).contains(&decoded_rms) {
        return Err(format!(
            "decoded tone RMS {decoded_rms:.3} outside the expected band"
        ));
    }
    let decoded_frame_start = (HISTORY_FRAMES - 1) * expected_samples - delay;
    let decode_snr = snr_db(
        &tone_block[decoded_frame_start..decoded_frame_start + expected_samples],
        decoded_slice,
    );
    if !decode_snr.is_finite() || decode_snr < DECODE_SNR_MIN {
        return Err(format!(
            "decoded tone SNR {decode_snr:.1} dB below the expected floor"
        ));
    }

    // The next sequence is never admitted, so the next playout decision is
    // deterministic decoder-side PLC for the confirmed missing frame.
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

    // Compare the concealment against the actual dropped frame. The alignment
    // search compensates for the codec's algorithmic delay; the found delay is
    // reported so the metric stays reproducible.
    let dropped_start = HISTORY_FRAMES * expected_samples;
    let (plc_snr, plc_delay) = best_aligned_snr(
        &tone_block[dropped_start..dropped_start + expected_samples],
        &block[..PCM_SAMPLES_PER_BLOCK],
    );
    if !plc_snr.is_finite() {
        return Err(format!("PLC-loss SNR {plc_snr:.1} dB is non-finite"));
    }

    let encode_per_frame = encoded_lens.iter().sum::<usize>() / HISTORY_FRAMES;
    Ok(format!(
        "opus_{frame_duration_ms}ms: ok (encode {encode_per_frame} B/frame; decode {expected_samples}/PLC {PCM_SAMPLES_PER_BLOCK} samples; snr {decode_snr:.1}/{plc_snr:.1} dB; delay {delay} samples; plc align {plc_delay})"
    ))
}

/// Returns the signal-to-noise ratio in decibels between `reference` and
/// `decoded` over the overlapping region, or infinity when either side is
/// digital silence.
#[must_use]
fn snr_db(reference: &[f32], decoded: &[f32]) -> f32 {
    let len = reference.len().min(decoded.len());
    if len == 0 {
        return f32::INFINITY;
    }
    let mut signal = 0.0_f32;
    let mut error = 0.0_f32;
    for index in 0..len {
        let expected = reference[index];
        let actual = decoded[index];
        signal += expected * expected;
        error += (actual - expected) * (actual - expected);
    }
    if signal <= f32::EPSILON || error <= f32::EPSILON {
        return f32::INFINITY;
    }
    10.0 * (signal / error).log10()
}

/// Returns the best SNR over bounded one-sided alignments, with the delay used.
///
/// Opus's constant algorithmic delay means the same audio appears at a fixed
/// offset; searching that offset keeps the comparison model-free while the
/// reported delay makes the result reproducible.
fn best_aligned_snr(reference: &[f32], decoded: &[f32]) -> (f32, i32) {
    const MIN_OVERLAP: usize = 64;
    let mut best = f32::NEG_INFINITY;
    let mut best_delay = 0_i32;
    let limit = reference
        .len()
        .min(decoded.len())
        .saturating_sub(MIN_OVERLAP);
    for offset in 0..=limit {
        let forward = snr_db(&reference[..reference.len() - offset], &decoded[offset..]);
        if forward > best {
            best = forward;
            best_delay = i32::try_from(offset).expect("alignment offset fits i32");
        }
        let backward = snr_db(&reference[offset..], &decoded[..decoded.len() - offset]);
        if backward > best {
            best = backward;
            best_delay = i32::try_from(offset)
                .expect("alignment offset fits i32")
                .wrapping_neg();
        }
    }
    (best, best_delay)
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

#[cfg(test)]
mod tests {
    use super::{best_aligned_snr, snr_db};

    #[test]
    fn snr_db_identical_signals_are_infinite() {
        let signal = [0.25_f32, -0.25, 0.5, -0.5];
        assert!(snr_db(&signal, &signal).is_infinite());
    }

    #[test]
    fn snr_db_matches_analytic_ratio() {
        let reference = [1.0_f32; 4];
        let decoded = [0.5_f32; 4];
        let expected = 10.0 * (4.0_f32).log10();
        assert!((snr_db(&reference, &decoded) - expected).abs() < 1.0e-5);
    }

    #[test]
    fn snr_db_silence_or_empty_is_infinite() {
        let silence = [0.0_f32; 8];
        assert!(snr_db(&silence, &[0.1_f32; 8]).is_infinite());
        assert!(snr_db(&[], &[1.0_f32]).is_infinite());
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn best_aligned_snr_identical_is_zero_delay() {
        let signal: Vec<f32> = (0..256)
            .map(|index| (index as f32 * 0.1).sin() * 0.25)
            .collect();
        let (snr, delay) = best_aligned_snr(&signal, &signal);
        assert_eq!(delay, 0);
        assert!(snr.is_infinite());
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn best_aligned_snr_recovers_lagging_decoded_signal() {
        // decoded[j] = reference[j - lag] for j >= lag: the decoded stream
        // repeats the source `lag` samples later, reported as a positive delay.
        let reference: Vec<f32> = (0..512)
            .map(|index| (index as f32 * 0.1).sin() * 0.25)
            .collect();
        let lag = 37_usize;
        let mut decoded = vec![0.0_f32; reference.len()];
        decoded[lag..].copy_from_slice(&reference[..reference.len() - lag]);
        let (snr, delay) = best_aligned_snr(&reference, &decoded);
        assert_eq!(delay, i32::try_from(lag).expect("lag fits i32"));
        assert!(snr.is_infinite());
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn best_aligned_snr_recovers_leading_decoded_signal() {
        // decoded[j] = reference[j + lead] for j < len - lead: the decoded
        // stream repeats the source `lead` samples earlier, reported negative.
        let reference: Vec<f32> = (0..512)
            .map(|index| (index as f32 * 0.1).sin() * 0.25)
            .collect();
        let lead = 41_usize;
        let mut decoded = vec![0.0_f32; reference.len()];
        decoded[..reference.len() - lead].copy_from_slice(&reference[lead..]);
        let (snr, delay) = best_aligned_snr(&reference, &decoded);
        assert_eq!(delay, -i32::try_from(lead).expect("lead fits i32"));
        assert!(snr.is_infinite());
    }
}
