//! Windows native libopus build and decode/PLC feasibility probe for SoniMesh.
//!
//! This prototype produces ADR 0009 evidence only: it proves the candidate
//! opus binding builds on this host, encodes a deterministic tone, decodes
//! a steady-state history of frames for each negotiated shape (10 ms and
//! 20 ms stereo) through the sonimesh-audio StreamDecodeWorker boundary,
//! conceals a deterministic burst of lost frames with decoder-side PLC,
//! confirms the stream re-locks to the source after the burst, sweeps
//! multiple burst lengths and deterministic loss patterns to record PLC
//! degradation and recovery, probes bounded reorder absorption and late-drop
//! behavior, sweeps deterministic jitter-wait steps that absorb a bounded
//! late frame without concealment or drop it when the wait is too short,
//! measures deterministic per-frame encode+decode and decode-only host
//! throughput for both shapes, compares steady-state and loss-window
//! quality across two deterministic content profiles (a single tone and a
//! multi-harmonic sum), compares in-band FEC recovery against PLC for a
//! dropped 20 ms frame across speech-like and tone content, sweeps the FEC
//! loss rate across the speech-like profile, measures the wall-clock cost of
//! enabling in-band FEC, and sweeps bounded deterministic malformed payloads
//! that must surface as stream-local errors or bounded decodes. It is not a
//! quality, power,
//! latency, Android ABI, or coverage-guided fuzz result.

use sonimesh_audio::{
    DecodeStep, FrameDecoder, InsertOutcome, OPUS_CHANNELS, OpusFrameConfig, RealtimePcmConsumer,
    RealtimePcmPopOutcome, StreamDecodeWorker, realtime_pcm_queue,
};
use std::time::Instant;

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
/// Peak amplitude shared by the tone and multi-harmonic content profiles.
const TONE_AMPLITUDE: f32 = 0.25;
/// Negotiated frame durations probed through the native decoder.
const FRAME_DURATIONS_MS: [u16; 2] = [10, 20];
/// Tone frames decoded before the loss burst begins.
const HISTORY_FRAMES: usize = 10;
/// Consecutive frames dropped after the history to probe sustained-loss PLC.
const PLC_BURST_FRAMES: usize = 5;
/// Frames admitted and decoded after the burst to confirm stream recovery.
const RECOVERY_FRAMES: usize = 2;
/// Burst lengths swept to record PLC degradation and recovery on this host,
/// including sustained lengths beyond 12 frames.
const BURST_LENGTHS: [usize; 9] = [1, 2, 3, 5, 8, 12, 16, 24, 32];
/// Frames in one deterministic loss-pattern window per negotiated shape.
const LOSS_PATTERN_FRAMES: usize = 240;
/// Deterministic seed shared by both shapes so the same frames drop in each.
const LOSS_PATTERN_SEED: u32 = 0x5eed_2026;
/// Every loss pattern must keep at least this mean SNR across the window.
const LOSS_PATTERN_MEAN_SNR_MIN: f32 = 10.0;
/// Frames in one deterministic reorder window after the decoded history.
const REORDER_WINDOW_FRAMES: usize = 6;
/// In-order reorder-window blocks must keep at least this mean SNR.
const REORDER_MEAN_SNR_MIN: f32 = 25.0;
/// Deterministic delay of the delayed frame in one jitter-wait window.
const JITTER_WAIT_DELAY_FRAMES: usize = 4;
/// Jitter-wait steps swept per negotiated shape.
const JITTER_WAIT_STEPS: [usize; 5] = [0, 1, 2, 4, 6];
/// Frames in one deterministic jitter-wait window after the decoded history.
const JITTER_WAIT_WINDOW_FRAMES: usize = 8;
/// Frames timed in the deterministic host-throughput pass per shape.
const CPU_PROBE_FRAMES: usize = 2_000;
/// Frames run untimed before a throughput pass to warm codec state and caches.
const CPU_WARMUP_FRAMES: usize = 200;
/// Timed passes per throughput measurement; the minimum wall time is reported.
const CPU_PROBE_REPEATS: usize = 3;
/// Seconds of audio processed in one sustained throughput pass per shape.
const SUSTAINED_AUDIO_SECONDS: u32 = 120;
/// Sustained passes per shape; the minimum wall time is reported.
const SUSTAINED_PROBE_REPEATS: usize = 2;
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

/// Frame duration probed for in-band FEC recovery; libopus codes LBRR only
/// when a packet carries more than one frame, so 10 ms packets cannot carry it.
const FEC_FRAME_MS: u16 = 20;
/// Source frames encoded in the deterministic in-band FEC recovery probe.
const FEC_SOURCE_FRAMES: usize = 240;
/// Frame index dropped to compare in-band FEC recovery against PLC for it.
const FEC_DROP_FRAME: usize = 120;
/// Encoder packet-loss percentage that enables LBRR coding in the probe.
const FEC_PACKET_LOSS_PERCENT: i32 = 10;
/// Speech-like FEC recovery must beat PLC by at least this many decibels.
const FEC_SPEECH_GAIN_MIN_DB: f32 = 4.0;
/// Non-speech FEC recovery must not fall more than this many decibels below PLC.
const FEC_NON_SPEECH_TOLERANCE_DB: f32 = 2.0;
/// Speech-like loss rates swept by the in-band FEC recovery probe.
const FEC_LOSS_PERCENTS: [i32; 2] = [5, 20];
/// Deterministic drop stride that exercises FEC recovery in the FEC CPU pass.
const FEC_CPU_DROP_EVERY: usize = 10;
/// Low bitrate swept by the FEC CPU pass to record mode-dependent overhead.
const FEC_CPU_LOW_BITRATE_BPS: i32 = 24_000;
/// Glottal pulse rate of the speech-like content profile in hertz.
const SPEECH_F0_HZ: u32 = 120;
/// Glottal pulse period in samples at `SPEECH_F0_HZ` and `SAMPLE_RATE_HZ`.
const SPEECH_PERIOD_SAMPLES: usize = (SAMPLE_RATE_HZ / SPEECH_F0_HZ) as usize;
/// Maximum glottal pulse length in samples before the next pulse begins.
const SPEECH_PULSE_MAX_SAMPLES: usize = 120;
/// Glottal pulse decay time constant in samples.
const SPEECH_PULSE_DECAY_SAMPLES: f32 = 40.0;
/// Glottal pulse attack time constant in samples.
const SPEECH_PULSE_ATTACK_SAMPLES: f32 = 8.0;
/// Formant resonators (frequency, radius) shaping the speech-like pulse train.
const SPEECH_FORMANTS: [(f32, f32); 2] = [(500.0, 0.92), (2_200.0, 0.86)];
/// Syllabic amplitude-modulation rate of the speech-like profile in hertz.
const SPEECH_AM_RATE_HZ: f32 = 4.0;

/// One deterministic loss pattern probed through the native decoder.
#[derive(Clone, Copy)]
enum LossPattern {
    /// No drops; baseline decode quality over the window.
    None,
    /// Each frame is dropped with `percent` probability from a shared seed.
    Independent { percent: u32 },
    /// `burst` consecutive frames are dropped every `period` frames.
    PeriodicBurst { burst: usize, period: usize },
}

impl LossPattern {
    /// Returns the stable report label for this pattern.
    fn label(self) -> String {
        match self {
            Self::None => "none".to_owned(),
            Self::Independent { percent } => format!("independent_{percent}pct"),
            Self::PeriodicBurst { burst, period } => format!("burst_{burst}in{period}"),
        }
    }

    /// Returns whether frame `index` is dropped, drawing from `rng` only for
    /// the random patterns so every run stays reproducible.
    fn drops(self, index: usize, rng: &mut u32) -> bool {
        match self {
            Self::None => false,
            Self::Independent { percent } => next_random(rng) % 100 < percent,
            Self::PeriodicBurst { burst, period } => index % period < burst,
        }
    }
}

/// One deterministic reorder arrival case for the bounded jitter buffer.
#[derive(Clone, Copy)]
enum ReorderCase {
    /// The whole window arrives out of order before playout; the buffer
    /// reorders it and no frame is concealed.
    Absorbed,
    /// One frame arrives after playout passed its slot: the slot is concealed
    /// by PLC and the late frame is dropped by the bounded buffer.
    LateOne,
}

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
/// after the burst; the content must cover it and the recovery frames that follow.
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
fn run_frame_path(frame_duration_ms: u16) -> ProbeResult<Vec<String>> {
    match frame_duration_ms {
        10 => run_frame_path_profiles::<960>(frame_duration_ms),
        20 => run_frame_path_profiles::<MAX_PCM_SAMPLES_PER_BLOCK>(frame_duration_ms),
        other => Err(format!("unsupported frame duration {other} ms")),
    }
}

/// Runs the steady-state path once per content profile for one shape.
fn run_frame_path_profiles<const PCM_SAMPLES_PER_BLOCK: usize>(
    frame_duration_ms: u16,
) -> ProbeResult<Vec<String>> {
    let mut lines = Vec::new();
    for profile in ContentProfile::ALL {
        lines.push(run_frame_path_impl::<PCM_SAMPLES_PER_BLOCK>(
            frame_duration_ms,
            profile,
        )?);
    }
    Ok(lines)
}

/// Runs one negotiated-duration probe with a compile-time PCM block capacity.
///
/// A deterministic content profile is encoded into a history of frames,
/// decoded through the bounded [`StreamDecodeWorker`] boundary, then a burst
/// of consecutive frames is dropped and concealed by decoder-side PLC while
/// every concealed block is compared against the actual dropped frame. The
/// stream then admits and decodes fresh frames to confirm it re-locks to the
/// source.
fn run_frame_path_impl<const PCM_SAMPLES_PER_BLOCK: usize>(
    frame_duration_ms: u16,
    profile: ContentProfile,
) -> ProbeResult<String> {
    let frame = OpusFrameConfig::new(SAMPLE_RATE_HZ, 2, frame_duration_ms)
        .map_err(|error| error.to_string())?;
    let expected_samples = frame.interleaved_samples();

    // A continuous content block long enough for the history, the loss burst,
    // and the recovery frames that follow it.
    let content_block =
        profile.content((HISTORY_FRAMES + PLC_BURST_FRAMES + RECOVERY_FRAMES) * expected_samples);

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
        &content_block,
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
    let decode_snr = steady_state_decode_snr(&block, &content_block, expected_samples, delay)?;

    // Sustained loss: the next PLC_BURST_FRAMES sequences are never admitted,
    // so playout schedules one Concealed block per missing frame. libopus
    // synthesizes frame_size = block capacity / channels for an empty packet,
    // so PLC fills exactly one negotiated frame on both shapes.
    let (burst_snr, first_align) = measure_loss_burst(
        &mut worker,
        &mut consumer,
        &mut block,
        &content_block,
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
        &content_block,
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
    let profile_label = profile.label();
    Ok(format!(
        "opus_{frame_duration_ms}ms_{profile_label}: ok (encode {encode_per_frame} B/frame; decode {expected_samples}/PLC {PCM_SAMPLES_PER_BLOCK} samples; snr {decode_snr:.1} dB; plc burst {PLC_BURST_FRAMES} min {plc_snr_min:.1} mean {plc_snr_mean:.1} dB; align {first_align}; recovery {recovery_snr:.1} dB; delay {delay} samples)"
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
            "decoded content RMS {decoded_rms:.3} outside the expected band"
        ));
    }
    let decoded_frame_start = (HISTORY_FRAMES - 1) * expected_samples - delay;
    let decode_snr = snr_db(
        &tone_block[decoded_frame_start..decoded_frame_start + expected_samples],
        decoded_slice,
    );
    if !decode_snr.is_finite() || decode_snr < DECODE_SNR_MIN {
        return Err(format!(
            "decoded content SNR {decode_snr:.1} dB below the expected floor"
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

/// Runs one deterministic loss pattern through a fresh native decode worker
/// and returns the dropped count, worst-block SNR, and mean SNR over the window.
///
/// The first [`HISTORY_FRAMES`] frames are always admitted as warm-up so PLC
/// never conceals without decoder history; the pattern then applies to every
/// later frame. Each dropped frame is concealed immediately by decoder-side
/// PLC (no jitter wait), matching the probe's burst methodology. Every block is
/// compared to its source frame at the codec's constant delay (the same
/// alignment as the steady-state check) so the metric is a fixed, reproducible
/// comparison rather than a per-block best fit.
fn run_loss_pattern<const N: usize>(
    frame: OpusFrameConfig,
    expected_samples: usize,
    pattern: LossPattern,
    profile: ContentProfile,
) -> ProbeResult<(usize, f32, f32)> {
    let content_block = profile.content(LOSS_PATTERN_FRAMES * expected_samples);
    let mut encoder = opus::Encoder::new(
        SAMPLE_RATE_HZ,
        opus::Channels::Stereo,
        opus::Application::Audio,
    )
    .map_err(|error| error.to_string())?;
    // The codec's constant lookahead delay in interleaved samples: the
    // decoded/concealed block for frame `index` aligns to the source at
    // `index * samples - delay`.
    let lookahead = usize::try_from(encoder.get_lookahead().map_err(|error| error.to_string())?)
        .expect("lookahead is non-negative");
    let delay = lookahead * OPUS_CHANNELS as usize;

    let (producer, mut consumer) =
        realtime_pcm_queue::<N>(PCM_QUEUE_CAPACITY).map_err(|error| error.to_string())?;
    let decoder = NativeOpusDecoder {
        decoder: opus::Decoder::new(SAMPLE_RATE_HZ, opus::Channels::Stereo)
            .map_err(|error| error.to_string())?,
    };
    let mut worker: StreamDecodeWorker<NativeOpusDecoder, JITTER_CAPACITY, MAX_ENCODED_BYTES, N> =
        StreamDecodeWorker::new_for_opus(decoder, producer, 0, frame)
            .map_err(|error| error.to_string())?;

    let mut rng = LOSS_PATTERN_SEED;
    let mut packet = [0_u8; MAX_ENCODED_BYTES];
    let mut block = [0.0_f32; N];
    let mut dropped = 0_usize;
    let mut snrs = Vec::with_capacity(LOSS_PATTERN_FRAMES - 1);
    for index in 0..LOSS_PATTERN_FRAMES {
        if index >= HISTORY_FRAMES && pattern.drops(index, &mut rng) {
            dropped += 1;
            match worker.decode_next().map_err(|error| error.to_string())? {
                DecodeStep::Concealed { samples, .. } if samples == expected_samples => {}
                other => return Err(format!("expected a concealed frame, got {other:?}")),
            }
        } else {
            let start = index * expected_samples;
            let len = encode_frame(
                &mut encoder,
                &content_block[start..start + expected_samples],
                &mut packet,
            )?;
            worker
                .admit(
                    u32::try_from(index).expect("loss-pattern frame index fits u32"),
                    0,
                    &packet[..len],
                )
                .map_err(|error| error.to_string())?;
            match worker.decode_next().map_err(|error| error.to_string())? {
                DecodeStep::Decoded { samples, .. } if samples == expected_samples => {}
                other => return Err(format!("expected a decoded frame, got {other:?}")),
            }
        }
        assert_popped_pcm(&mut consumer, &mut block, expected_samples)?;
        let current = &block[..expected_samples];
        let rms = block_rms(current);
        if !rms.is_finite() || rms < PLC_RMS_MIN {
            return Err(format!(
                "loss-pattern frame {index} RMS {rms:.3} is silent or non-finite"
            ));
        }
        // The first block predates the source content by the codec lookahead
        // and is skipped, so the mean covers LOSS_PATTERN_FRAMES - 1 frames.
        if index == 0 {
            continue;
        }
        let reference_start = index * expected_samples - delay;
        let snr = snr_db(
            &content_block[reference_start..reference_start + expected_samples],
            current,
        );
        if !snr.is_finite() {
            return Err(format!(
                "loss-pattern frame {index} SNR {snr:.1} dB is non-finite"
            ));
        }
        snrs.push(snr);
    }
    let worst = snrs.iter().copied().fold(f32::INFINITY, f32::min);
    let mean = mean_snr(&snrs);
    Ok((dropped, worst, mean))
}

/// Runs every deterministic loss pattern for one negotiated shape and returns
/// one success line per pattern.
fn run_loss_patterns(frame_duration_ms: u16) -> ProbeResult<Vec<String>> {
    match frame_duration_ms {
        10 => run_loss_patterns_impl::<960>(frame_duration_ms),
        20 => run_loss_patterns_impl::<MAX_PCM_SAMPLES_PER_BLOCK>(frame_duration_ms),
        other => Err(format!("unsupported frame duration {other} ms")),
    }
}

/// Sweeps the deterministic loss patterns with a compile-time PCM block capacity.
fn run_loss_patterns_impl<const PCM_SAMPLES_PER_BLOCK: usize>(
    frame_duration_ms: u16,
) -> ProbeResult<Vec<String>> {
    let frame = OpusFrameConfig::new(SAMPLE_RATE_HZ, 2, frame_duration_ms)
        .map_err(|error| error.to_string())?;
    let expected_samples = frame.interleaved_samples();
    let mut lines = Vec::new();
    for profile in ContentProfile::ALL {
        let profile_label = profile.label();
        for pattern in [
            LossPattern::None,
            LossPattern::Independent { percent: 1 },
            LossPattern::Independent { percent: 3 },
            LossPattern::Independent { percent: 5 },
            LossPattern::PeriodicBurst {
                burst: 5,
                period: 50,
            },
        ] {
            let (dropped, worst, mean) = run_loss_pattern::<PCM_SAMPLES_PER_BLOCK>(
                frame,
                expected_samples,
                pattern,
                profile,
            )?;
            if mean < LOSS_PATTERN_MEAN_SNR_MIN {
                return Err(format!(
                    "loss pattern {} mean SNR {mean:.1} dB below the expected floor",
                    pattern.label()
                ));
            }
            lines.push(format!(
                "opus_{frame_duration_ms}ms_{profile_label}_loss: pattern {} ok ({LOSS_PATTERN_FRAMES} frames; {dropped} dropped; mean {mean:.1} worst {worst:.1} dB)",
                pattern.label()
            ));
        }
    }
    Ok(lines)
}

/// Admits one pre-encoded source frame, decodes it in order, and pops the PCM
/// block.
fn admit_preencoded_frame<const N: usize>(
    worker: &mut StreamDecodeWorker<NativeOpusDecoder, JITTER_CAPACITY, MAX_ENCODED_BYTES, N>,
    consumer: &mut RealtimePcmConsumer<N>,
    block: &mut [f32; N],
    packet: &[u8],
    expected_sequence: u32,
    expected_samples: usize,
) -> ProbeResult<()> {
    worker
        .admit(expected_sequence, 0, packet)
        .map_err(|error| error.to_string())?;
    match worker.decode_next().map_err(|error| error.to_string())? {
        DecodeStep::Decoded {
            sequence: decoded,
            samples,
            ..
        } if decoded == expected_sequence && samples == expected_samples => {}
        other => return Err(format!("expected a decoded frame, got {other:?}")),
    }
    assert_popped_pcm(consumer, block, expected_samples)
}

/// Decodes one missing frame with decoder-side PLC and pops the PCM block.
fn conceal_frame<const N: usize>(
    worker: &mut StreamDecodeWorker<NativeOpusDecoder, JITTER_CAPACITY, MAX_ENCODED_BYTES, N>,
    consumer: &mut RealtimePcmConsumer<N>,
    block: &mut [f32; N],
    expected_sequence: u32,
    expected_samples: usize,
) -> ProbeResult<()> {
    match worker.decode_next().map_err(|error| error.to_string())? {
        DecodeStep::Concealed {
            sequence, samples, ..
        } if sequence == expected_sequence && samples == expected_samples => {}
        other => return Err(format!("expected a concealed frame, got {other:?}")),
    }
    assert_popped_pcm(consumer, block, expected_samples)
}

/// Asserts a popped block is non-silent and finite.
fn assert_non_silent(block: &[f32], expected_samples: usize, label: &str) -> ProbeResult<()> {
    let rms = block_rms(&block[..expected_samples]);
    if !rms.is_finite() || rms < PLC_RMS_MIN {
        return Err(format!("{label} RMS {rms:.3} is silent or non-finite"));
    }
    Ok(())
}

/// Returns the SNR of a popped block against its source frame at the codec's
/// constant delay.
///
/// Cleanly decoded frames carry the source delayed by the encoder lookahead;
/// a stream that shifted would collapse this SNR, so it is the robust
/// reorder-absorption check for a periodic tone where model-free alignment
/// can lock onto a tone-period alias instead.
fn snr_vs_source(
    tone_block: &[f32],
    expected_samples: usize,
    source_index: usize,
    delay: usize,
    block: &[f32],
) -> ProbeResult<f32> {
    let frame_start = source_index
        .checked_mul(expected_samples)
        .ok_or_else(|| format!("source frame {source_index} start overflow"))?;
    let reference_start = frame_start
        .checked_sub(delay)
        .ok_or_else(|| format!("source frame {source_index} predates the codec delay"))?;
    let reference_end = reference_start + expected_samples;
    if reference_end > tone_block.len() {
        return Err(format!(
            "source frame {source_index} reference slice out of bounds"
        ));
    }
    let snr = snr_db(
        &tone_block[reference_start..reference_end],
        &block[..expected_samples],
    );
    if !snr.is_finite() {
        return Err(format!(
            "source frame {source_index} SNR {snr:.1} dB is non-finite"
        ));
    }
    Ok(snr)
}

/// Returns the model-free aligned SNR of a popped block against its source
/// frame, used only for concealed blocks whose content has no fixed delay.
fn aligned_snr_vs_source(
    tone_block: &[f32],
    expected_samples: usize,
    source_index: usize,
    block: &[f32],
) -> ProbeResult<f32> {
    let start = source_index
        .checked_mul(expected_samples)
        .ok_or_else(|| format!("source frame {source_index} start overflow"))?;
    let end = start + expected_samples;
    if end > tone_block.len() {
        return Err(format!(
            "source frame {source_index} reference slice out of bounds"
        ));
    }
    let (snr, _) = best_aligned_snr(&tone_block[start..end], &block[..expected_samples]);
    if !snr.is_finite() {
        return Err(format!(
            "source frame {source_index} SNR {snr:.1} dB is non-finite"
        ));
    }
    Ok(snr)
}

/// Immutable source material shared by the two deterministic reorder runs.
struct ReorderSource<'a> {
    /// Pre-encoded packets in source order.
    packets: &'a [[u8; MAX_ENCODED_BYTES]],
    /// Encoded length of each packet.
    encoded_lens: &'a [usize],
    /// Deterministic source tone.
    tone_block: &'a [f32],
    /// Interleaved PCM samples per negotiated frame.
    expected_samples: usize,
    /// Codec lookahead delay in interleaved samples.
    delay: usize,
}

/// Runs one deterministic reorder case through a fresh native decode worker and
/// returns one success line.
///
/// A ten-frame history is always decoded first. The reorder window then either
/// arrives in a fixed out-of-order batch within the 8-slot jitter capacity
/// (absorbed and played out in sequence with no concealment and no alignment
/// shift) or leaves one frame arriving one slot late so its playout slot is
/// concealed by PLC and the late arrival is dropped.
fn run_reorder_case<const N: usize>(
    frame: OpusFrameConfig,
    frame_duration_ms: u16,
    expected_samples: usize,
    case: ReorderCase,
) -> ProbeResult<String> {
    let total_frames = HISTORY_FRAMES + REORDER_WINDOW_FRAMES;
    let tone_block = tone(total_frames * expected_samples);

    let mut encoder = opus::Encoder::new(
        SAMPLE_RATE_HZ,
        opus::Channels::Stereo,
        opus::Application::Audio,
    )
    .map_err(|error| error.to_string())?;
    let lookahead = usize::try_from(encoder.get_lookahead().map_err(|error| error.to_string())?)
        .expect("lookahead is non-negative");
    let delay = lookahead * OPUS_CHANNELS as usize;

    // Pre-encode the history and reorder window in source order so every
    // packet represents the deterministic tone timeline; admission order below
    // is independent of the encoder's sequential state.
    let mut packets = vec![[0_u8; MAX_ENCODED_BYTES]; total_frames];
    let mut encoded_lens = vec![0_usize; total_frames];
    for index in 0..total_frames {
        let start = index * expected_samples;
        encoded_lens[index] = encode_frame(
            &mut encoder,
            &tone_block[start..start + expected_samples],
            &mut packets[index],
        )?;
    }

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
        let sequence = u32::try_from(index).expect("history frame index fits u32");
        admit_preencoded_frame(
            &mut worker,
            &mut consumer,
            &mut block,
            &packets[index][..encoded_lens[index]],
            sequence,
            expected_samples,
        )?;
    }

    let source = ReorderSource {
        packets: &packets,
        encoded_lens: &encoded_lens,
        tone_block: &tone_block,
        expected_samples,
        delay,
    };
    match case {
        ReorderCase::Absorbed => run_absorbed_reorder(
            &mut worker,
            &mut consumer,
            &mut block,
            &source,
            frame_duration_ms,
        ),
        ReorderCase::LateOne => run_late_one_reorder(
            &mut worker,
            &mut consumer,
            &mut block,
            &source,
            frame_duration_ms,
        ),
    }
}

/// Plays out a fixed out-of-order arrival batch in sequence order with no
/// concealment, and asserts every block stays at the codec's constant delay.
fn run_absorbed_reorder<const N: usize>(
    worker: &mut StreamDecodeWorker<NativeOpusDecoder, JITTER_CAPACITY, MAX_ENCODED_BYTES, N>,
    consumer: &mut RealtimePcmConsumer<N>,
    block: &mut [f32; N],
    source: &ReorderSource<'_>,
    frame_duration_ms: u16,
) -> ProbeResult<String> {
    // A fixed out-of-order arrival batch within the 8-slot capacity.
    let arrival = [13_u32, 10, 15, 12, 11, 14];
    for sequence in arrival {
        let index = usize::try_from(sequence).expect("reorder sequence fits usize");
        let outcome = worker
            .admit(
                sequence,
                0,
                &source.packets[index][..source.encoded_lens[index]],
            )
            .map_err(|error| error.to_string())?;
        if !matches!(outcome, InsertOutcome::Inserted | InsertOutcome::Reordered) {
            return Err(format!(
                "reorder batch admit {sequence} returned {outcome:?}"
            ));
        }
    }
    // Playout runs in sequence order with no concealment; every block is
    // compared at the codec's constant delay, so an absorbed window that
    // shifted the stream would collapse the SNR.
    let mut snrs = Vec::with_capacity(REORDER_WINDOW_FRAMES);
    for offset in 0..REORDER_WINDOW_FRAMES {
        let sequence = u32::try_from(HISTORY_FRAMES + offset).expect("reorder sequence fits u32");
        match worker.decode_next().map_err(|error| error.to_string())? {
            DecodeStep::Decoded {
                sequence: actual,
                samples,
                ..
            } if actual == sequence && samples == source.expected_samples => {}
            other => return Err(format!("expected an in-order decode, got {other:?}")),
        }
        assert_popped_pcm(consumer, block, source.expected_samples)?;
        assert_non_silent(block, source.expected_samples, "reorder absorbed block")?;
        snrs.push(snr_vs_source(
            source.tone_block,
            source.expected_samples,
            HISTORY_FRAMES + offset,
            source.delay,
            block,
        )?);
    }
    let worst = snrs.iter().copied().fold(f32::INFINITY, f32::min);
    let mean = mean_snr(&snrs);
    if mean < REORDER_MEAN_SNR_MIN {
        return Err(format!(
            "reorder absorbed mean SNR {mean:.1} dB below the expected floor"
        ));
    }
    Ok(format!(
        "opus_{frame_duration_ms}ms_reorder: absorbed ok ({REORDER_WINDOW_FRAMES} frames; 0 concealed; mean {mean:.1} worst {worst:.1} dB)"
    ))
}

/// Conceals one slot whose frame arrives one step late, decodes the two
/// following frames, and confirms the bounded buffer drops the late arrival.
fn run_late_one_reorder<const N: usize>(
    worker: &mut StreamDecodeWorker<NativeOpusDecoder, JITTER_CAPACITY, MAX_ENCODED_BYTES, N>,
    consumer: &mut RealtimePcmConsumer<N>,
    block: &mut [f32; N],
    source: &ReorderSource<'_>,
    frame_duration_ms: u16,
) -> ProbeResult<String> {
    let history = u32::try_from(HISTORY_FRAMES).expect("history frames fit u32");
    // Frames 11 and 12 arrive before playout reaches frame 10.
    for sequence in [history + 1, history + 2] {
        let index = usize::try_from(sequence).expect("reorder sequence fits usize");
        let outcome = worker
            .admit(
                sequence,
                0,
                &source.packets[index][..source.encoded_lens[index]],
            )
            .map_err(|error| error.to_string())?;
        if !matches!(outcome, InsertOutcome::Inserted | InsertOutcome::Reordered) {
            return Err(format!(
                "reorder case admit {sequence} returned {outcome:?}"
            ));
        }
    }
    // Playout reaches frame 10 before it arrives: one PLC concealment.
    conceal_frame(worker, consumer, block, history, source.expected_samples)?;
    assert_non_silent(block, source.expected_samples, "reorder concealed block")?;
    // The concealed block has no fixed delay; compare model-free.
    let concealed_snr = aligned_snr_vs_source(
        source.tone_block,
        source.expected_samples,
        HISTORY_FRAMES,
        block,
    )?;

    // Frames 11 and 12 decode cleanly behind the concealed slot and must
    // re-lock to the source. Their alignment can differ from the constant
    // delay after PLC, so compare model-free like the burst probe's recovery.
    let mut decoded_snrs = Vec::with_capacity(2);
    for sequence in [history + 1, history + 2] {
        match worker.decode_next().map_err(|error| error.to_string())? {
            DecodeStep::Decoded {
                sequence: actual,
                samples,
                ..
            } if actual == sequence && samples == source.expected_samples => {}
            other => return Err(format!("expected an in-order decode, got {other:?}")),
        }
        assert_popped_pcm(consumer, block, source.expected_samples)?;
        assert_non_silent(block, source.expected_samples, "reorder decoded block")?;
        decoded_snrs.push(aligned_snr_vs_source(
            source.tone_block,
            source.expected_samples,
            usize::try_from(sequence).expect("reorder sequence fits usize"),
            block,
        )?);
    }

    // The late frame is dropped by the bounded buffer.
    let late_index = usize::try_from(history).expect("history fits usize");
    let outcome = worker
        .admit(
            history,
            0,
            &source.packets[late_index][..source.encoded_lens[late_index]],
        )
        .map_err(|error| error.to_string())?;
    if outcome != InsertOutcome::Late {
        return Err(format!(
            "expected the late frame to be dropped, got {outcome:?}"
        ));
    }

    let decoded_mean = mean_snr(&decoded_snrs);
    if decoded_mean < REORDER_MEAN_SNR_MIN {
        return Err(format!(
            "reorder late decoded mean SNR {decoded_mean:.1} dB below the expected floor"
        ));
    }
    Ok(format!(
        "opus_{frame_duration_ms}ms_reorder: late_one ok (1 concealed snr {concealed_snr:.1} dB; late drop 1; decoded mean {decoded_mean:.1} dB)"
    ))
}

/// Runs both deterministic reorder cases for one negotiated shape.
fn run_reorder_probe(frame_duration_ms: u16) -> ProbeResult<Vec<String>> {
    match frame_duration_ms {
        10 => run_reorder_probe_impl::<960>(frame_duration_ms),
        20 => run_reorder_probe_impl::<MAX_PCM_SAMPLES_PER_BLOCK>(frame_duration_ms),
        other => Err(format!("unsupported frame duration {other} ms")),
    }
}

/// Runs both reorder cases with a compile-time PCM block capacity.
fn run_reorder_probe_impl<const PCM_SAMPLES_PER_BLOCK: usize>(
    frame_duration_ms: u16,
) -> ProbeResult<Vec<String>> {
    let frame = OpusFrameConfig::new(SAMPLE_RATE_HZ, 2, frame_duration_ms)
        .map_err(|error| error.to_string())?;
    let expected_samples = frame.interleaved_samples();
    let mut lines = Vec::new();
    for case in [ReorderCase::Absorbed, ReorderCase::LateOne] {
        lines.push(run_reorder_case::<PCM_SAMPLES_PER_BLOCK>(
            frame,
            frame_duration_ms,
            expected_samples,
            case,
        )?);
    }
    Ok(lines)
}

/// Immutable source material shared by the two deterministic jitter-wait runs.
struct JitterWaitSource<'a> {
    /// Pre-encoded packets in source order.
    packets: &'a [[u8; MAX_ENCODED_BYTES]],
    /// Encoded length of each packet.
    encoded_lens: &'a [usize],
    /// Deterministic source tone.
    tone_block: &'a [f32],
    /// Interleaved PCM samples per negotiated frame.
    expected_samples: usize,
    /// Codec lookahead delay in interleaved samples.
    delay: usize,
}

/// Runs one deterministic jitter-wait step through a fresh native decode worker
/// and returns one success line.
///
/// A ten-frame history is always decoded first. In the eight-frame window the
/// first frame is the delayed frame: it arrives `JITTER_WAIT_DELAY_FRAMES`
/// slots after its own slot. A wait of at least that many frames admits it
/// before its playout slot, so it decodes in order with no concealment and no
/// drop; a shorter wait lets its slot conceal by PLC and the bounded buffer
/// drops the late arrival. This records the codec/PLC outcome of a
/// jitter-wait decision only; the scheduling latency of the wait is measured
/// by the network simulator, not by this probe.
fn run_jitter_wait_case<const N: usize>(
    frame: OpusFrameConfig,
    frame_duration_ms: u16,
    expected_samples: usize,
    wait: usize,
) -> ProbeResult<String> {
    let total_frames = HISTORY_FRAMES + JITTER_WAIT_WINDOW_FRAMES;
    let tone_block = tone(total_frames * expected_samples);

    let mut encoder = opus::Encoder::new(
        SAMPLE_RATE_HZ,
        opus::Channels::Stereo,
        opus::Application::Audio,
    )
    .map_err(|error| error.to_string())?;
    let lookahead = usize::try_from(encoder.get_lookahead().map_err(|error| error.to_string())?)
        .expect("lookahead is non-negative");
    let delay = lookahead * OPUS_CHANNELS as usize;

    // Pre-encode the history and jitter-wait window in source order so every
    // packet represents the deterministic tone timeline; admission order below
    // is independent of the encoder's sequential state.
    let mut packets = vec![[0_u8; MAX_ENCODED_BYTES]; total_frames];
    let mut encoded_lens = vec![0_usize; total_frames];
    for index in 0..total_frames {
        let start = index * expected_samples;
        encoded_lens[index] = encode_frame(
            &mut encoder,
            &tone_block[start..start + expected_samples],
            &mut packets[index],
        )?;
    }

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
        let sequence = u32::try_from(index).expect("history frame index fits u32");
        admit_preencoded_frame(
            &mut worker,
            &mut consumer,
            &mut block,
            &packets[index][..encoded_lens[index]],
            sequence,
            expected_samples,
        )?;
    }

    let source = JitterWaitSource {
        packets: &packets,
        encoded_lens: &encoded_lens,
        tone_block: &tone_block,
        expected_samples,
        delay,
    };
    if wait >= JITTER_WAIT_DELAY_FRAMES {
        run_absorbed_jitter_wait(
            &mut worker,
            &mut consumer,
            &mut block,
            &source,
            frame_duration_ms,
            wait,
        )
    } else {
        run_unabsorbed_jitter_wait(
            &mut worker,
            &mut consumer,
            &mut block,
            &source,
            frame_duration_ms,
            wait,
        )
    }
}

/// Admits and decodes one in-order jitter-wait window frame and returns its
/// SNR against the source at the codec's constant delay.
fn decode_in_order_window_frame<const N: usize>(
    worker: &mut StreamDecodeWorker<NativeOpusDecoder, JITTER_CAPACITY, MAX_ENCODED_BYTES, N>,
    consumer: &mut RealtimePcmConsumer<N>,
    block: &mut [f32; N],
    source: &JitterWaitSource<'_>,
    source_index: usize,
) -> ProbeResult<f32> {
    let sequence = u32::try_from(source_index).expect("jitter-wait source index fits u32");
    admit_preencoded_frame(
        worker,
        consumer,
        block,
        &source.packets[source_index][..source.encoded_lens[source_index]],
        sequence,
        source.expected_samples,
    )?;
    assert_non_silent(block, source.expected_samples, "jitter-wait decoded block")?;
    snr_vs_source(
        source.tone_block,
        source.expected_samples,
        source_index,
        source.delay,
        block,
    )
}

/// Plays out the jitter-wait window with the delayed frame admitted before its
/// slot: no concealment, no drop, and every block stays at the constant delay.
fn run_absorbed_jitter_wait<const N: usize>(
    worker: &mut StreamDecodeWorker<NativeOpusDecoder, JITTER_CAPACITY, MAX_ENCODED_BYTES, N>,
    consumer: &mut RealtimePcmConsumer<N>,
    block: &mut [f32; N],
    source: &JitterWaitSource<'_>,
    frame_duration_ms: u16,
    wait: usize,
) -> ProbeResult<String> {
    let history = u32::try_from(HISTORY_FRAMES).expect("history frames fit u32");
    // The wait covered the delay, so the delayed first window frame is
    // admitted before its playout slot and decodes in order.
    let outcome = worker
        .admit(
            history,
            0,
            &source.packets[HISTORY_FRAMES][..source.encoded_lens[HISTORY_FRAMES]],
        )
        .map_err(|error| error.to_string())?;
    if !matches!(outcome, InsertOutcome::Inserted | InsertOutcome::Reordered) {
        return Err(format!(
            "jitter-wait absorbed admit {history} returned {outcome:?}"
        ));
    }
    match worker.decode_next().map_err(|error| error.to_string())? {
        DecodeStep::Decoded {
            sequence: actual,
            samples,
            ..
        } if actual == history && samples == source.expected_samples => {}
        other => return Err(format!("expected an in-order decode, got {other:?}")),
    }
    assert_popped_pcm(consumer, block, source.expected_samples)?;
    assert_non_silent(block, source.expected_samples, "jitter-wait absorbed block")?;
    let mut snrs = vec![snr_vs_source(
        source.tone_block,
        source.expected_samples,
        HISTORY_FRAMES,
        source.delay,
        block,
    )?];
    for source_index in (HISTORY_FRAMES + 1)..(HISTORY_FRAMES + JITTER_WAIT_WINDOW_FRAMES) {
        snrs.push(decode_in_order_window_frame(
            worker,
            consumer,
            block,
            source,
            source_index,
        )?);
    }
    let mean = mean_snr(&snrs);
    if mean < REORDER_MEAN_SNR_MIN {
        return Err(format!(
            "jitter-wait absorbed decoded mean SNR {mean:.1} dB below the expected floor"
        ));
    }
    Ok(format!(
        "opus_{frame_duration_ms}ms_jitter_wait: wait {wait} ok ({JITTER_WAIT_WINDOW_FRAMES} frames; 0 concealed; late drop 0; decoded mean {mean:.1} dB)"
    ))
}

/// Conceals the delayed frame's slot by PLC, decodes the remaining window
/// frames in order, and confirms the bounded buffer drops the late arrival.
fn run_unabsorbed_jitter_wait<const N: usize>(
    worker: &mut StreamDecodeWorker<NativeOpusDecoder, JITTER_CAPACITY, MAX_ENCODED_BYTES, N>,
    consumer: &mut RealtimePcmConsumer<N>,
    block: &mut [f32; N],
    source: &JitterWaitSource<'_>,
    frame_duration_ms: u16,
    wait: usize,
) -> ProbeResult<String> {
    let history = u32::try_from(HISTORY_FRAMES).expect("history frames fit u32");
    match worker.decode_next().map_err(|error| error.to_string())? {
        DecodeStep::Concealed {
            sequence: actual,
            samples,
            ..
        } if actual == history && samples == source.expected_samples => {}
        other => return Err(format!("expected a concealed frame, got {other:?}")),
    }
    assert_popped_pcm(consumer, block, source.expected_samples)?;
    assert_non_silent(
        block,
        source.expected_samples,
        "jitter-wait concealed block",
    )?;
    let concealed_snr = aligned_snr_vs_source(
        source.tone_block,
        source.expected_samples,
        HISTORY_FRAMES,
        block,
    )?;
    let mut snrs = Vec::with_capacity(JITTER_WAIT_WINDOW_FRAMES - 1);
    for source_index in (HISTORY_FRAMES + 1)..(HISTORY_FRAMES + JITTER_WAIT_WINDOW_FRAMES) {
        snrs.push(decode_in_order_window_frame(
            worker,
            consumer,
            block,
            source,
            source_index,
        )?);
    }
    // The delayed frame arrives after its slot passed; the bounded buffer
    // reports the late arrival as dropped.
    let outcome = worker
        .admit(
            history,
            0,
            &source.packets[HISTORY_FRAMES][..source.encoded_lens[HISTORY_FRAMES]],
        )
        .map_err(|error| error.to_string())?;
    if outcome != InsertOutcome::Late {
        return Err(format!(
            "expected the late frame to be dropped, got {outcome:?}"
        ));
    }
    let decoded_mean = mean_snr(&snrs);
    if decoded_mean < LOSS_PATTERN_MEAN_SNR_MIN {
        return Err(format!(
            "jitter-wait decoded mean SNR {decoded_mean:.1} dB below the expected floor"
        ));
    }
    Ok(format!(
        "opus_{frame_duration_ms}ms_jitter_wait: wait {wait} ok ({JITTER_WAIT_WINDOW_FRAMES} frames; 1 concealed snr {concealed_snr:.1} dB; late drop 1; decoded mean {decoded_mean:.1} dB)"
    ))
}

/// Runs every deterministic jitter-wait step for one negotiated shape.
fn run_jitter_wait_probe(frame_duration_ms: u16) -> ProbeResult<Vec<String>> {
    match frame_duration_ms {
        10 => run_jitter_wait_probe_impl::<960>(frame_duration_ms),
        20 => run_jitter_wait_probe_impl::<MAX_PCM_SAMPLES_PER_BLOCK>(frame_duration_ms),
        other => Err(format!("unsupported frame duration {other} ms")),
    }
}

/// Sweeps the deterministic jitter-wait steps with a compile-time PCM block capacity.
fn run_jitter_wait_probe_impl<const PCM_SAMPLES_PER_BLOCK: usize>(
    frame_duration_ms: u16,
) -> ProbeResult<Vec<String>> {
    let frame = OpusFrameConfig::new(SAMPLE_RATE_HZ, 2, frame_duration_ms)
        .map_err(|error| error.to_string())?;
    let expected_samples = frame.interleaved_samples();
    let mut lines = Vec::new();
    for wait in JITTER_WAIT_STEPS {
        lines.push(run_jitter_wait_case::<PCM_SAMPLES_PER_BLOCK>(
            frame,
            frame_duration_ms,
            expected_samples,
            wait,
        )?);
    }
    Ok(lines)
}

/// Times one deterministic encode+decode pass over `frames` source frames and
/// returns the total wall time in microseconds.
///
/// Each timed frame runs the full per-stream path: encode into a reused
/// packet, admit through the bounded jitter buffer, decode outside the realtime
/// callback, and pop the PCM block. The worker and decoder are recreated per
/// pass so every pass starts from a clean negotiated sequence.
fn timed_encode_decode_pass<const N: usize>(
    frame: OpusFrameConfig,
    expected_samples: usize,
    tone_block: &[f32],
    frames: usize,
) -> ProbeResult<f64> {
    let mut encoder = opus::Encoder::new(
        SAMPLE_RATE_HZ,
        opus::Channels::Stereo,
        opus::Application::Audio,
    )
    .map_err(|error| error.to_string())?;
    let (producer, mut consumer) =
        realtime_pcm_queue::<N>(PCM_QUEUE_CAPACITY).map_err(|error| error.to_string())?;
    let decoder = NativeOpusDecoder {
        decoder: opus::Decoder::new(SAMPLE_RATE_HZ, opus::Channels::Stereo)
            .map_err(|error| error.to_string())?,
    };
    let mut worker: StreamDecodeWorker<NativeOpusDecoder, JITTER_CAPACITY, MAX_ENCODED_BYTES, N> =
        StreamDecodeWorker::new_for_opus(decoder, producer, 0, frame)
            .map_err(|error| error.to_string())?;

    let mut packet = [0_u8; MAX_ENCODED_BYTES];
    let mut block = [0.0_f32; N];
    let start = Instant::now();
    for index in 0..frames {
        let source_start = index * expected_samples;
        let len = encode_frame(
            &mut encoder,
            &tone_block[source_start..source_start + expected_samples],
            &mut packet,
        )?;
        worker
            .admit(
                u32::try_from(index).expect("cpu frame index fits u32"),
                0,
                &packet[..len],
            )
            .map_err(|error| error.to_string())?;
        match worker.decode_next().map_err(|error| error.to_string())? {
            DecodeStep::Decoded { samples, .. } if samples == expected_samples => {}
            other => return Err(format!("expected a decoded frame, got {other:?}")),
        }
        match consumer.pop_into(&mut block) {
            RealtimePcmPopOutcome::Filled { samples, .. } if samples == expected_samples => Ok(()),
            other => Err(format!("expected a filled PCM block, got {other:?}")),
        }?;
    }
    let elapsed_us = start.elapsed().as_secs_f64() * 1.0e6;
    // Sanity-check the final block so the compiler cannot treat the loop as
    // producing dead output.
    assert_non_silent(&block, expected_samples, "cpu encode+decode block")?;
    Ok(elapsed_us)
}

/// Times one deterministic decode-only pass over pre-encoded packets and
/// returns the total wall time in microseconds.
///
/// The encode cost is excluded; each timed frame still runs the full per-stream
/// worker path (admit, decode, pop) against packets encoded in source order.
fn timed_decode_only_pass<const N: usize>(
    frame: OpusFrameConfig,
    expected_samples: usize,
    packets: &[[u8; MAX_ENCODED_BYTES]],
    encoded_lens: &[usize],
    frames: usize,
) -> ProbeResult<f64> {
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
    let start = Instant::now();
    for index in 0..frames {
        worker
            .admit(
                u32::try_from(index).expect("cpu frame index fits u32"),
                0,
                &packets[index][..encoded_lens[index]],
            )
            .map_err(|error| error.to_string())?;
        match worker.decode_next().map_err(|error| error.to_string())? {
            DecodeStep::Decoded { samples, .. } if samples == expected_samples => {}
            other => return Err(format!("expected a decoded frame, got {other:?}")),
        }
        match consumer.pop_into(&mut block) {
            RealtimePcmPopOutcome::Filled { samples, .. } if samples == expected_samples => Ok(()),
            other => Err(format!("expected a filled PCM block, got {other:?}")),
        }?;
    }
    let elapsed_us = start.elapsed().as_secs_f64() * 1.0e6;
    assert_non_silent(&block, expected_samples, "cpu decode-only block")?;
    Ok(elapsed_us)
}

/// Measures deterministic per-frame host throughput for one negotiated shape
/// and returns one success line.
///
/// The reported wall time covers the CPU-bound per-frame path and therefore
/// tracks CPU time closely, but it is host- and build-profile-dependent and is
/// not a CPU-utilization, allocation, or power result. Each pass is repeated
/// `CPU_PROBE_REPEATS` times and the minimum wall time is reported to reduce
/// scheduling noise.
fn run_cpu_probe(frame_duration_ms: u16) -> ProbeResult<String> {
    match frame_duration_ms {
        10 => run_cpu_probe_impl::<960>(frame_duration_ms),
        20 => run_cpu_probe_impl::<MAX_PCM_SAMPLES_PER_BLOCK>(frame_duration_ms),
        other => Err(format!("unsupported frame duration {other} ms")),
    }
}

/// Runs the throughput passes with a compile-time PCM block capacity.
#[allow(clippy::cast_precision_loss)]
fn run_cpu_probe_impl<const PCM_SAMPLES_PER_BLOCK: usize>(
    frame_duration_ms: u16,
) -> ProbeResult<String> {
    let frame = OpusFrameConfig::new(SAMPLE_RATE_HZ, 2, frame_duration_ms)
        .map_err(|error| error.to_string())?;
    let expected_samples = frame.interleaved_samples();
    let tone_block = tone(CPU_PROBE_FRAMES * expected_samples);

    // Pre-encode every timed packet in source order for the decode-only pass.
    let mut encoder = opus::Encoder::new(
        SAMPLE_RATE_HZ,
        opus::Channels::Stereo,
        opus::Application::Audio,
    )
    .map_err(|error| error.to_string())?;
    let mut packets = vec![[0_u8; MAX_ENCODED_BYTES]; CPU_PROBE_FRAMES];
    let mut encoded_lens = vec![0_usize; CPU_PROBE_FRAMES];
    for index in 0..CPU_PROBE_FRAMES {
        let start = index * expected_samples;
        encoded_lens[index] = encode_frame(
            &mut encoder,
            &tone_block[start..start + expected_samples],
            &mut packets[index],
        )?;
    }

    // Warm the codec and caches with untimed passes.
    timed_encode_decode_pass::<PCM_SAMPLES_PER_BLOCK>(
        frame,
        expected_samples,
        &tone_block,
        CPU_WARMUP_FRAMES,
    )?;
    timed_decode_only_pass::<PCM_SAMPLES_PER_BLOCK>(
        frame,
        expected_samples,
        &packets,
        &encoded_lens,
        CPU_WARMUP_FRAMES,
    )?;

    let mut encode_decode_us = f64::INFINITY;
    let mut decode_only_us = f64::INFINITY;
    for _ in 0..CPU_PROBE_REPEATS {
        encode_decode_us = encode_decode_us.min(timed_encode_decode_pass::<PCM_SAMPLES_PER_BLOCK>(
            frame,
            expected_samples,
            &tone_block,
            CPU_PROBE_FRAMES,
        )?);
        decode_only_us = decode_only_us.min(timed_decode_only_pass::<PCM_SAMPLES_PER_BLOCK>(
            frame,
            expected_samples,
            &packets,
            &encoded_lens,
            CPU_PROBE_FRAMES,
        )?);
    }

    let encode_decode_per_frame = encode_decode_us / CPU_PROBE_FRAMES as f64;
    let decode_only_per_frame = decode_only_us / CPU_PROBE_FRAMES as f64;
    Ok(format!(
        "opus_{frame_duration_ms}ms_cpu: encode+decode mean {encode_decode_per_frame:.1} us/frame ({encode_decode_rate:.0} frames/s); decode-only mean {decode_only_per_frame:.1} us/frame ({decode_only_rate:.0} frames/s)",
        encode_decode_rate = 1.0e6 / encode_decode_per_frame,
        decode_only_rate = 1.0e6 / decode_only_per_frame,
    ))
}

/// Measures one sustained encode+decode pass over `SUSTAINED_AUDIO_SECONDS`
/// of tone audio and returns one success line.
///
/// Unlike the min-of-three `run_cpu_probe` numbers, this is a single long
/// window (120 s of audio) that reports end-to-end wall time, realtime
/// headroom, and a wall-clock-derived single-core utilization estimate. It
/// is not a measured CPU-time, allocation, or power result.
fn run_sustained_probe(frame_duration_ms: u16) -> ProbeResult<String> {
    match frame_duration_ms {
        10 => run_sustained_probe_impl::<960>(frame_duration_ms),
        20 => run_sustained_probe_impl::<MAX_PCM_SAMPLES_PER_BLOCK>(frame_duration_ms),
        other => Err(format!("unsupported frame duration {other} ms")),
    }
}

/// Runs the sustained passes with a compile-time PCM block capacity.
#[allow(clippy::cast_precision_loss)]
fn run_sustained_probe_impl<const PCM_SAMPLES_PER_BLOCK: usize>(
    frame_duration_ms: u16,
) -> ProbeResult<String> {
    let frame = OpusFrameConfig::new(SAMPLE_RATE_HZ, 2, frame_duration_ms)
        .map_err(|error| error.to_string())?;
    let expected_samples = frame.interleaved_samples();
    let frames = usize::try_from(SUSTAINED_AUDIO_SECONDS * 1_000 / u32::from(frame_duration_ms))
        .expect("sustained frame count fits usize");
    let tone_block = tone(frames * expected_samples);

    let mut elapsed_us = f64::INFINITY;
    for _ in 0..SUSTAINED_PROBE_REPEATS {
        elapsed_us = elapsed_us.min(timed_encode_decode_pass::<PCM_SAMPLES_PER_BLOCK>(
            frame,
            expected_samples,
            &tone_block,
            frames,
        )?);
    }
    Ok(sustained_summary(
        frame_duration_ms,
        SUSTAINED_AUDIO_SECONDS,
        frames,
        elapsed_us,
    ))
}

/// Formats one sustained throughput line for a negotiated shape.
///
/// `elapsed_us` is the minimum wall time over `frames` encode+decode frames
/// covering `audio_seconds` of audio. The reported single-core estimate is
/// the per-frame wall time divided by the frame period and is therefore an
/// upper bound on one core's busy fraction rather than a measured CPU-time
/// trace.
#[must_use]
#[allow(clippy::cast_precision_loss)]
fn sustained_summary(
    frame_duration_ms: u16,
    audio_seconds: u32,
    frames: usize,
    elapsed_us: f64,
) -> String {
    let wall_seconds = elapsed_us / 1.0e6;
    let headroom = f64::from(audio_seconds) / wall_seconds;
    let per_frame_us = elapsed_us / frames as f64;
    let frame_period_us = f64::from(frame_duration_ms) * 1.0e3;
    let single_core_estimate = per_frame_us / frame_period_us * 100.0;
    format!(
        "opus_{frame_duration_ms}ms_sustained: {audio_seconds} s of tone audio in \
         {wall_seconds:.2} s wall ({headroom:.1}x realtime; {per_frame_us:.1} us/frame; \
         ~{single_core_estimate:.1}% of one core estimate)"
    )
}

/// Content profiles compared by the in-band FEC recovery probe.
///
/// The speech-like profile models voiced speech, which libopus's SILK LBRR
/// layer targets; the tone profile is the non-speech reference expected to
/// show no FEC benefit.
#[derive(Clone, Copy)]
enum FecContent {
    /// Voiced speech-like profile.
    Speech,
    /// Single-tone non-speech reference profile.
    Tone,
}

impl FecContent {
    /// Short stable label used in probe output lines.
    #[must_use]
    fn label(self) -> &'static str {
        match self {
            Self::Speech => "speech",
            Self::Tone => "tone",
        }
    }

    /// Builds one deterministic stereo block with `samples` interleaved samples.
    #[must_use]
    fn content(self, samples: usize) -> Vec<f32> {
        match self {
            Self::Speech => speech_like(samples),
            Self::Tone => tone(samples),
        }
    }
}

/// Runs the deterministic in-band FEC recovery probe.
///
/// libopus codes LBRR (low-bitrate redundancy) only when a packet carries
/// more than one frame, so the probe runs at the 20 ms shape only; 10 ms
/// packets cannot carry FEC and must rely on PLC. For each content profile it
/// encodes `FEC_SOURCE_FRAMES` frames with in-band FEC enabled, drops one
/// deterministic frame, and compares the aligned SNR of that frame recovered
/// through in-band FEC against the same frame concealed by decoder-side PLC.
/// The speech-like profile additionally sweeps `FEC_LOSS_PERCENTS` so the
/// recovery benefit is recorded across announced loss rates. This is
/// codec-level evidence, not a `FrameDecoder`-boundary result.
fn run_fec_probe() -> ProbeResult<Vec<String>> {
    let frame =
        OpusFrameConfig::new(SAMPLE_RATE_HZ, 2, FEC_FRAME_MS).map_err(|error| error.to_string())?;
    let expected_samples = frame.interleaved_samples();
    let source_samples = FEC_SOURCE_FRAMES * expected_samples;
    let mut lines = Vec::new();
    for content in [FecContent::Speech, FecContent::Tone] {
        let source = content.content(source_samples);
        let (fec_snr, plc_snr, avg_bytes) =
            measure_fec_case(frame, &source, FEC_PACKET_LOSS_PERCENT)?;
        let gain = fec_snr - plc_snr;
        match content {
            FecContent::Speech if gain < FEC_SPEECH_GAIN_MIN_DB => {
                return Err(format!(
                    "speech FEC gain {gain:.1} dB is below the {FEC_SPEECH_GAIN_MIN_DB:.1} dB gate"
                ));
            }
            FecContent::Tone if gain < -FEC_NON_SPEECH_TOLERANCE_DB => {
                return Err(format!(
                    "non-speech FEC {fec_snr:.1} dB is worse than PLC {plc_snr:.1} dB by {gain:.1} dB"
                ));
            }
            FecContent::Speech | FecContent::Tone => {}
        }
        lines.push(format!(
            "opus_20ms_fec: {label} fec {fec_snr:.1} dB plc {plc_snr:.1} dB gain {gain:+.1} dB \
             (loss {FEC_PACKET_LOSS_PERCENT}% drop {FEC_DROP_FRAME} avg {avg_bytes:.0} B)",
            label = content.label(),
        ));
    }

    // Sweep the configured loss rate on the speech-like profile while the same
    // single frame is dropped, so the sweep isolates how LBRR coding reacts to
    // the announced loss rate rather than to the actual drop.
    let speech = speech_like(source_samples);
    for loss_percent in FEC_LOSS_PERCENTS {
        let (fec_snr, plc_snr, avg_bytes) = measure_fec_case(frame, &speech, loss_percent)?;
        let gain = fec_snr - plc_snr;
        if gain < 0.0 {
            return Err(format!(
                "speech FEC gain {gain:.1} dB is negative at {loss_percent}% loss"
            ));
        }
        lines.push(format!(
            "opus_20ms_fec_loss: speech loss {loss_percent}% fec {fec_snr:.1} dB plc {plc_snr:.1} dB gain {gain:+.1} dB (drop {FEC_DROP_FRAME} avg {avg_bytes:.0} B)"
        ));
    }
    Ok(lines)
}

/// Encodes one content block with in-band FEC enabled at `loss_percent` and
/// measures recovery of the dropped frame through FEC and through PLC.
fn measure_fec_case(
    frame: OpusFrameConfig,
    source: &[f32],
    loss_percent: i32,
) -> ProbeResult<(f32, f32, f64)> {
    let mut encoder = opus::Encoder::new(
        SAMPLE_RATE_HZ,
        opus::Channels::Stereo,
        opus::Application::Audio,
    )
    .map_err(|error| error.to_string())?;
    encoder
        .set_inband_fec(true)
        .map_err(|error| error.to_string())?;
    encoder
        .set_packet_loss_perc(loss_percent)
        .map_err(|error| error.to_string())?;
    let (packets, encoded_lens) = encode_fec_frames(&mut encoder, frame, source)?;
    measure_fec_recovery(&packets, &encoded_lens, frame, source)
}

/// Returns whether packet `k` is treated as lost under the drop stride.
#[must_use]
fn is_dropped(k: usize, drop_every: Option<usize>) -> bool {
    drop_every.is_some_and(|stride| k % stride == 0)
}

/// Encodes `FEC_SOURCE_FRAMES` frames of `source` with in-band FEC enabled.
///
/// The encoder must be configured with `set_inband_fec` and
/// `set_packet_loss_perc` before the first encode so libopus codes LBRR data
/// into each following packet.
fn encode_fec_frames(
    encoder: &mut opus::Encoder,
    frame: OpusFrameConfig,
    source: &[f32],
) -> ProbeResult<(Vec<[u8; MAX_ENCODED_BYTES]>, Vec<usize>)> {
    let expected_samples = frame.interleaved_samples();
    let mut packets = vec![[0_u8; MAX_ENCODED_BYTES]; FEC_SOURCE_FRAMES];
    let mut encoded_lens = vec![0_usize; FEC_SOURCE_FRAMES];
    for index in 0..FEC_SOURCE_FRAMES {
        let start = index * expected_samples;
        encoded_lens[index] = encode_frame(
            encoder,
            &source[start..start + expected_samples],
            &mut packets[index],
        )?;
    }
    Ok((packets, encoded_lens))
}

/// Decodes one dropped frame through in-band FEC recovery and through
/// decoder-side PLC, returning both aligned SNRs and the average packet size.
///
/// The FEC path uses the canonical one-packet-delay flow from `opus_demo.c`:
/// the output of iteration `k` is frame `k - 1`; when packet `k - 1` was lost,
/// frame `k - 1` is decoded from packet `k` with `fec = true`, otherwise
/// packet `k - 1` is decoded normally with `fec = false`. The final source
/// frame is held back by that one-packet delay and is not decoded.
#[allow(clippy::cast_precision_loss)]
fn measure_fec_recovery(
    packets: &[[u8; MAX_ENCODED_BYTES]],
    encoded_lens: &[usize],
    frame: OpusFrameConfig,
    source: &[f32],
) -> ProbeResult<(f32, f32, f64)> {
    if FEC_DROP_FRAME == 0 {
        return Err("FEC drop frame must be at least 1".to_owned());
    }
    let expected_samples = frame.interleaved_samples();
    let per_channel_samples = frame.samples_per_channel();
    let mut block = [0.0_f32; MAX_PCM_SAMPLES_PER_BLOCK];
    let mut decoder = opus::Decoder::new(SAMPLE_RATE_HZ, opus::Channels::Stereo)
        .map_err(|error| error.to_string())?;
    let mut fec_snr = f32::NAN;
    let mut prev_lost = false;
    for k in 0..FEC_SOURCE_FRAMES {
        let lost_here = k == FEC_DROP_FRAME;
        if k >= 1 {
            let (payload, fec) = if prev_lost {
                (&packets[k][..encoded_lens[k]], true)
            } else {
                (&packets[k - 1][..encoded_lens[k - 1]], false)
            };
            let samples = decoder
                .decode_float(payload, &mut block, fec)
                .map_err(|error| error.to_string())?;
            if samples != per_channel_samples {
                return Err(format!(
                    "FEC-path decode produced {samples} samples, expected {per_channel_samples}"
                ));
            }
            if prev_lost {
                let start = (k - 1) * expected_samples;
                let (snr, _) = best_aligned_snr(
                    &source[start..start + expected_samples],
                    &block[..expected_samples],
                );
                if !snr.is_finite() {
                    return Err("FEC-recovered frame SNR is non-finite".to_owned());
                }
                fec_snr = snr;
            }
        }
        prev_lost = lost_here;
    }
    if !fec_snr.is_finite() {
        return Err("FEC probe never decoded the dropped frame".to_owned());
    }

    let mut plc_decoder = opus::Decoder::new(SAMPLE_RATE_HZ, opus::Channels::Stereo)
        .map_err(|error| error.to_string())?;
    let mut plc_snr = f32::NAN;
    for k in 0..FEC_SOURCE_FRAMES {
        let samples = if k == FEC_DROP_FRAME {
            plc_decoder
                .decode_float(&[], &mut block, false)
                .map_err(|error| error.to_string())?
        } else {
            plc_decoder
                .decode_float(&packets[k][..encoded_lens[k]], &mut block, false)
                .map_err(|error| error.to_string())?
        };
        if samples != per_channel_samples {
            return Err(format!(
                "PLC-path decode produced {samples} samples, expected {per_channel_samples}"
            ));
        }
        if k == FEC_DROP_FRAME {
            let start = k * expected_samples;
            let (snr, _) = best_aligned_snr(
                &source[start..start + expected_samples],
                &block[..expected_samples],
            );
            if !snr.is_finite() {
                return Err("PLC-concealed frame SNR is non-finite".to_owned());
            }
            plc_snr = snr;
        }
    }
    if !plc_snr.is_finite() {
        return Err("PLC probe never concealed the dropped frame".to_owned());
    }

    let total_bytes = encoded_lens.iter().copied().sum::<usize>();
    let avg_bytes = total_bytes as f64 / encoded_lens.len() as f64;
    Ok((fec_snr, plc_snr, avg_bytes))
}

/// Times one encode+decode pass over `frames` speech-like frames and returns
/// the total wall time in microseconds.
///
/// When `encoder_fec` is set the encoder enables in-band FEC at
/// `loss_percent`, so LBRR is coded into every packet. When `bitrate` is set
/// the encoder targets that bitrate, so the pass records mode-dependent FEC
/// overhead. When `drop_every` is set, every `drop_every`-th packet is
/// treated as lost and the following packet decodes it with `fec = true`,
/// exercising the FEC recovery path. The pass uses the same direct `opus`
/// calls for every scenario so the three FEC CPU measurements share one code
/// path.
fn timed_fec_pass(
    frame: OpusFrameConfig,
    source: &[f32],
    frames: usize,
    encoder_fec: bool,
    loss_percent: i32,
    bitrate: Option<i32>,
    drop_every: Option<usize>,
) -> ProbeResult<f64> {
    let expected_samples = frame.interleaved_samples();
    let per_channel_samples = frame.samples_per_channel();
    let mut encoder = opus::Encoder::new(
        SAMPLE_RATE_HZ,
        opus::Channels::Stereo,
        opus::Application::Audio,
    )
    .map_err(|error| error.to_string())?;
    if let Some(bitrate) = bitrate {
        encoder
            .set_bitrate(opus::Bitrate::Bits(bitrate))
            .map_err(|error| error.to_string())?;
    }
    if encoder_fec {
        encoder
            .set_inband_fec(true)
            .map_err(|error| error.to_string())?;
        encoder
            .set_packet_loss_perc(loss_percent)
            .map_err(|error| error.to_string())?;
    }
    let mut decoder = opus::Decoder::new(SAMPLE_RATE_HZ, opus::Channels::Stereo)
        .map_err(|error| error.to_string())?;
    let mut packets = vec![[0_u8; MAX_ENCODED_BYTES]; frames];
    let mut encoded_lens = vec![0_usize; frames];
    let mut block = [0.0_f32; MAX_PCM_SAMPLES_PER_BLOCK];
    let start = Instant::now();
    let mut prev_lost = false;
    for k in 0..frames {
        let source_start = k * expected_samples;
        encoded_lens[k] = encode_frame(
            &mut encoder,
            &source[source_start..source_start + expected_samples],
            &mut packets[k],
        )?;
        if k >= 1 {
            let (payload, fec) = if prev_lost {
                (&packets[k][..encoded_lens[k]], true)
            } else {
                (&packets[k - 1][..encoded_lens[k - 1]], false)
            };
            let samples = decoder
                .decode_float(payload, &mut block, fec)
                .map_err(|error| error.to_string())?;
            if samples != per_channel_samples {
                return Err(format!(
                    "FEC CPU decode produced {samples} samples, expected {per_channel_samples}"
                ));
            }
        }
        prev_lost = is_dropped(k, drop_every);
    }
    Ok(start.elapsed().as_secs_f64() * 1.0e6)
}

/// Measures the wall-clock cost of enabling in-band FEC and returns one line
/// per swept bitrate.
///
/// At each bitrate three scenarios share the direct codec-level pass over
/// `CPU_PROBE_FRAMES` speech-like frames at the 20 ms shape: no FEC, FEC with
/// no actual loss (LBRR coded but unused), and FEC with every
/// `FEC_CPU_DROP_EVERY`-th packet recovered by FEC. Each pass is repeated
/// `CPU_PROBE_REPEATS` times and the minimum wall time is reported. The auto
/// bitrate shows the mode-switch cost of forcing hybrid coding; the
/// `FEC_CPU_LOW_BITRATE_BPS` case shows the smaller SILK-mode cost. This is a
/// wall-clock estimate, not a measured CPU-time trace, and is not comparable
/// to the worker-path `run_cpu_probe` numbers.
#[allow(clippy::cast_precision_loss)]
fn run_fec_cpu_probe() -> ProbeResult<Vec<String>> {
    let frame =
        OpusFrameConfig::new(SAMPLE_RATE_HZ, 2, FEC_FRAME_MS).map_err(|error| error.to_string())?;
    let expected_samples = frame.interleaved_samples();
    let source = speech_like(CPU_PROBE_FRAMES * expected_samples);
    let mut lines = Vec::new();
    for (bitrate, label) in [(None, "auto"), (Some(FEC_CPU_LOW_BITRATE_BPS), "24k")] {
        timed_fec_pass(frame, &source, CPU_WARMUP_FRAMES, false, 0, bitrate, None)?;
        timed_fec_pass(
            frame,
            &source,
            CPU_WARMUP_FRAMES,
            true,
            FEC_PACKET_LOSS_PERCENT,
            bitrate,
            None,
        )?;
        timed_fec_pass(
            frame,
            &source,
            CPU_WARMUP_FRAMES,
            true,
            FEC_PACKET_LOSS_PERCENT,
            bitrate,
            Some(FEC_CPU_DROP_EVERY),
        )?;

        let mut baseline_us = f64::INFINITY;
        let mut fec_no_loss_us = f64::INFINITY;
        let mut fec_loss_us = f64::INFINITY;
        for _ in 0..CPU_PROBE_REPEATS {
            baseline_us = baseline_us.min(timed_fec_pass(
                frame,
                &source,
                CPU_PROBE_FRAMES,
                false,
                0,
                bitrate,
                None,
            )?);
            fec_no_loss_us = fec_no_loss_us.min(timed_fec_pass(
                frame,
                &source,
                CPU_PROBE_FRAMES,
                true,
                FEC_PACKET_LOSS_PERCENT,
                bitrate,
                None,
            )?);
            fec_loss_us = fec_loss_us.min(timed_fec_pass(
                frame,
                &source,
                CPU_PROBE_FRAMES,
                true,
                FEC_PACKET_LOSS_PERCENT,
                bitrate,
                Some(FEC_CPU_DROP_EVERY),
            )?);
        }

        let baseline_per_frame = baseline_us / CPU_PROBE_FRAMES as f64;
        let fec_no_loss_overhead = (fec_no_loss_us - baseline_us) / baseline_us * 100.0;
        let fec_loss_overhead = (fec_loss_us - baseline_us) / baseline_us * 100.0;
        lines.push(format!(
            "opus_20ms_fec_cpu: {label} no-fec {baseline_per_frame:.1} us/frame; fec no-loss \
             {fec_no_loss_overhead:+.1}%; fec loss {fec_loss_overhead:+.1}% (speech, min of {CPU_PROBE_REPEATS})",
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
        lines.extend(run_frame_path(duration_ms)?);
        lines.extend(run_burst_sweep(duration_ms)?);
        lines.extend(run_loss_patterns(duration_ms)?);
        lines.extend(run_reorder_probe(duration_ms)?);
        lines.extend(run_jitter_wait_probe(duration_ms)?);
        lines.push(run_cpu_probe(duration_ms)?);
        lines.push(run_sustained_probe(duration_ms)?);
    }

    // In-band FEC recovery is probed once at the 20 ms shape; libopus codes
    // LBRR only when a packet carries more than one frame, so 10 ms cannot.
    // The loss sweep and the FEC CPU pass share that shape.
    lines.extend(run_fec_probe()?);
    lines.extend(run_fec_cpu_probe()?);

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

/// Deterministic content profile used by the quality probes.
///
/// The tone is the original single-sine content; the multi-harmonic profile
/// adds spectral complexity at the same RMS so PLC quality is not tuned to a
/// pure sine alone.
#[derive(Clone, Copy)]
enum ContentProfile {
    /// Single `TONE_HZ` sine at `TONE_AMPLITUDE`.
    Tone,
    /// Fixed five-harmonic sum scaled to the tone profile's RMS.
    MultiHarmonic,
}

impl ContentProfile {
    /// Every profile probed for each negotiated shape.
    const ALL: [ContentProfile; 2] = [ContentProfile::Tone, ContentProfile::MultiHarmonic];

    /// Short stable label used in probe output lines.
    #[must_use]
    fn label(self) -> &'static str {
        match self {
            Self::Tone => "tone",
            Self::MultiHarmonic => "multi",
        }
    }

    /// Builds one deterministic stereo block with `samples` interleaved samples.
    #[must_use]
    fn content(self, samples: usize) -> Vec<f32> {
        match self {
            Self::Tone => tone(samples),
            Self::MultiHarmonic => multi_harmonic(samples),
        }
    }
}

/// Builds one deterministic stereo multi-harmonic block with `samples`
/// interleaved samples, scaled to the tone profile's RMS so quality gates
/// compare content-relative behavior rather than level.
#[must_use]
#[allow(clippy::cast_precision_loss)]
fn multi_harmonic(samples: usize) -> Vec<f32> {
    const HARMONICS: [(f32, f32); 5] = [
        (220.0, 0.15),
        (330.0, 0.12),
        (440.0, 0.10),
        (550.0, 0.08),
        (880.0, 0.05),
    ];
    let raw: Vec<f32> = (0..samples)
        .map(|index| {
            HARMONICS
                .iter()
                .enumerate()
                .fold(0.0_f32, |sum, (harmonic, (frequency, amplitude))| {
                    let phase = 2.0 * std::f32::consts::PI * frequency * index as f32
                        / SAMPLE_RATE_HZ as f32
                        + harmonic as f32 * 0.7;
                    sum + amplitude * phase.sin()
                })
        })
        .collect();
    let rms = block_rms(&raw);
    let scale = TONE_AMPLITUDE / (std::f32::consts::SQRT_2 * rms);
    raw.into_iter().map(|sample| sample * scale).collect()
}

/// Builds one deterministic stereo tone block with `samples` interleaved samples.
#[must_use]
#[allow(clippy::cast_precision_loss)]
fn tone(samples: usize) -> Vec<f32> {
    (0..samples)
        .map(|index| {
            let phase = 2.0 * std::f32::consts::PI * TONE_HZ * index as f32 / SAMPLE_RATE_HZ as f32;
            TONE_AMPLITUDE * phase.sin()
        })
        .collect()
}

/// Builds one deterministic stereo speech-like block with `samples` interleaved
/// samples, scaled to the tone profile's peak amplitude.
///
/// A glottal pulse train at `SPEECH_F0_HZ` with exponential decay is shaped
/// through two bandpass formant resonators and multiplied by a syllabic
/// amplitude modulation. The generator uses no randomness, so every quality
/// line derived from it is reproducible across runs.
#[must_use]
#[allow(clippy::cast_precision_loss)]
fn speech_like(samples: usize) -> Vec<f32> {
    let mut out = vec![0.0_f32; samples];
    let mut pulse_start = 0_usize;
    while pulse_start < samples {
        let mut index = 0_usize;
        while pulse_start + index < samples && index < SPEECH_PULSE_MAX_SAMPLES {
            let time = index as f32;
            out[pulse_start + index] += (-time / SPEECH_PULSE_DECAY_SAMPLES).exp()
                * (1.0 - (-time / SPEECH_PULSE_ATTACK_SAMPLES).exp());
            index += 1;
        }
        pulse_start += SPEECH_PERIOD_SAMPLES;
    }
    for (frequency, radius) in SPEECH_FORMANTS {
        let omega = 2.0 * std::f32::consts::PI * frequency / SAMPLE_RATE_HZ as f32;
        let alpha = omega.sin() * (1.0 - radius) / 2.0;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * omega.cos();
        let a2 = 1.0 - alpha;
        let feedforward1 = -a1;
        let feedforward2 = -a2;
        let mut x1 = 0.0_f32;
        let mut x2 = 0.0_f32;
        let mut y1 = 0.0_f32;
        let mut y2 = 0.0_f32;
        for sample in &mut out {
            let input = *sample;
            let output = (feedforward1 * x1 + feedforward2 * x2 - a1 * y1 - a2 * y2) / a0;
            x2 = x1;
            x1 = input;
            y2 = y1;
            y1 = output;
            *sample = output;
        }
    }
    for (index, sample) in out.iter_mut().enumerate() {
        let time = index as f32 / SAMPLE_RATE_HZ as f32;
        let am = 0.55 + 0.45 * (2.0 * std::f32::consts::PI * SPEECH_AM_RATE_HZ * time).sin();
        *sample *= am;
    }
    let peak = out
        .iter()
        .fold(0.0_f32, |acc, sample| acc.max(sample.abs()));
    let scale = TONE_AMPLITUDE / peak;
    out.into_iter().map(|sample| sample * scale).collect()
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
    use super::{
        TONE_AMPLITUDE, best_aligned_snr, block_rms, is_dropped, snr_db, speech_like,
        sustained_summary,
    };

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

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn sustained_summary_reports_headroom_and_utilization() {
        let line = sustained_summary(10, 120, 12_000, 9.0e6);
        assert!(line.starts_with("opus_10ms_sustained: "));
        assert!(line.contains("120 s of tone audio in 9.00 s wall"));
        assert!(line.contains("13.3x realtime"));
        assert!(line.contains("750.0 us/frame"));
        assert!(line.contains("~7.5% of one core estimate"));
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn sustained_summary_scales_with_frame_duration() {
        let line = sustained_summary(20, 120, 6_000, 10.0e6);
        assert!(line.starts_with("opus_20ms_sustained: "));
        assert!(line.contains("12.0x realtime"));
        assert!(line.contains("1666.7 us/frame"));
        assert!(line.contains("~8.3% of one core estimate"));
    }

    #[test]
    fn speech_like_is_deterministic_and_bounded() {
        let first = speech_like(4 * 960);
        let second = speech_like(4 * 960);
        assert_eq!(first.len(), 4 * 960);
        assert_eq!(first, second);
        assert!(first.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn speech_like_scales_to_tone_peak_with_nonzero_rms() {
        let block = speech_like(960);
        let peak = block
            .iter()
            .fold(0.0_f32, |acc, sample| acc.max(sample.abs()));
        assert!((peak - TONE_AMPLITUDE).abs() < 1.0e-3);
        let rms = block_rms(&block);
        assert!(rms > 0.0 && rms < TONE_AMPLITUDE);
    }

    #[test]
    fn fec_drop_predicate_is_deterministic() {
        assert!(!is_dropped(0, None));
        assert!(!is_dropped(9, None));
        assert!(is_dropped(0, Some(10)));
        assert!(!is_dropped(9, Some(10)));
        assert!(is_dropped(10, Some(10)));
        assert!(!is_dropped(19, Some(10)));
    }
}
