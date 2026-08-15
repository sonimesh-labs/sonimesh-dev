//! Windows native libopus build and decode/PLC feasibility probe for SoniMesh.
//!
//! This prototype produces ADR 0009 evidence only: it proves the candidate
//! opus binding builds on this host, encodes a deterministic tone, decodes
//! a steady-state history of frames for each negotiated shape (10 ms and
//! 20 ms stereo) through the sonimesh-audio StreamDecodeWorker boundary,
//! conceals a deterministic burst of lost frames with decoder-side PLC,
//! confirms the stream re-locks to the source after the burst, sweeps
//! multiple burst lengths to record PLC degradation and recovery, and sweeps
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
/// Maximum interleaved Float32 samples per decoded stereo block (the 20 ms shape).
const MAX_PCM_SAMPLES_PER_BLOCK: usize = 1_920;
/// Cross-thread PCM queue capacity in blocks.
const PCM_QUEUE_CAPACITY: usize = 8;
/// Sample rate shared by the encoder, decoder, and negotiated frame shape.
const SAMPLE_RATE_HZ: u32 = 48_000;
/// Deterministic probe tone frequency in hertz.
const TONE_HZ: f32 = 440.0;
/// Negotiated frame durations probed through the native decoder.
const FRAME_DURATIONS_MS: [u16; 2] = [10, 20];
/// Tone frames decoded before the loss burst begins.
const HISTORY_FRAMES: usize = 10;
/// Consecutive frames dropped after the history to probe sustained-loss PLC.
const PLC_BURST_FRAMES: usize = 5;
/// Frames admitted and decoded after the burst to confirm stream recovery.
const RECOVERY_FRAMES: usize = 2;
/// Burst lengths swept to record PLC degradation and recovery on this host.
const BURST_LENGTHS: [usize; 6] = [1, 2, 3, 5, 8, 12];
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
/// Every concealed block in the sustained-loss burst must keep at least this SNR.
const PLC_BURST_SNR_MIN: f32 = 1.0;
/// The sustained-loss burst mean SNR must stay at least this high.
const PLC_BURST_MEAN_SNR_MIN: f32 = 10.0;
/// A recovered frame must re-lock to the source at least this SNR.
const RECOVERY_SNR_MIN: f32 = 0.0;

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

/// Encodes one deterministic tone frame into `packet` and validates the length.
fn encode_frame(
    encoder: &mut opus::Encoder,
    source: &[f32],
    packet: &mut [u8; MAX_ENCODED_BYTES],
) -> ProbeResult<usize> {
    let len = encoder
        .encode_float(source, packet)
        .map_err(|error| error.to_string())?;
    if len == 0 || len > MAX_ENCODED_BYTES {
        return Err(format!("encoder produced an invalid length {len}"));
    }
    Ok(len)
}

/// Encodes the deterministic tone history plus the recovery frames that follow
/// one burst, returning packets and their encoded lengths.
///
/// The `recovery_start` is the source frame index of the first frame admitted
/// after the burst; the tone must cover it and the recovery frames that follow.
fn encode_history_and_recovery(
    encoder: &mut opus::Encoder,
    tone_block: &[f32],
    expected_samples: usize,
    recovery_start: usize,
) -> ProbeResult<(Vec<[u8; MAX_ENCODED_BYTES]>, Vec<usize>)> {
    let mut packets = vec![[0_u8; MAX_ENCODED_BYTES]; HISTORY_FRAMES + RECOVERY_FRAMES];
    let mut encoded_lens = vec![0_usize; HISTORY_FRAMES + RECOVERY_FRAMES];
    for index in 0..HISTORY_FRAMES {
        let start = index * expected_samples;
        encoded_lens[index] = encode_frame(
            encoder,
            &tone_block[start..start + expected_samples],
            &mut packets[index],
        )?;
    }
    // The recovery frames sit after the burst in the source tone; encoding
    // them out of order keeps the probe deterministic and skips the lost ones.
    for index in 0..RECOVERY_FRAMES {
        let source_index = recovery_start + index;
        let start = source_index * expected_samples;
        encoded_lens[HISTORY_FRAMES + index] = encode_frame(
            encoder,
            &tone_block[start..start + expected_samples],
            &mut packets[HISTORY_FRAMES + index],
        )?;
    }
    Ok((packets, encoded_lens))
}

/// Runs the steady-state encode/decode/PLC path for one negotiated duration.
///
/// The PCM block is sized to the negotiated frame (10 ms -> 960 samples,
/// 20 ms -> 1,920 samples) so PLC synthesizes exactly one frame of
/// concealment per missing frame on both shapes.
fn run_frame_path(frame_duration_ms: u16) -> ProbeResult<String> {
    match frame_duration_ms {
        10 => run_frame_path_impl::<960>(frame_duration_ms),
        20 => run_frame_path_impl::<MAX_PCM_SAMPLES_PER_BLOCK>(frame_duration_ms),
        other => Err(format!("unsupported frame duration {other} ms")),
    }
}

/// Runs one negotiated-duration probe with a compile-time PCM block capacity.
///
/// A deterministic tone is encoded into a history of frames, decoded through
/// the bounded [`StreamDecodeWorker`] boundary, then a burst of consecutive
/// frames is dropped and concealed by decoder-side PLC while every concealed
/// block is compared against the actual dropped frame. The stream then admits
/// and decodes fresh frames to confirm it re-locks to the source.
fn run_frame_path_impl<const PCM_SAMPLES_PER_BLOCK: usize>(
    frame_duration_ms: u16,
) -> ProbeResult<String> {
    let frame = OpusFrameConfig::new(SAMPLE_RATE_HZ, 2, frame_duration_ms)
        .map_err(|error| error.to_string())?;
    let expected_samples = frame.interleaved_samples();

    // A continuous tone long enough for the history, the loss burst, and the
    // recovery frames that follow it.
    let tone_block = tone((HISTORY_FRAMES + PLC_BURST_FRAMES + RECOVERY_FRAMES) * expected_samples);

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

    let (packets, encoded_lens) = encode_history_and_recovery(
        &mut encoder,
        &tone_block,
        expected_samples,
        HISTORY_FRAMES + PLC_BURST_FRAMES,
    )?;

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
    let decode_snr = steady_state_decode_snr(&block, &tone_block, expected_samples, delay)?;

    // Sustained loss: the next PLC_BURST_FRAMES sequences are never admitted,
    // so playout schedules one Concealed block per missing frame. libopus
    // synthesizes frame_size = block capacity / channels for an empty packet,
    // so PLC fills exactly one negotiated frame on both shapes.
    let (burst_snr, first_align) = measure_loss_burst(
        &mut worker,
        &mut consumer,
        &mut block,
        &tone_block,
        expected_samples,
    )?;
    let plc_snr_min = burst_snr.iter().copied().fold(f32::INFINITY, f32::min);
    let plc_snr_mean = mean_snr(&burst_snr);
    if plc_snr_min < PLC_BURST_SNR_MIN {
        return Err(format!(
            "concealed block SNR min {plc_snr_min:.1} dB collapsed below the floor"
        ));
    }
    if plc_snr_mean < PLC_BURST_MEAN_SNR_MIN {
        return Err(format!(
            "sustained-loss PLC mean SNR {plc_snr_mean:.1} dB below the expected floor"
        ));
    }

    // The stream re-locks after the burst: admit and decode fresh frames, then
    // check the final recovered block against the source with the same
    // model-free alignment used for steady-state decode.
    let recovery_snr = verify_recovery(
        &mut worker,
        &mut consumer,
        &mut block,
        &tone_block,
        &packets,
        &encoded_lens,
        expected_samples,
    )?;
    if !recovery_snr.is_finite() || recovery_snr < RECOVERY_SNR_MIN {
        return Err(format!(
            "recovered frame SNR {recovery_snr:.1} dB below the expected floor"
        ));
    }

    let encode_per_frame = encoded_lens[..HISTORY_FRAMES].iter().sum::<usize>() / HISTORY_FRAMES;
    Ok(format!(
        "opus_{frame_duration_ms}ms: ok (encode {encode_per_frame} B/frame; decode {expected_samples}/PLC {PCM_SAMPLES_PER_BLOCK} samples; snr {decode_snr:.1} dB; plc burst {PLC_BURST_FRAMES} min {plc_snr_min:.1} mean {plc_snr_mean:.1} dB; align {first_align}; recovery {recovery_snr:.1} dB; delay {delay} samples)"
    ))
}

/// Checks the last steady-state decoded block against the source and returns
/// its SNR, compensating for the codec lookahead delay.
fn steady_state_decode_snr(
    block: &[f32],
    tone_block: &[f32],
    expected_samples: usize,
    delay: usize,
) -> ProbeResult<f32> {
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
    Ok(decode_snr)
}

/// Conceals `PLC_BURST_FRAMES` consecutive missing frames and returns each
/// concealed block's aligned SNR against the actual dropped frame, plus the
/// first block's alignment offset.
fn measure_loss_burst<const N: usize>(
    worker: &mut StreamDecodeWorker<NativeOpusDecoder, JITTER_CAPACITY, MAX_ENCODED_BYTES, N>,
    consumer: &mut RealtimePcmConsumer<N>,
    block: &mut [f32; N],
    tone_block: &[f32],
    expected_samples: usize,
) -> ProbeResult<([f32; PLC_BURST_FRAMES], i32)> {
    let mut burst_snr = [0.0_f32; PLC_BURST_FRAMES];
    let mut first_align = 0_i32;
    for (drop_index, slot) in burst_snr.iter_mut().enumerate() {
        match worker.decode_next().map_err(|error| error.to_string())? {
            DecodeStep::Concealed { samples, .. } if samples == expected_samples => {}
            other => return Err(format!("expected a concealed frame, got {other:?}")),
        }
        assert_popped_pcm(consumer, block, expected_samples)?;
        let concealed = &block[..expected_samples];
        let concealed_rms = block_rms(concealed);
        if !concealed_rms.is_finite() || concealed_rms < PLC_RMS_MIN {
            return Err(format!(
                "concealed block {drop_index} RMS {concealed_rms:.3} is silent or non-finite"
            ));
        }
        let dropped_start = (HISTORY_FRAMES + drop_index) * expected_samples;
        let (snr, align) = best_aligned_snr(
            &tone_block[dropped_start..dropped_start + expected_samples],
            concealed,
        );
        if !snr.is_finite() {
            return Err(format!(
                "concealed block {drop_index} SNR {snr:.1} dB is non-finite"
            ));
        }
        if drop_index == 0 {
            first_align = align;
        }
        *slot = snr;
    }
    Ok((burst_snr, first_align))
}

/// Admits and decodes `RECOVERY_FRAMES` fresh frames after the loss burst and
/// returns the final recovered block's aligned SNR against its source frame.
fn verify_recovery<const N: usize>(
    worker: &mut StreamDecodeWorker<NativeOpusDecoder, JITTER_CAPACITY, MAX_ENCODED_BYTES, N>,
    consumer: &mut RealtimePcmConsumer<N>,
    block: &mut [f32; N],
    tone_block: &[f32],
    packets: &[[u8; MAX_ENCODED_BYTES]],
    encoded_lens: &[usize],
    expected_samples: usize,
) -> ProbeResult<f32> {
    for index in 0..RECOVERY_FRAMES {
        let sequence = HISTORY_FRAMES + PLC_BURST_FRAMES + index;
        worker
            .admit(
                u32::try_from(sequence).expect("recovery frame index fits u32"),
                0,
                &packets[HISTORY_FRAMES + index][..encoded_lens[HISTORY_FRAMES + index]],
            )
            .map_err(|error| error.to_string())?;
        match worker.decode_next().map_err(|error| error.to_string())? {
            DecodeStep::Decoded { samples, .. } if samples == expected_samples => {}
            other => return Err(format!("expected a recovered frame, got {other:?}")),
        }
        assert_popped_pcm(consumer, block, expected_samples)?;
    }
    let recovered_frame = HISTORY_FRAMES + PLC_BURST_FRAMES + RECOVERY_FRAMES - 1;
    let recovered_start = recovered_frame * expected_samples;
    let (recovery_snr, _) = best_aligned_snr(
        &tone_block[recovered_start..recovered_start + expected_samples],
        &block[..expected_samples],
    );
    Ok(recovery_snr)
}

/// Conceals one `burst_len`-frame loss burst through a fresh decode worker and
/// returns the worst and mean concealed-block SNR plus the recovery SNR.
///
/// Every concealed block must stay non-silent and finite, and the stream must
/// re-lock to the source after the burst. Quality degradation across longer
/// bursts is reported rather than gated, so the probe records the loss curve
/// instead of assuming one floor holds for every length.
fn run_burst_len<const N: usize>(
    frame: OpusFrameConfig,
    burst_len: usize,
    expected_samples: usize,
) -> ProbeResult<(f32, f32, f32)> {
    let tone_block = tone((HISTORY_FRAMES + burst_len + RECOVERY_FRAMES) * expected_samples);

    let mut encoder = opus::Encoder::new(
        SAMPLE_RATE_HZ,
        opus::Channels::Stereo,
        opus::Application::Audio,
    )
    .map_err(|error| error.to_string())?;
    let (packets, encoded_lens) = encode_history_and_recovery(
        &mut encoder,
        &tone_block,
        expected_samples,
        HISTORY_FRAMES + burst_len,
    )?;

    let (producer, mut consumer) =
        realtime_pcm_queue::<N>(PCM_QUEUE_CAPACITY).map_err(|error| error.to_string())?;
    let decoder = NativeOpusDecoder {
        decoder: opus::Decoder::new(SAMPLE_RATE_HZ, opus::Channels::Stereo)
            .map_err(|error| error.to_string())?,
    };
    let mut worker: StreamDecodeWorker<NativeOpusDecoder, JITTER_CAPACITY, MAX_ENCODED_BYTES, N> =
        StreamDecodeWorker::new_for_opus(decoder, producer, 0, frame)
            .map_err(|error| error.to_string())?;

    let mut block = [0.0_f32; N];
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

    let mut concealed_snr = Vec::with_capacity(burst_len);
    for drop_index in 0..burst_len {
        match worker.decode_next().map_err(|error| error.to_string())? {
            DecodeStep::Concealed { samples, .. } if samples == expected_samples => {}
            other => return Err(format!("expected a concealed frame, got {other:?}")),
        }
        assert_popped_pcm(&mut consumer, &mut block, expected_samples)?;
        let concealed = &block[..expected_samples];
        let concealed_rms = block_rms(concealed);
        if !concealed_rms.is_finite() || concealed_rms < PLC_RMS_MIN {
            return Err(format!(
                "sweep concealed block {drop_index} RMS {concealed_rms:.3} is silent or non-finite"
            ));
        }
        let dropped_start = (HISTORY_FRAMES + drop_index) * expected_samples;
        let (snr, _) = best_aligned_snr(
            &tone_block[dropped_start..dropped_start + expected_samples],
            concealed,
        );
        if !snr.is_finite() {
            return Err(format!(
                "sweep concealed block {drop_index} SNR {snr:.1} dB is non-finite"
            ));
        }
        concealed_snr.push(snr);
    }
    let worst = concealed_snr.iter().copied().fold(f32::INFINITY, f32::min);
    let mean = mean_snr(&concealed_snr);

    for index in 0..RECOVERY_FRAMES {
        let sequence = HISTORY_FRAMES + burst_len + index;
        worker
            .admit(
                u32::try_from(sequence).expect("recovery frame index fits u32"),
                0,
                &packets[HISTORY_FRAMES + index][..encoded_lens[HISTORY_FRAMES + index]],
            )
            .map_err(|error| error.to_string())?;
        match worker.decode_next().map_err(|error| error.to_string())? {
            DecodeStep::Decoded { samples, .. } if samples == expected_samples => {}
            other => return Err(format!("expected a recovered frame, got {other:?}")),
        }
        assert_popped_pcm(&mut consumer, &mut block, expected_samples)?;
    }
    let recovered_frame = HISTORY_FRAMES + burst_len + RECOVERY_FRAMES - 1;
    let recovered_start = recovered_frame * expected_samples;
    let (recovery_snr, _) = best_aligned_snr(
        &tone_block[recovered_start..recovered_start + expected_samples],
        &block[..expected_samples],
    );
    Ok((worst, mean, recovery_snr))
}

/// Runs the burst-length sweep for one negotiated duration and returns one
/// success line per swept length.
fn run_burst_sweep(frame_duration_ms: u16) -> ProbeResult<Vec<String>> {
    match frame_duration_ms {
        10 => run_burst_sweep_impl::<960>(frame_duration_ms),
        20 => run_burst_sweep_impl::<MAX_PCM_SAMPLES_PER_BLOCK>(frame_duration_ms),
        other => Err(format!("unsupported frame duration {other} ms")),
    }
}

/// Sweeps [`BURST_LENGTHS`] through fresh decode workers with a compile-time
/// PCM block capacity and validates each length's sanity and recovery floors.
fn run_burst_sweep_impl<const PCM_SAMPLES_PER_BLOCK: usize>(
    frame_duration_ms: u16,
) -> ProbeResult<Vec<String>> {
    let frame = OpusFrameConfig::new(SAMPLE_RATE_HZ, 2, frame_duration_ms)
        .map_err(|error| error.to_string())?;
    let expected_samples = frame.interleaved_samples();
    let mut lines = Vec::new();
    for &burst_len in &BURST_LENGTHS {
        let (worst, mean, recovery_snr) =
            run_burst_len::<PCM_SAMPLES_PER_BLOCK>(frame, burst_len, expected_samples)?;
        if !recovery_snr.is_finite() || recovery_snr < RECOVERY_SNR_MIN {
            return Err(format!(
                "burst length {burst_len} recovery SNR {recovery_snr:.1} dB below the expected floor"
            ));
        }
        lines.push(format!(
            "opus_{frame_duration_ms}ms_burst: len {burst_len} ok (min {worst:.1} mean {mean:.1} recovery {recovery_snr:.1} dB)"
        ));
    }
    Ok(lines)
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

/// Returns the arithmetic mean of a non-empty finite SNR slice.
#[must_use]
#[allow(clippy::cast_precision_loss)]
fn mean_snr(samples: &[f32]) -> f32 {
    samples.iter().copied().sum::<f32>() / samples.len() as f32
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
    let mut block = [0.0_f32; MAX_PCM_SAMPLES_PER_BLOCK];
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
            Ok(samples) if (1..=MAX_PCM_SAMPLES_PER_BLOCK).contains(&samples) => accepted += 1,
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
        lines.extend(run_burst_sweep(duration_ms)?);
    }

    // A malformed authenticated payload surfaces as a bounded stream-local
    // decoder error and never reaches the PCM queue.
    let mut block = [0.0_f32; MAX_PCM_SAMPLES_PER_BLOCK];
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
