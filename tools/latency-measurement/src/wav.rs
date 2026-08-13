//! Minimal RIFF/WAVE reading and writing for measurement recordings.
//!
//! Only the subset needed by the latency tool is supported: PCM 16/24/32-bit and
//! IEEE-float 32/64-bit mono or multi-channel WAVE files, including their
//! WAVE_FORMAT_EXTENSIBLE variants. Multi-channel input is down-mixed to mono
//! by averaging frames.

use std::fmt;
use std::path::Path;

/// Failure while parsing or writing a WAVE file.
#[derive(Debug)]
pub enum WavError {
    /// The underlying file operation failed.
    Io(std::io::Error),
    /// The file does not begin with the RIFF marker.
    NotRiff,
    /// The RIFF container is not a WAVE file.
    NotWave,
    /// A required format or data chunk is missing.
    MissingChunk,
    /// The file ended before a declared header or chunk was complete.
    Truncated,
    /// The format or bit depth is not supported.
    UnsupportedFormat {
        /// WAVE format code.
        format: u16,
        /// Bits per sample declared by the format chunk.
        bits: u16,
    },
    /// The format chunk declares zero channels.
    ZeroChannels,
    /// The format chunk declares a zero sample rate.
    ZeroSampleRate,
    /// The file contains no audio frames.
    Empty,
}

impl fmt::Display for WavError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "wave file I/O failed: {error}"),
            Self::NotRiff => formatter.write_str("file is not a RIFF container"),
            Self::NotWave => formatter.write_str("RIFF container is not WAVE"),
            Self::MissingChunk => {
                formatter.write_str("wave file is missing a format or data chunk")
            }
            Self::Truncated => formatter.write_str("wave file is truncated"),
            Self::UnsupportedFormat { format, bits } => {
                write!(
                    formatter,
                    "unsupported wave format {format} with {bits} bits per sample"
                )
            }
            Self::ZeroChannels => formatter.write_str("wave format declares zero channels"),
            Self::ZeroSampleRate => formatter.write_str("wave format declares a zero sample rate"),
            Self::Empty => formatter.write_str("wave file contains no audio frames"),
        }
    }
}

impl std::error::Error for WavError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::NotRiff
            | Self::NotWave
            | Self::MissingChunk
            | Self::Truncated
            | Self::UnsupportedFormat { .. }
            | Self::ZeroChannels
            | Self::ZeroSampleRate
            | Self::Empty => None,
        }
    }
}

/// Decoded mono PCM samples plus the source sample rate.
#[derive(Clone, Debug, PartialEq)]
pub struct MonoWav {
    /// Sample rate declared by the WAVE format chunk.
    pub sample_rate_hz: u32,
    /// Mono samples normalized to the approximate `[-1.0, 1.0]` range.
    pub samples: Vec<f32>,
}

/// Reads a WAVE file and returns mono samples.
///
/// # Errors
///
/// Returns [`WavError`] when the file is not a supported WAVE file or the
/// operating system cannot read it.
pub fn read_mono(path: &Path) -> Result<MonoWav, WavError> {
    let bytes = std::fs::read(path).map_err(WavError::Io)?;
    parse_mono(&bytes)
}

/// Reads a WAVE file and reports its declared properties.
///
/// # Errors
///
/// Returns [`WavError`] when the file is not a supported WAVE file or the
/// operating system cannot read it.
pub fn read_info(path: &Path) -> Result<WavInfo, WavError> {
    let bytes = std::fs::read(path).map_err(WavError::Io)?;
    inspect_mono(&bytes)
}

/// Sample layout used when encoding WAVE output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SampleFormat {
    /// 16-bit signed PCM (`WAVE_FORMAT_PCM`).
    Pcm16,
    /// 24-bit signed PCM packed into three little-endian bytes.
    Pcm24,
    /// 32-bit signed PCM.
    Pcm32,
    /// 32-bit IEEE float (`WAVE_FORMAT_IEEE_FLOAT`).
    Float32,
    /// 64-bit IEEE float.
    Float64,
}

impl SampleFormat {
    /// WAVE format tag for this layout.
    const fn audio_format(self) -> u16 {
        match self {
            Self::Pcm16 | Self::Pcm24 | Self::Pcm32 => 1,
            Self::Float32 | Self::Float64 => 3,
        }
    }

    /// Bits per sample declared in the format chunk.
    const fn bits_per_sample(self) -> u16 {
        match self {
            Self::Pcm16 => 16,
            Self::Pcm24 => 24,
            Self::Pcm32 | Self::Float32 => 32,
            Self::Float64 => 64,
        }
    }

    /// Bytes occupied by one mono sample in the data chunk.
    const fn bytes_per_sample(self) -> usize {
        match self {
            Self::Pcm16 => 2,
            Self::Pcm24 => 3,
            Self::Pcm32 | Self::Float32 => 4,
            Self::Float64 => 8,
        }
    }

    /// CLI token accepted by `generate --bits`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Pcm16 => "16",
            Self::Pcm24 => "24",
            Self::Pcm32 => "32",
            Self::Float32 => "f32",
            Self::Float64 => "f64",
        }
    }

    /// Parses a `generate --bits` token.
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        match token {
            "16" => Some(Self::Pcm16),
            "24" => Some(Self::Pcm24),
            "32" => Some(Self::Pcm32),
            "f32" => Some(Self::Float32),
            "f64" => Some(Self::Float64),
            _ => None,
        }
    }
}

/// Writes mono samples as a WAVE file in the requested format.
///
/// # Errors
///
/// Returns [`WavError`] when the file cannot be written.
pub fn write_mono(
    path: &Path,
    sample_rate_hz: u32,
    samples: &[f32],
    format: SampleFormat,
) -> Result<(), WavError> {
    let bytes = encode_mono(sample_rate_hz, samples, format);
    std::fs::write(path, bytes).map_err(WavError::Io)
}

/// Writes mono samples as a 16-bit PCM WAVE file.
///
/// # Errors
///
/// Returns [`WavError`] when the file cannot be written.
pub fn write_mono_16bit(path: &Path, sample_rate_hz: u32, samples: &[f32]) -> Result<(), WavError> {
    write_mono(path, sample_rate_hz, samples, SampleFormat::Pcm16)
}

/// Parses the RIFF container and returns the validated format plus the raw
/// data chunk payload.
fn parse_wave(bytes: &[u8]) -> Result<(FormatInfo, &[u8]), WavError> {
    if bytes.len() < 12 {
        return Err(WavError::Truncated);
    }
    if &bytes[0..4] != b"RIFF" {
        return Err(WavError::NotRiff);
    }
    if &bytes[8..12] != b"WAVE" {
        return Err(WavError::NotWave);
    }

    let mut format = None;
    let mut data = None;
    let mut offset = 12_usize;
    while offset + 8 <= bytes.len() {
        let chunk_size = u32::from_le_bytes(
            bytes[offset + 4..offset + 8]
                .try_into()
                .expect("slice length is fixed at four"),
        ) as usize;
        let start = offset + 8;
        let end = start.checked_add(chunk_size).ok_or(WavError::Truncated)?;
        if end > bytes.len() {
            return Err(WavError::Truncated);
        }
        match &bytes[offset..offset + 4] {
            b"fmt " => format = Some(parse_format_chunk(&bytes[start..end])?),
            b"data" => data = Some(&bytes[start..end]),
            _ => {}
        }
        offset = end + (chunk_size & 1);
    }

    Ok((
        format.ok_or(WavError::MissingChunk)?,
        data.ok_or(WavError::MissingChunk)?,
    ))
}

/// Declared properties of a supported WAVE file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WavInfo {
    /// Sample rate declared by the format chunk.
    pub sample_rate_hz: u32,
    /// Interleaved channel count.
    pub channels: u16,
    /// Bits per sample declared by the format chunk.
    pub bits_per_sample: u16,
    /// Resolved WAVE format tag (`1` = PCM, `3` = IEEE float).
    pub audio_format: u16,
    /// Sample frames in the data chunk; one frame spans all channels.
    pub frames: usize,
}

impl WavInfo {
    /// Duration of the data chunk in whole milliseconds, or `None` when the
    /// declared sample rate is zero.
    #[must_use]
    pub fn duration_ms(self) -> Option<u64> {
        if self.sample_rate_hz == 0 {
            return None;
        }
        Some((self.frames as u64).saturating_mul(1_000) / u64::from(self.sample_rate_hz))
    }
}

/// Reports the declared properties of supported WAVE bytes without decoding
/// their samples.
///
/// # Errors
///
/// Returns [`WavError`] for the same failures as [`parse_mono`].
///
/// # Panics
///
/// Panics only on an internal fixed-slice conversion that the length checks in
/// this function prevent from failing.
pub fn inspect_mono(bytes: &[u8]) -> Result<WavInfo, WavError> {
    let (format, data) = parse_wave(bytes)?;
    let frame_bytes = usize::from(format.channels) * bytes_per_sample(&format)?;
    let frames = data.len() / frame_bytes;
    if frames == 0 {
        return Err(WavError::Empty);
    }
    Ok(WavInfo {
        sample_rate_hz: format.sample_rate_hz,
        channels: format.channels,
        bits_per_sample: format.bits_per_sample,
        audio_format: format.audio_format,
        frames,
    })
}

/// Parses WAVE bytes and returns mono samples without touching the filesystem.
///
/// # Errors
///
/// Returns [`WavError`] when the bytes are not a supported WAVE file.
///
/// # Panics
///
/// Panics only on an internal fixed-slice conversion that the length checks in
/// this function prevent from failing.
pub fn parse_mono(bytes: &[u8]) -> Result<MonoWav, WavError> {
    let (format, data) = parse_wave(bytes)?;
    let samples = decode_frames(&format, data)?;
    if samples.is_empty() {
        return Err(WavError::Empty);
    }
    Ok(MonoWav {
        sample_rate_hz: format.sample_rate_hz,
        samples,
    })
}

/// Encodes mono samples as WAVE bytes in the requested format.
///
/// # Panics
///
/// Panics when the encoded file is larger than `u32::MAX` bytes.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn encode_mono(sample_rate_hz: u32, samples: &[f32], format: SampleFormat) -> Vec<u8> {
    let bytes_per_sample = format.bytes_per_sample();
    let data_bytes = samples.len() * bytes_per_sample;
    let mut bytes = Vec::with_capacity(44 + data_bytes);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(
        &u32::try_from(36 + data_bytes)
            .expect("file size fits u32")
            .to_le_bytes(),
    );
    bytes.extend_from_slice(b"WAVE");
    bytes.extend_from_slice(b"fmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&format.audio_format().to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&sample_rate_hz.to_le_bytes());
    let block_align = u16::try_from(bytes_per_sample).expect("block align fits u16");
    bytes.extend_from_slice(
        &(sample_rate_hz * u32::from(block_align)).to_le_bytes(), // byte rate
    );
    bytes.extend_from_slice(&block_align.to_le_bytes());
    bytes.extend_from_slice(&format.bits_per_sample().to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(
        &u32::try_from(data_bytes)
            .expect("data size fits u32")
            .to_le_bytes(),
    );
    for &sample in samples {
        encode_sample(sample, format, &mut bytes);
    }
    bytes
}

/// Encodes mono samples as 16-bit PCM WAVE bytes.
///
/// # Panics
///
/// Panics when the encoded file is larger than `u32::MAX` bytes.
#[must_use]
pub fn encode_mono_16bit(sample_rate_hz: u32, samples: &[f32]) -> Vec<u8> {
    encode_mono(sample_rate_hz, samples, SampleFormat::Pcm16)
}

/// Appends one mono sample in `format` to the WAVE data payload.
#[allow(clippy::cast_possible_truncation)]
fn encode_sample(sample: f32, format: SampleFormat, out: &mut Vec<u8>) {
    match format {
        SampleFormat::Pcm16 => {
            let scaled = (f64::from(sample) * 32_767.0)
                .round()
                .clamp(-32_768.0, 32_767.0);
            out.extend_from_slice(&(scaled as i16).to_le_bytes());
        }
        SampleFormat::Pcm24 => {
            let scaled = (f64::from(sample) * 8_388_607.0)
                .round()
                .clamp(-8_388_608.0, 8_388_607.0) as i32;
            // Little-endian three-byte two's-complement representation.
            out.extend_from_slice(&scaled.to_le_bytes()[..3]);
        }
        SampleFormat::Pcm32 => {
            let scaled = (f64::from(sample) * 2_147_483_647.0)
                .round()
                .clamp(-2_147_483_648.0, 2_147_483_647.0) as i32;
            out.extend_from_slice(&scaled.to_le_bytes());
        }
        SampleFormat::Float32 => out.extend_from_slice(&sample.to_le_bytes()),
        SampleFormat::Float64 => out.extend_from_slice(&f64::from(sample).to_le_bytes()),
    }
}

struct FormatInfo {
    audio_format: u16,
    channels: u16,
    sample_rate_hz: u32,
    bits_per_sample: u16,
}

fn parse_format_chunk(chunk: &[u8]) -> Result<FormatInfo, WavError> {
    if chunk.len() < 16 {
        return Err(WavError::Truncated);
    }
    let mut audio_format = u16::from_le_bytes(chunk[0..2].try_into().expect("fixed slice"));
    let channels = u16::from_le_bytes(chunk[2..4].try_into().expect("fixed slice"));
    let sample_rate_hz = u32::from_le_bytes(chunk[4..8].try_into().expect("fixed slice"));
    let bits_per_sample = u16::from_le_bytes(chunk[14..16].try_into().expect("fixed slice"));
    if channels == 0 {
        return Err(WavError::ZeroChannels);
    }
    if sample_rate_hz == 0 {
        return Err(WavError::ZeroSampleRate);
    }
    // WAVE_FORMAT_EXTENSIBLE wraps the real format code in a subformat GUID.
    if audio_format == 0xFFFE {
        if chunk.len() < 40 {
            return Err(WavError::Truncated);
        }
        audio_format = u16::from_le_bytes(chunk[24..26].try_into().expect("fixed slice"));
    }
    Ok(FormatInfo {
        audio_format,
        channels,
        sample_rate_hz,
        bits_per_sample,
    })
}

fn bytes_per_sample(format: &FormatInfo) -> Result<usize, WavError> {
    match (format.audio_format, format.bits_per_sample) {
        (1, 16) => Ok(2_usize),
        (1, 24) => Ok(3_usize),
        (1 | 3, 32) => Ok(4_usize),
        (3, 64) => Ok(8_usize),
        (format, bits) => Err(WavError::UnsupportedFormat { format, bits }),
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn decode_frames(format: &FormatInfo, data: &[u8]) -> Result<Vec<f32>, WavError> {
    let bytes_per_sample = bytes_per_sample(format)?;
    let channels = usize::from(format.channels);
    let frame_bytes = channels * bytes_per_sample;
    if frame_bytes == 0 {
        return Err(WavError::ZeroChannels);
    }
    let frame_count = data.len() / frame_bytes;
    let mut mono = Vec::with_capacity(frame_count);
    for frame in 0..frame_count {
        let base = frame * frame_bytes;
        let mut sum = 0.0_f64;
        for channel in 0..channels {
            let sample_offset = base + channel * bytes_per_sample;
            let value = match (format.audio_format, format.bits_per_sample) {
                (1, 16) => {
                    let raw = i16::from_le_bytes(
                        data[sample_offset..sample_offset + 2]
                            .try_into()
                            .expect("fixed slice"),
                    );
                    f64::from(raw) / 32_768.0
                }
                (1, 24) => {
                    let raw = i32::from_le_bytes([
                        data[sample_offset],
                        data[sample_offset + 1],
                        data[sample_offset + 2],
                        0,
                    ]);
                    // Sign-extend the 24-bit sample: an arithmetic shift of the
                    // left-aligned value restores the sign bits above bit 23.
                    f64::from((raw << 8) >> 8) / 8_388_608.0
                }
                (1, 32) => {
                    let raw = i32::from_le_bytes(
                        data[sample_offset..sample_offset + 4]
                            .try_into()
                            .expect("fixed slice"),
                    );
                    f64::from(raw) / 2_147_483_648.0
                }
                (3, 32) => {
                    let raw = f32::from_le_bytes(
                        data[sample_offset..sample_offset + 4]
                            .try_into()
                            .expect("fixed slice"),
                    );
                    f64::from(raw)
                }
                (3, 64) => f64::from_le_bytes(
                    data[sample_offset..sample_offset + 8]
                        .try_into()
                        .expect("fixed slice"),
                ),
                _ => unreachable!("format was validated before frame decode"),
            };
            sum += value;
        }
        mono.push((sum / channels as f64) as f32);
    }
    Ok(mono)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixteen_bit_mono_round_trips_through_encode_and_parse() {
        let samples = [0.0, 0.5, -0.5, 1.0, -1.0, 0.25];
        let bytes = encode_mono_16bit(48_000, &samples);
        let parsed = parse_mono(&bytes).expect("encoded wave parses");
        assert_eq!(parsed.sample_rate_hz, 48_000);
        assert_eq!(parsed.samples.len(), samples.len());
        for (actual, expected) in parsed.samples.iter().zip(samples) {
            assert!((actual - expected).abs() < 2.0 / 32_768.0);
        }
    }

    #[test]
    fn every_format_round_trips_through_encode_and_parse() {
        let samples = [-1.0, -0.5, 0.0, 0.25, 0.5, 1.0];
        let tolerances = [
            (SampleFormat::Pcm16, 2.0 / 32_768.0),
            (SampleFormat::Pcm24, 2.0 / 8_388_608.0),
            (SampleFormat::Pcm32, 2.0 / 2_147_483_648.0),
            (SampleFormat::Float32, 1e-6),
            (SampleFormat::Float64, 1e-6),
        ];
        for (format, tolerance) in tolerances {
            let bytes = encode_mono(48_000, &samples, format);
            let parsed = parse_mono(&bytes).expect("encoded wave parses");
            assert_eq!(parsed.sample_rate_hz, 48_000, "{format:?}");
            assert_eq!(parsed.samples.len(), samples.len(), "{format:?}");
            for (actual, expected) in parsed.samples.iter().zip(samples) {
                assert!(
                    (*actual - expected).abs() < tolerance,
                    "{format:?} expected {expected}, observed {actual}"
                );
            }
        }
    }

    #[test]
    fn pcm_encoders_clamp_out_of_range_samples() {
        for format in [
            SampleFormat::Pcm16,
            SampleFormat::Pcm24,
            SampleFormat::Pcm32,
        ] {
            let bytes = encode_mono(48_000, &[2.0, -2.0], format);
            let parsed = parse_mono(&bytes).expect("encoded wave parses");
            assert!((parsed.samples[0] - 1.0).abs() < 1e-3, "{format:?}");
            assert!((parsed.samples[1] + 1.0).abs() < 1e-3, "{format:?}");
        }
    }

    #[test]
    fn encoded_headers_declare_matching_sizes() {
        let samples = [0.25_f32; 100];
        for format in [
            SampleFormat::Pcm16,
            SampleFormat::Pcm24,
            SampleFormat::Pcm32,
            SampleFormat::Float32,
            SampleFormat::Float64,
        ] {
            let bytes = encode_mono(48_000, &samples, format);
            let data_bytes = samples.len() * format.bytes_per_sample();
            assert_eq!(
                u32::from_le_bytes(bytes[4..8].try_into().expect("fixed slice")) as usize,
                36 + data_bytes,
                "{format:?} RIFF size"
            );
            assert_eq!(
                u32::from_le_bytes(bytes[40..44].try_into().expect("fixed slice")) as usize,
                data_bytes,
                "{format:?} data size"
            );
        }
    }

    #[test]
    fn inspect_mono_reports_declared_properties() {
        let frames = 48_000_usize;
        let data = vec![0_u8; frames * 3];
        let bytes = build_wave(48_000, 1, 24, &data);
        let info = inspect_mono(&bytes).expect("wave inspects");
        assert_eq!(
            info,
            WavInfo {
                sample_rate_hz: 48_000,
                channels: 1,
                bits_per_sample: 24,
                audio_format: 1,
                frames,
            }
        );
        assert_eq!(info.duration_ms(), Some(1_000));
    }

    #[test]
    fn inspect_mono_resolves_extensible_audio_format() {
        // WAVE_FORMAT_EXTENSIBLE wrapping IEEE float resolves to tag 3.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(4_u32 + 48 + 12).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&40_u32.to_le_bytes());
        bytes.extend_from_slice(&0xFFFE_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&(48_000_u32 * 4).to_le_bytes());
        bytes.extend_from_slice(&4_u16.to_le_bytes());
        bytes.extend_from_slice(&32_u16.to_le_bytes());
        bytes.extend_from_slice(&22_u16.to_le_bytes()); // cbSize
        bytes.extend_from_slice(&32_u16.to_le_bytes()); // valid bits
        bytes.extend_from_slice(&0_u32.to_le_bytes()); // channel mask
        // Subformat GUID {00000003-0000-0010-8000-00AA00389B71} (IEEE float).
        bytes.extend_from_slice(&0x0000_0003_u32.to_le_bytes());
        bytes.extend_from_slice(&0x0000_u16.to_le_bytes());
        bytes.extend_from_slice(&0x0010_u16.to_le_bytes());
        bytes.extend_from_slice(&[0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71]);
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&4_u32.to_le_bytes());
        bytes.extend_from_slice(&0.5_f32.to_le_bytes());

        let info = inspect_mono(&bytes).expect("extensible wave inspects");
        assert_eq!(info.audio_format, 3);
        assert_eq!(info.bits_per_sample, 32);
        assert_eq!(info.channels, 1);
        assert_eq!(info.frames, 1);
    }

    #[test]
    fn inspect_mono_rejects_unsupported_and_empty_waves() {
        let bytes = build_wave(48_000, 1, 8, &[0x80]);
        assert!(matches!(
            inspect_mono(&bytes),
            Err(WavError::UnsupportedFormat { format: 1, bits: 8 })
        ));

        let empty = encode_mono(48_000, &[], SampleFormat::Pcm16);
        assert!(matches!(inspect_mono(&empty), Err(WavError::Empty)));
    }

    #[test]
    fn readers_reject_zero_sample_rate() {
        let bytes = build_wave(0, 1, 16, &[0x00, 0x00]);
        assert!(matches!(
            inspect_mono(&bytes),
            Err(WavError::ZeroSampleRate)
        ));
        assert!(matches!(parse_mono(&bytes), Err(WavError::ZeroSampleRate)));
        assert_eq!(
            WavError::ZeroSampleRate.to_string(),
            "wave format declares a zero sample rate"
        );
    }

    #[test]
    fn stereo_frames_are_averaged_to_mono() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36_u32 + 4 * 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes()); // two channels
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&(48_000_u32 * 4).to_le_bytes());
        bytes.extend_from_slice(&4_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&8_u32.to_le_bytes());
        bytes.extend_from_slice(&0_i16.to_le_bytes()); // left
        bytes.extend_from_slice(&2_048_i16.to_le_bytes()); // right
        bytes.extend_from_slice(&(-2_048_i16).to_le_bytes()); // left
        bytes.extend_from_slice(&2_048_i16.to_le_bytes()); // right

        let parsed = parse_mono(&bytes).expect("stereo wave parses");
        assert_eq!(parsed.samples.len(), 2);
        assert!((parsed.samples[0] - 1_024.0 / 32_768.0).abs() < 1e-6);
        assert!(parsed.samples[1].abs() < 1e-6);
    }

    #[test]
    fn rejects_non_wave_bytes_and_truncation() {
        assert!(matches!(
            parse_mono(b"NOT A RIFF FILE HERE"),
            Err(WavError::NotRiff)
        ));
        assert!(matches!(parse_mono(b"RIFF"), Err(WavError::Truncated)));
    }

    #[test]
    fn extensible_float32_is_decoded_to_mono() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(4_u32 + 48 + 12).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&40_u32.to_le_bytes());
        bytes.extend_from_slice(&0xFFFE_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&(48_000_u32 * 4).to_le_bytes());
        bytes.extend_from_slice(&4_u16.to_le_bytes());
        bytes.extend_from_slice(&32_u16.to_le_bytes());
        bytes.extend_from_slice(&22_u16.to_le_bytes()); // cbSize
        bytes.extend_from_slice(&32_u16.to_le_bytes()); // valid bits
        bytes.extend_from_slice(&0_u32.to_le_bytes()); // channel mask
        // Subformat GUID {00000003-0000-0010-8000-00AA00389B71} (IEEE float).
        bytes.extend_from_slice(&0x0000_0003_u32.to_le_bytes());
        bytes.extend_from_slice(&0x0000_u16.to_le_bytes());
        bytes.extend_from_slice(&0x0010_u16.to_le_bytes());
        bytes.extend_from_slice(&[0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71]);
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&4_u32.to_le_bytes());
        bytes.extend_from_slice(&0.5_f32.to_le_bytes());

        let parsed = parse_mono(&bytes).expect("extensible float wave parses");
        assert_eq!(parsed.sample_rate_hz, 48_000);
        assert_eq!(parsed.samples.len(), 1);
        assert!((parsed.samples[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn empty_data_chunk_is_rejected() {
        let mut bytes = encode_mono_16bit(8_000, &[0.5]);
        // Rewrite the data chunk to declare zero bytes and clear the payload.
        let data_offset = 40;
        bytes[data_offset..data_offset + 4].copy_from_slice(&0_u32.to_le_bytes());
        bytes.truncate(data_offset + 4);
        bytes[4..8].copy_from_slice(&36_u32.to_le_bytes());
        assert!(matches!(parse_mono(&bytes), Err(WavError::Empty)));
    }

    #[test]
    fn twenty_four_bit_mono_decodes_with_sign_extension() {
        let data: &[u8] = &[
            0xFF, 0xFF, 0x7F, // +8_388_607 -> ~+1.0
            0x00, 0x00, 0x80, // -8_388_608 -> -1.0
            0x00, 0x00, 0x00, // 0 -> 0.0
            0x00, 0x00, 0x40, // +4_194_304 -> +0.5
            0x00, 0x00, 0xC0, // -4_194_304 -> -0.5
        ];
        let bytes = build_wave(48_000, 1, 24, data);
        let parsed = parse_mono(&bytes).expect("24-bit wave parses");
        assert_eq!(parsed.sample_rate_hz, 48_000);
        let expected = [8_388_607.0 / 8_388_608.0, -1.0, 0.0, 0.5, -0.5];
        assert_eq!(parsed.samples.len(), expected.len());
        for (actual, expected) in parsed.samples.iter().zip(expected) {
            assert!((f64::from(*actual) - expected).abs() < 1e-6);
        }
    }

    #[test]
    fn twenty_four_bit_stereo_averages_to_mono() {
        let data: &[u8] = &[
            0x00, 0x00, 0x40, // left +0.5
            0x00, 0x00, 0x00, // right 0.0
            0x00, 0x00, 0xC0, // left -0.5
            0x00, 0x00, 0x40, // right +0.5
        ];
        let bytes = build_wave(48_000, 2, 24, data);
        let parsed = parse_mono(&bytes).expect("24-bit stereo wave parses");
        assert_eq!(parsed.samples.len(), 2);
        assert!((f64::from(parsed.samples[0]) - 0.25).abs() < 1e-6);
        assert!(f64::from(parsed.samples[1]).abs() < 1e-6);
    }

    #[test]
    fn twenty_four_bit_extensible_pcm_is_decoded() {
        let data: &[u8] = &[0x00, 0x00, 0x60]; // +6_291_456 -> +0.75
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(4_u32 + 48 + 12).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&40_u32.to_le_bytes());
        bytes.extend_from_slice(&0xFFFE_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&(48_000_u32 * 3).to_le_bytes());
        bytes.extend_from_slice(&3_u16.to_le_bytes());
        bytes.extend_from_slice(&24_u16.to_le_bytes());
        bytes.extend_from_slice(&22_u16.to_le_bytes()); // cbSize
        bytes.extend_from_slice(&24_u16.to_le_bytes()); // valid bits
        bytes.extend_from_slice(&0_u32.to_le_bytes()); // channel mask
        // Subformat GUID {00000001-0000-0010-8000-00AA00389B71} (PCM).
        bytes.extend_from_slice(&0x0000_0001_u32.to_le_bytes());
        bytes.extend_from_slice(&0x0000_u16.to_le_bytes());
        bytes.extend_from_slice(&0x0010_u16.to_le_bytes());
        bytes.extend_from_slice(&[0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71]);
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&3_u32.to_le_bytes());
        bytes.extend_from_slice(data);

        let parsed = parse_mono(&bytes).expect("24-bit extensible wave parses");
        assert_eq!(parsed.sample_rate_hz, 48_000);
        assert_eq!(parsed.samples.len(), 1);
        assert!((f64::from(parsed.samples[0]) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn rejects_unsupported_bit_depths() {
        let bytes = build_wave(48_000, 1, 8, &[0x80]);
        assert!(matches!(
            parse_mono(&bytes),
            Err(WavError::UnsupportedFormat { format: 1, bits: 8 })
        ));
    }

    fn build_wave(
        sample_rate_hz: u32,
        channels: u16,
        bits_per_sample: u16,
        data: &[u8],
    ) -> Vec<u8> {
        let block_align = channels * (bits_per_sample / 8);
        let byte_rate = sample_rate_hz * u32::from(block_align);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(
            &u32::try_from(36 + data.len())
                .expect("test wave fits u32")
                .to_le_bytes(),
        );
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&sample_rate_hz.to_le_bytes());
        bytes.extend_from_slice(&byte_rate.to_le_bytes());
        bytes.extend_from_slice(&block_align.to_le_bytes());
        bytes.extend_from_slice(&bits_per_sample.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(
            &u32::try_from(data.len())
                .expect("test wave fits u32")
                .to_le_bytes(),
        );
        bytes.extend_from_slice(data);
        bytes
    }
}
