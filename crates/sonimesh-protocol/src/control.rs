use core::fmt;

use prost::Message;

use crate::{ControlError, Envelope, Hello, StartStream, StopStream, StreamStatus, envelope};

/// Exact protocol version accepted by this controlled-lab implementation.
pub const CONTROL_PROTOCOL_VERSION: u32 = 1;
/// Largest length-prefixed Protobuf control payload.
pub const MAX_CONTROL_FRAME_BYTES: usize = 64 * 1024;
/// Maximum advertised capabilities in one hello message.
pub const MAX_CONTROL_CAPABILITIES: usize = 16;
/// Maximum advertised protocol versions in one hello message.
pub const MAX_CONTROL_SUPPORTED_VERSIONS: usize = 8;
/// Maximum UTF-8 byte length for a peer display name.
pub const MAX_CONTROL_DISPLAY_NAME_BYTES: usize = 128;
/// Maximum UTF-8 byte length for status/error detail.
pub const MAX_CONTROL_DETAIL_BYTES: usize = 512;
/// Lowest sample rate permitted in a stream-start control command.
pub const MIN_CONTROL_SAMPLE_RATE_HZ: u32 = 8_000;
/// Highest sample rate permitted in a stream-start control command.
pub const MAX_CONTROL_SAMPLE_RATE_HZ: u32 = 192_000;
/// Highest channel count permitted in a stream-start control command.
pub const MAX_CONTROL_CHANNELS: u32 = 8;
/// Largest wire epoch representable by the version-one media key context.
pub const MAX_CONTROL_MEDIA_KEY_EPOCH: u32 = u16::MAX as u32;

/// Failure while framing, decoding, or validating a control envelope.
#[derive(Debug)]
pub enum ControlFrameError {
    /// Input cannot contain the four-byte frame length.
    TruncatedLength,
    /// Declared payload exceeds [`MAX_CONTROL_FRAME_BYTES`].
    FrameTooLarge,
    /// Declared payload length and supplied byte length differ.
    LengthMismatch,
    /// Protobuf payload is malformed.
    Decode(prost::DecodeError),
    /// Protobuf payload could not be encoded.
    Encode(prost::EncodeError),
    /// Envelope version differs from [`CONTROL_PROTOCOL_VERSION`].
    UnsupportedVersion(u32),
    /// Envelope does not contain a known `oneof` payload.
    MissingPayload,
    /// Hello exceeded its bounded capability count.
    TooManyCapabilities,
    /// Hello exceeded its bounded version count.
    TooManySupportedVersions,
    /// Display name exceeded [`MAX_CONTROL_DISPLAY_NAME_BYTES`].
    DisplayNameTooLong,
    /// Status or error detail exceeded [`MAX_CONTROL_DETAIL_BYTES`].
    DetailTooLong,
    /// Start stream command has an invalid identity or audio parameter.
    InvalidStartStream,
    /// Stop stream command has an invalid identity.
    InvalidStopStream,
    /// Stream command epoch cannot fit the media protocol's 16-bit field.
    InvalidMediaKeyEpoch,
}

impl fmt::Display for ControlFrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncatedLength => formatter.write_str("control frame is missing its length"),
            Self::FrameTooLarge => formatter.write_str("control frame exceeds the hard limit"),
            Self::LengthMismatch => {
                formatter.write_str("control frame length does not match payload")
            }
            Self::Decode(error) => write!(formatter, "control protobuf decode failed: {error}"),
            Self::Encode(error) => write!(formatter, "control protobuf encode failed: {error}"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported control protocol version {version}")
            }
            Self::MissingPayload => formatter.write_str("control envelope has no known payload"),
            Self::TooManyCapabilities => {
                formatter.write_str("control hello has too many capabilities")
            }
            Self::TooManySupportedVersions => {
                formatter.write_str("control hello has too many supported versions")
            }
            Self::DisplayNameTooLong => {
                formatter.write_str("control display name exceeds the limit")
            }
            Self::DetailTooLong => formatter.write_str("control detail exceeds the limit"),
            Self::InvalidStartStream => formatter.write_str("control start stream is invalid"),
            Self::InvalidStopStream => formatter.write_str("control stop stream is invalid"),
            Self::InvalidMediaKeyEpoch => {
                formatter.write_str("control media key epoch exceeds the protocol limit")
            }
        }
    }
}

impl std::error::Error for ControlFrameError {}

/// Encodes one validated envelope as a big-endian length-prefixed control frame.
///
/// # Errors
///
/// Returns [`ControlFrameError`] when envelope validation fails or its encoded
/// Protobuf payload exceeds the hard frame limit.
pub fn encode_control_frame(envelope: &Envelope) -> Result<Vec<u8>, ControlFrameError> {
    validate_control_envelope(envelope)?;
    let payload_len = envelope.encoded_len();
    if payload_len > MAX_CONTROL_FRAME_BYTES {
        return Err(ControlFrameError::FrameTooLarge);
    }
    let length = u32::try_from(payload_len).map_err(|_| ControlFrameError::FrameTooLarge)?;
    let mut frame = Vec::with_capacity(4 + payload_len);
    frame.extend_from_slice(&length.to_be_bytes());
    envelope
        .encode(&mut frame)
        .map_err(ControlFrameError::Encode)?;
    Ok(frame)
}

/// Decodes and validates exactly one big-endian length-prefixed control frame.
///
/// # Errors
///
/// Returns [`ControlFrameError`] before Protobuf decode when framing is truncated,
/// oversized, or inconsistent, and after decode for invalid envelope semantics.
pub fn decode_control_frame(frame: &[u8]) -> Result<Envelope, ControlFrameError> {
    if frame.len() < 4 {
        return Err(ControlFrameError::TruncatedLength);
    }
    let length_bytes: [u8; 4] = frame[..4]
        .try_into()
        .map_err(|_| ControlFrameError::TruncatedLength)?;
    let payload_len = usize::try_from(u32::from_be_bytes(length_bytes))
        .map_err(|_| ControlFrameError::FrameTooLarge)?;
    if payload_len > MAX_CONTROL_FRAME_BYTES {
        return Err(ControlFrameError::FrameTooLarge);
    }
    if frame.len().checked_sub(4) != Some(payload_len) {
        return Err(ControlFrameError::LengthMismatch);
    }
    let envelope = Envelope::decode(&frame[4..]).map_err(ControlFrameError::Decode)?;
    validate_control_envelope(&envelope)?;
    Ok(envelope)
}

/// Checks semantic bounds that Protobuf syntax does not enforce.
///
/// # Errors
///
/// Returns [`ControlFrameError`] for version, payload, count, or byte-limit
/// violations.
pub fn validate_control_envelope(envelope: &Envelope) -> Result<(), ControlFrameError> {
    if envelope.protocol_version != CONTROL_PROTOCOL_VERSION {
        return Err(ControlFrameError::UnsupportedVersion(
            envelope.protocol_version,
        ));
    }
    match envelope.payload.as_ref() {
        Some(envelope::Payload::Hello(hello)) => validate_hello(hello),
        Some(envelope::Payload::StreamStatus(status)) => validate_status(status),
        Some(envelope::Payload::Error(error)) => validate_error(error),
        Some(envelope::Payload::StartStream(start)) => validate_start_stream(start),
        Some(envelope::Payload::StopStream(stop)) => validate_stop_stream(stop),
        None => Err(ControlFrameError::MissingPayload),
    }
}

pub(crate) fn validate_hello(hello: &Hello) -> Result<(), ControlFrameError> {
    if hello.capabilities.len() > MAX_CONTROL_CAPABILITIES {
        return Err(ControlFrameError::TooManyCapabilities);
    }
    if hello.supported_versions.len() > MAX_CONTROL_SUPPORTED_VERSIONS {
        return Err(ControlFrameError::TooManySupportedVersions);
    }
    if hello.display_name.len() > MAX_CONTROL_DISPLAY_NAME_BYTES {
        return Err(ControlFrameError::DisplayNameTooLong);
    }
    Ok(())
}

fn validate_status(status: &StreamStatus) -> Result<(), ControlFrameError> {
    validate_detail(&status.detail)
}

fn validate_error(error: &ControlError) -> Result<(), ControlFrameError> {
    validate_detail(&error.detail)
}

fn validate_start_stream(start: &StartStream) -> Result<(), ControlFrameError> {
    if start.media_session_id == 0
        || start.stream_id == 0
        || !(MIN_CONTROL_SAMPLE_RATE_HZ..=MAX_CONTROL_SAMPLE_RATE_HZ)
            .contains(&start.sample_rate_hz)
        || !(1..=MAX_CONTROL_CHANNELS).contains(&start.channels)
        || !matches!(start.frame_duration_ms, 10 | 20)
    {
        return Err(ControlFrameError::InvalidStartStream);
    }
    validate_media_key_epoch(start.media_key_epoch)
}

fn validate_stop_stream(stop: &StopStream) -> Result<(), ControlFrameError> {
    if stop.media_session_id == 0 || stop.stream_id == 0 {
        return Err(ControlFrameError::InvalidStopStream);
    }
    validate_media_key_epoch(stop.media_key_epoch)
}

fn validate_media_key_epoch(epoch: u32) -> Result<(), ControlFrameError> {
    if epoch > MAX_CONTROL_MEDIA_KEY_EPOCH {
        return Err(ControlFrameError::InvalidMediaKeyEpoch);
    }
    Ok(())
}

fn validate_detail(detail: &str) -> Result<(), ControlFrameError> {
    if detail.len() > MAX_CONTROL_DETAIL_BYTES {
        return Err(ControlFrameError::DetailTooLong);
    }
    Ok(())
}
