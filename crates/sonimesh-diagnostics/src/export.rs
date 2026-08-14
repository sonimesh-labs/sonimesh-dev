//! Bounded, allowlisted diagnostic export bundles.
//!
//! This module implements the ADR 0011 bundle writer. The desktop control
//! domain owns the export action; it fills an [`ExportSessionSnapshot`] with
//! only approved low-rate values and calls [`build_export_bundle`] to produce
//! one deterministic, versioned JSON document. The realtime callback must
//! never call this module: building a bundle allocates and serializes.

use core::fmt;
use core::fmt::Write as _;
use std::net::SocketAddr;

use crate::{StreamMetricsSnapshot, redact_endpoint};

/// Current diagnostic bundle format version.
///
/// The bundle schema is versioned (ADR 0011 rollback plan): a new field
/// requires an explicit allowlist change and tests, and older builds never
/// emit fields a newer build introduced.
pub const EXPORT_FORMAT_VERSION: u32 = 1;

/// Fixed upper bound for one serialized diagnostic bundle in bytes.
///
/// Building a bundle that exceeds this bound fails closed with
/// [`ExportError::BundleTooLarge`]; the writer never truncates or returns a
/// partial bundle.
pub const MAX_EXPORT_BYTES: usize = 16 * 1024;

/// Fixed upper bound for the number of stream snapshots in one bundle.
///
/// This is headroom above the Phase 2 mixer's eight-slot bound and keeps the
/// serialized size bounded independently of the byte limit.
pub const MAX_EXPORT_STREAMS: usize = 32;

/// Fixed upper bound for the software-version token.
pub const MAX_SOFTWARE_VERSION_LEN: usize = 64;

/// Fixed upper bound for a control-domain-defined buffer policy token.
pub const MAX_POLICY_TOKEN_LEN: usize = 32;

/// Approved platform output format value included in an export.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExportPlatformFormat {
    /// Device sample rate in hertz.
    pub sample_rate_hz: u32,
    /// Interleaved output channel count.
    pub channels: u16,
}

/// Closed set of session phases the export schema understands.
///
/// The control domain maps its lifecycle phase onto this closed set; an
/// unknown future phase is omitted rather than guessed, so older builds never
/// emit a phase token a newer schema introduced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExportSessionPhase {
    /// No trusted media session is active.
    Idle,
    /// A short-lived first-pairing record is active.
    Pairing,
    /// Trusted control session is ready but not streaming.
    Ready,
    /// Authenticated media is being received.
    Streaming,
    /// Control or network reachability is within a bounded grace period.
    Reconnecting,
    /// Output device must be recreated by the control domain.
    OutputUnavailable,
}

impl ExportSessionPhase {
    /// Returns the stable bundle token for this phase.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Pairing => "pairing",
            Self::Ready => "ready",
            Self::Streaming => "streaming",
            Self::Reconnecting => "reconnecting",
            Self::OutputUnavailable => "output_unavailable",
        }
    }
}

/// Approved low-rate snapshot the desktop control domain may export.
///
/// Every field is allowlisted by the bundle schema: software version, the
/// endpoint family and port (never the address), platform output format,
/// selected buffer policy, session phase, and stream counter snapshots.
/// Streams carry no wire identity; their array position is the redacted
/// per-bundle ordinal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExportSessionSnapshot<'a> {
    /// Bounded printable-ASCII software version, for example `0.1.0`.
    pub software_version: &'a str,
    /// Endpoint to include as a redacted family and port marker.
    pub endpoint: Option<SocketAddr>,
    /// Approved platform output format, or `None` before any output starts.
    pub platform_format: Option<ExportPlatformFormat>,
    /// Control-domain-defined buffer policy token, or `None` if unset.
    ///
    /// The token is bounded to [`MAX_POLICY_TOKEN_LEN`] printable-ASCII
    /// characters. The schema may promote this to a closed set in a future
    /// format version once the control domain defines its policy set.
    pub buffer_policy: Option<&'a str>,
    /// Lifecycle phase at snapshot time, or `None` if unknown.
    pub session_phase: Option<ExportSessionPhase>,
    /// One counter snapshot per active stream, in bundle ordinal order.
    pub streams: &'a [StreamMetricsSnapshot],
}

/// Failure while building one diagnostic export bundle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExportError {
    /// The software version token is empty.
    EmptySoftwareVersion,
    /// The software version exceeds the fixed byte bound.
    SoftwareVersionTooLong {
        /// Fixed bound.
        max: usize,
        /// Actual byte length of the rejected token.
        actual: usize,
    },
    /// The software version is not printable ASCII.
    SoftwareVersionNotAscii,
    /// The buffer policy token is empty.
    EmptyBufferPolicy,
    /// The buffer policy token exceeds the fixed byte bound.
    BufferPolicyTooLong {
        /// Fixed bound.
        max: usize,
        /// Actual byte length of the rejected token.
        actual: usize,
    },
    /// The buffer policy token is not printable ASCII.
    BufferPolicyNotAscii,
    /// The snapshot carries more streams than the fixed export bound.
    TooManyStreams {
        /// Fixed bound.
        max: usize,
        /// Actual stream count of the rejected snapshot.
        actual: usize,
    },
    /// The serialized bundle exceeds the fixed byte bound.
    BundleTooLarge {
        /// Fixed bound.
        max: usize,
        /// Actual byte length of the rejected bundle.
        actual: usize,
    },
}

impl fmt::Display for ExportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySoftwareVersion => {
                formatter.write_str("diagnostic export software version is empty")
            }
            Self::SoftwareVersionTooLong { max, actual } => write!(
                formatter,
                "diagnostic export software version is {actual} bytes, over the {max} byte bound"
            ),
            Self::SoftwareVersionNotAscii => {
                formatter.write_str("diagnostic export software version must be printable ASCII")
            }
            Self::EmptyBufferPolicy => {
                formatter.write_str("diagnostic export buffer policy token is empty")
            }
            Self::BufferPolicyTooLong { max, actual } => write!(
                formatter,
                "diagnostic export buffer policy token is {actual} bytes, over the {max} byte bound"
            ),
            Self::BufferPolicyNotAscii => {
                formatter.write_str("diagnostic export buffer policy token must be printable ASCII")
            }
            Self::TooManyStreams { max, actual } => write!(
                formatter,
                "diagnostic export carries {actual} streams, over the {max} stream bound"
            ),
            Self::BundleTooLarge { max, actual } => write!(
                formatter,
                "diagnostic export bundle is {actual} bytes, over the {max} byte bound"
            ),
        }
    }
}

impl std::error::Error for ExportError {}

/// Builds one deterministic, bounded, versioned JSON bundle.
///
/// The returned document contains exactly the allowlisted fields in a fixed
/// order and never truncates: an oversized bundle is rejected with
/// [`ExportError::BundleTooLarge`]. Cumulative `u64` counters are emitted as
/// unsigned decimal strings so a JavaScript consumer cannot lose precision;
/// bounded queue, format, and timing fields stay numeric.
///
/// # Errors
///
/// Returns an [`ExportError`] when the software version or buffer policy
/// token is empty, too long, or not printable ASCII, when the snapshot carries
/// more than [`MAX_EXPORT_STREAMS`] streams, or when the serialized bundle
/// exceeds [`MAX_EXPORT_BYTES`].
pub fn build_export_bundle(snapshot: &ExportSessionSnapshot<'_>) -> Result<String, ExportError> {
    validate_software_version(snapshot.software_version)?;
    if let Some(policy) = snapshot.buffer_policy {
        validate_policy_token(policy)?;
    }
    if snapshot.streams.len() > MAX_EXPORT_STREAMS {
        return Err(ExportError::TooManyStreams {
            max: MAX_EXPORT_STREAMS,
            actual: snapshot.streams.len(),
        });
    }

    let mut bundle = String::new();
    bundle.push('{');
    let mut first = true;

    write_number_field(
        &mut bundle,
        &mut first,
        "format_version",
        EXPORT_FORMAT_VERSION,
    );
    write_string_field(
        &mut bundle,
        &mut first,
        "software_version",
        snapshot.software_version,
    );

    if let Some(endpoint) = snapshot.endpoint {
        // Redaction runs before any part of the endpoint is written; the
        // current SocketAddr redaction is infallible, and a future redaction
        // failure must abort here instead of emitting raw data.
        write_string_field(
            &mut bundle,
            &mut first,
            "endpoint",
            &redact_endpoint(endpoint),
        );
    }
    if let Some(format) = snapshot.platform_format {
        write_separator(&mut bundle, &mut first);
        write_json_string(&mut bundle, "platform_format");
        bundle.push_str(":{");
        let mut format_first = true;
        write_number_field(
            &mut bundle,
            &mut format_first,
            "sample_rate_hz",
            format.sample_rate_hz,
        );
        write_number_field(&mut bundle, &mut format_first, "channels", format.channels);
        bundle.push('}');
    }
    if let Some(policy) = snapshot.buffer_policy {
        write_string_field(&mut bundle, &mut first, "buffer_policy", policy);
    }
    if let Some(phase) = snapshot.session_phase {
        write_string_field(&mut bundle, &mut first, "session_phase", phase.as_str());
    }

    write_separator(&mut bundle, &mut first);
    write_json_string(&mut bundle, "streams");
    bundle.push_str(":[");
    for (index, stream) in snapshot.streams.iter().enumerate() {
        if index > 0 {
            bundle.push(',');
        }
        write_stream_object(&mut bundle, stream);
    }
    bundle.push_str("]}");

    if bundle.len() > MAX_EXPORT_BYTES {
        return Err(ExportError::BundleTooLarge {
            max: MAX_EXPORT_BYTES,
            actual: bundle.len(),
        });
    }
    Ok(bundle)
}

fn validate_software_version(version: &str) -> Result<(), ExportError> {
    if version.is_empty() {
        return Err(ExportError::EmptySoftwareVersion);
    }
    if version.len() > MAX_SOFTWARE_VERSION_LEN {
        return Err(ExportError::SoftwareVersionTooLong {
            max: MAX_SOFTWARE_VERSION_LEN,
            actual: version.len(),
        });
    }
    if !is_printable_ascii(version) {
        return Err(ExportError::SoftwareVersionNotAscii);
    }
    Ok(())
}

fn validate_policy_token(token: &str) -> Result<(), ExportError> {
    if token.is_empty() {
        return Err(ExportError::EmptyBufferPolicy);
    }
    if token.len() > MAX_POLICY_TOKEN_LEN {
        return Err(ExportError::BufferPolicyTooLong {
            max: MAX_POLICY_TOKEN_LEN,
            actual: token.len(),
        });
    }
    if !is_printable_ascii(token) {
        return Err(ExportError::BufferPolicyNotAscii);
    }
    Ok(())
}

fn is_printable_ascii(value: &str) -> bool {
    value.chars().all(|c| c.is_ascii_graphic() || c == ' ')
}

/// One allowlisted stream counter value in bundle field order.
///
/// Cumulative counters cross a JSON boundary as unsigned decimal strings to
/// avoid JavaScript precision loss; bounded queue, format, and timing values
/// stay numeric (ADR 0011).
enum ExportCounterValue {
    /// Cumulative `u64` counter serialized as an unsigned decimal string.
    Decimal(u64),
    /// Bounded numeric value that fits exactly in a JSON number.
    Number(i64),
}

#[allow(clippy::too_many_lines)]
fn write_stream_object(out: &mut String, snapshot: &StreamMetricsSnapshot) {
    out.push('{');
    let mut first = true;
    for (key, value) in [
        (
            "authenticated_packets",
            ExportCounterValue::Decimal(snapshot.authenticated_packets),
        ),
        (
            "duplicate_packets",
            ExportCounterValue::Decimal(snapshot.duplicate_packets),
        ),
        (
            "rejected_packets",
            ExportCounterValue::Decimal(snapshot.rejected_packets),
        ),
        (
            "late_packets",
            ExportCounterValue::Decimal(snapshot.late_packets),
        ),
        (
            "reordered_packets",
            ExportCounterValue::Decimal(snapshot.reordered_packets),
        ),
        (
            "jitter_resync_dropped_frames",
            ExportCounterValue::Decimal(snapshot.jitter_resync_dropped_frames),
        ),
        (
            "jitter_frames",
            ExportCounterValue::Number(i64::from(snapshot.jitter_frames)),
        ),
        (
            "jitter_high_water_frames",
            ExportCounterValue::Number(i64::from(snapshot.jitter_high_water_frames)),
        ),
        (
            "target_min_frames",
            ExportCounterValue::Number(i64::from(snapshot.target_min_frames)),
        ),
        (
            "target_max_frames",
            ExportCounterValue::Number(i64::from(snapshot.target_max_frames)),
        ),
        (
            "target_frames",
            ExportCounterValue::Number(i64::from(snapshot.target_frames)),
        ),
        (
            "target_smoothed_jitter_ms",
            ExportCounterValue::Number(i64::from(snapshot.target_smoothed_jitter_ms)),
        ),
        (
            "target_raised_steps",
            ExportCounterValue::Number(i64::from(snapshot.target_raised_steps)),
        ),
        (
            "target_lowered_steps",
            ExportCounterValue::Number(i64::from(snapshot.target_lowered_steps)),
        ),
        (
            "plc_frames",
            ExportCounterValue::Decimal(snapshot.plc_frames),
        ),
        (
            "decode_errors",
            ExportCounterValue::Decimal(snapshot.decode_errors),
        ),
        (
            "pcm_underflows",
            ExportCounterValue::Decimal(snapshot.pcm_underflows),
        ),
        (
            "pcm_refill_limit_hits",
            ExportCounterValue::Decimal(snapshot.pcm_refill_limit_hits),
        ),
        (
            "pcm_overflows",
            ExportCounterValue::Decimal(snapshot.pcm_overflows),
        ),
        (
            "pcm_discontinuities",
            ExportCounterValue::Decimal(snapshot.pcm_discontinuities),
        ),
        (
            "queued_pcm_blocks",
            ExportCounterValue::Number(i64::from(snapshot.queued_pcm_blocks)),
        ),
        (
            "queued_pcm_high_water_blocks",
            ExportCounterValue::Number(i64::from(snapshot.queued_pcm_high_water_blocks)),
        ),
        (
            "stream_resets",
            ExportCounterValue::Decimal(snapshot.stream_resets),
        ),
        (
            "nominal_input_sample_rate_hz",
            ExportCounterValue::Number(i64::from(snapshot.nominal_input_sample_rate_hz)),
        ),
        (
            "nominal_output_sample_rate_hz",
            ExportCounterValue::Number(i64::from(snapshot.nominal_output_sample_rate_hz)),
        ),
        (
            "drift_adjustment_ppm",
            ExportCounterValue::Number(i64::from(snapshot.drift_adjustment_ppm)),
        ),
        (
            "drift_hard_resyncs",
            ExportCounterValue::Decimal(snapshot.drift_hard_resyncs),
        ),
    ] {
        match value {
            ExportCounterValue::Decimal(value) => {
                write_string_field(out, &mut first, key, &value.to_string());
            }
            ExportCounterValue::Number(value) => {
                write_number_field(out, &mut first, key, value);
            }
        }
    }
    out.push('}');
}

fn write_separator(out: &mut String, first: &mut bool) {
    if *first {
        *first = false;
    } else {
        out.push(',');
    }
}

fn write_string_field(out: &mut String, first: &mut bool, key: &str, value: &str) {
    write_separator(out, first);
    write_json_string(out, key);
    out.push(':');
    write_json_string(out, value);
}

fn write_number_field<T: fmt::Display>(out: &mut String, first: &mut bool, key: &str, value: T) {
    write_separator(out, first);
    write_json_string(out, key);
    out.push(':');
    out.push_str(&value.to_string());
}

fn write_json_string(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use super::*;

    fn endpoint(ip: &str, port: u16) -> SocketAddr {
        format!("{ip}:{port}").parse().unwrap()
    }

    fn sample_stream() -> StreamMetricsSnapshot {
        StreamMetricsSnapshot {
            authenticated_packets: 12,
            pcm_underflows: 3,
            pcm_refill_limit_hits: 1,
            queued_pcm_blocks: 7,
            queued_pcm_high_water_blocks: 9,
            jitter_frames: 3,
            jitter_high_water_frames: 5,
            target_min_frames: 2,
            target_max_frames: 8,
            target_frames: 4,
            target_smoothed_jitter_ms: 35,
            target_raised_steps: 3,
            target_lowered_steps: 1,
            drift_adjustment_ppm: -125,
            ..StreamMetricsSnapshot::default()
        }
    }

    fn full_snapshot(streams: &[StreamMetricsSnapshot]) -> ExportSessionSnapshot<'_> {
        ExportSessionSnapshot {
            software_version: "0.1.0",
            endpoint: Some(endpoint("192.168.1.50", 49321)),
            platform_format: Some(ExportPlatformFormat {
                sample_rate_hz: 48_000,
                channels: 2,
            }),
            buffer_policy: Some("balanced"),
            session_phase: Some(ExportSessionPhase::Streaming),
            streams,
        }
    }

    #[test]
    fn golden_bundle_is_deterministic_and_allowlisted() {
        let stream = sample_stream();
        let streams = [stream];
        let snapshot = full_snapshot(&streams);
        let expected = "{\"format_version\":1,\"software_version\":\"0.1.0\",\"endpoint\":\"[ipv4-address-redacted]:49321\",\"platform_format\":{\"sample_rate_hz\":48000,\"channels\":2},\"buffer_policy\":\"balanced\",\"session_phase\":\"streaming\",\"streams\":[{\"authenticated_packets\":\"12\",\"duplicate_packets\":\"0\",\"rejected_packets\":\"0\",\"late_packets\":\"0\",\"reordered_packets\":\"0\",\"jitter_resync_dropped_frames\":\"0\",\"jitter_frames\":3,\"jitter_high_water_frames\":5,\"target_min_frames\":2,\"target_max_frames\":8,\"target_frames\":4,\"target_smoothed_jitter_ms\":35,\"target_raised_steps\":3,\"target_lowered_steps\":1,\"plc_frames\":\"0\",\"decode_errors\":\"0\",\"pcm_underflows\":\"3\",\"pcm_refill_limit_hits\":\"1\",\"pcm_overflows\":\"0\",\"pcm_discontinuities\":\"0\",\"queued_pcm_blocks\":7,\"queued_pcm_high_water_blocks\":9,\"stream_resets\":\"0\",\"nominal_input_sample_rate_hz\":0,\"nominal_output_sample_rate_hz\":0,\"drift_adjustment_ppm\":-125,\"drift_hard_resyncs\":\"0\"}]}";
        let first = build_export_bundle(&snapshot).unwrap();
        let second = build_export_bundle(&snapshot).unwrap();
        assert_eq!(first, expected);
        assert_eq!(second, expected);
    }

    #[test]
    fn u64_counters_are_decimal_strings_and_bounded_fields_are_numeric() {
        let stream = StreamMetricsSnapshot {
            authenticated_packets: u64::MAX,
            drift_adjustment_ppm: i32::MIN,
            nominal_input_sample_rate_hz: 48_000,
            ..StreamMetricsSnapshot::default()
        };
        let snapshot = ExportSessionSnapshot {
            software_version: "0.1.0",
            endpoint: None,
            platform_format: None,
            buffer_policy: None,
            session_phase: None,
            streams: &[stream],
        };
        let bundle = build_export_bundle(&snapshot).unwrap();
        assert!(bundle.contains("\"authenticated_packets\":\"18446744073709551615\""));
        assert!(bundle.contains("\"drift_adjustment_ppm\":-2147483648"));
        assert!(bundle.contains("\"nominal_input_sample_rate_hz\":48000"));
        assert!(bundle.contains("\"jitter_frames\":0"));
        assert!(bundle.contains("\"target_min_frames\":0"));
        assert!(bundle.contains("\"target_max_frames\":0"));
        assert!(bundle.contains("\"target_frames\":0"));
        assert!(bundle.contains("\"target_smoothed_jitter_ms\":0"));
        assert!(!bundle.contains("\"queued_pcm_blocks\":\""));
        assert!(!bundle.contains("\"jitter_frames\":\""));
        assert!(!bundle.contains("\"target_frames\":\""));
        assert!(!bundle.contains("\"target_smoothed_jitter_ms\":\""));
    }

    #[test]
    fn endpoint_is_redacted_for_ipv4_and_ipv6() {
        let stream = sample_stream();
        let streams = [stream];
        let ipv4 = ExportSessionSnapshot {
            endpoint: Some(endpoint("192.168.10.42", 49321)),
            ..full_snapshot(&streams)
        };
        let ipv6 = ExportSessionSnapshot {
            endpoint: Some(endpoint("[2001:db8:85a3::8a2e:370:7334]", 49321)),
            ..full_snapshot(&streams)
        };
        let bundle_ipv4 = build_export_bundle(&ipv4).unwrap();
        let bundle_ipv6 = build_export_bundle(&ipv6).unwrap();
        assert!(bundle_ipv4.contains("[ipv4-address-redacted]:49321"));
        assert!(!bundle_ipv4.contains("192.168"));
        assert!(bundle_ipv6.contains("[ipv6-address-redacted]:49321"));
        assert!(!bundle_ipv6.contains("2001:db8"));
        assert!(!bundle_ipv6.contains("7334"));
    }

    #[test]
    fn absent_optional_fields_are_omitted() {
        let snapshot = ExportSessionSnapshot {
            software_version: "0.1.0",
            endpoint: None,
            platform_format: None,
            buffer_policy: None,
            session_phase: None,
            streams: &[],
        };
        let bundle = build_export_bundle(&snapshot).unwrap();
        assert_eq!(
            bundle,
            "{\"format_version\":1,\"software_version\":\"0.1.0\",\"streams\":[]}"
        );
    }

    #[test]
    fn empty_stream_list_is_emitted_as_an_empty_array() {
        let snapshot = ExportSessionSnapshot {
            software_version: "0.1.0",
            endpoint: None,
            platform_format: None,
            buffer_policy: None,
            session_phase: None,
            streams: &[],
        };
        let bundle = build_export_bundle(&snapshot).unwrap();
        assert!(bundle.ends_with("\"streams\":[]}"));
    }

    #[test]
    fn software_version_is_validated_fail_closed() {
        let empty = ExportSessionSnapshot {
            software_version: "",
            endpoint: None,
            platform_format: None,
            buffer_policy: None,
            session_phase: None,
            streams: &[],
        };
        assert_eq!(
            build_export_bundle(&empty),
            Err(ExportError::EmptySoftwareVersion)
        );

        let too_long = "v".repeat(MAX_SOFTWARE_VERSION_LEN + 1);
        let long = ExportSessionSnapshot {
            software_version: too_long.as_str(),
            endpoint: None,
            platform_format: None,
            buffer_policy: None,
            session_phase: None,
            streams: &[],
        };
        assert_eq!(
            build_export_bundle(&long),
            Err(ExportError::SoftwareVersionTooLong {
                max: MAX_SOFTWARE_VERSION_LEN,
                actual: MAX_SOFTWARE_VERSION_LEN + 1,
            })
        );

        let non_ascii = ExportSessionSnapshot {
            software_version: "0.1.0-\u{7248}\u{672c}",
            endpoint: None,
            platform_format: None,
            buffer_policy: None,
            session_phase: None,
            streams: &[],
        };
        assert_eq!(
            build_export_bundle(&non_ascii),
            Err(ExportError::SoftwareVersionNotAscii)
        );
    }

    #[test]
    fn buffer_policy_is_validated_fail_closed() {
        let empty = ExportSessionSnapshot {
            software_version: "0.1.0",
            endpoint: None,
            platform_format: None,
            buffer_policy: Some(""),
            session_phase: None,
            streams: &[],
        };
        assert_eq!(
            build_export_bundle(&empty),
            Err(ExportError::EmptyBufferPolicy)
        );

        let too_long = "p".repeat(MAX_POLICY_TOKEN_LEN + 1);
        let long = ExportSessionSnapshot {
            software_version: "0.1.0",
            endpoint: None,
            platform_format: None,
            buffer_policy: Some(too_long.as_str()),
            session_phase: None,
            streams: &[],
        };
        assert_eq!(
            build_export_bundle(&long),
            Err(ExportError::BufferPolicyTooLong {
                max: MAX_POLICY_TOKEN_LEN,
                actual: MAX_POLICY_TOKEN_LEN + 1,
            })
        );

        let non_ascii = ExportSessionSnapshot {
            software_version: "0.1.0",
            endpoint: None,
            platform_format: None,
            buffer_policy: Some("\u{5747}\u{8861}"),
            session_phase: None,
            streams: &[],
        };
        assert_eq!(
            build_export_bundle(&non_ascii),
            Err(ExportError::BufferPolicyNotAscii)
        );
    }

    #[test]
    fn stream_count_over_the_fixed_bound_is_rejected() {
        let streams = vec![StreamMetricsSnapshot::default(); MAX_EXPORT_STREAMS + 1];
        let snapshot = ExportSessionSnapshot {
            software_version: "0.1.0",
            endpoint: None,
            platform_format: None,
            buffer_policy: None,
            session_phase: None,
            streams: &streams,
        };
        assert_eq!(
            build_export_bundle(&snapshot),
            Err(ExportError::TooManyStreams {
                max: MAX_EXPORT_STREAMS,
                actual: MAX_EXPORT_STREAMS + 1,
            })
        );
    }

    #[test]
    fn oversized_bundle_is_rejected_without_truncation() {
        let stream = StreamMetricsSnapshot {
            authenticated_packets: u64::MAX,
            duplicate_packets: u64::MAX,
            rejected_packets: u64::MAX,
            late_packets: u64::MAX,
            reordered_packets: u64::MAX,
            jitter_resync_dropped_frames: u64::MAX,
            plc_frames: u64::MAX,
            decode_errors: u64::MAX,
            pcm_underflows: u64::MAX,
            pcm_refill_limit_hits: u64::MAX,
            pcm_overflows: u64::MAX,
            pcm_discontinuities: u64::MAX,
            queued_pcm_blocks: u16::MAX,
            queued_pcm_high_water_blocks: u16::MAX,
            jitter_frames: u16::MAX,
            jitter_high_water_frames: u16::MAX,
            target_min_frames: u16::MAX,
            target_max_frames: u16::MAX,
            target_frames: u16::MAX,
            target_smoothed_jitter_ms: u32::MAX,
            target_raised_steps: u32::MAX,
            target_lowered_steps: u32::MAX,
            stream_resets: u64::MAX,
            nominal_input_sample_rate_hz: 192_000,
            nominal_output_sample_rate_hz: 8_000,
            drift_adjustment_ppm: i32::MAX,
            drift_hard_resyncs: u64::MAX,
        };

        let streams = vec![stream; MAX_EXPORT_STREAMS];
        let snapshot = ExportSessionSnapshot {
            software_version: "0.1.0",
            endpoint: Some(endpoint("192.168.1.50", 49321)),
            platform_format: Some(ExportPlatformFormat {
                sample_rate_hz: 192_000,
                channels: 8,
            }),
            buffer_policy: Some("balanced"),
            session_phase: Some(ExportSessionPhase::Streaming),
            streams: &streams,
        };
        let error = build_export_bundle(&snapshot).unwrap_err();
        match error {
            ExportError::BundleTooLarge { max, actual } => {
                assert_eq!(max, MAX_EXPORT_BYTES);
                assert!(actual > max, "bundle must exceed the byte bound");
            }
            other => panic!("expected BundleTooLarge, found {other:?}"),
        }
    }

    #[test]
    fn realistic_stream_count_fits_the_byte_bound() {
        let streams = vec![StreamMetricsSnapshot::default(); 8];
        let snapshot = ExportSessionSnapshot {
            software_version: "0.1.0",
            endpoint: Some(endpoint("192.168.1.50", 49321)),
            platform_format: Some(ExportPlatformFormat {
                sample_rate_hz: 48_000,
                channels: 2,
            }),
            buffer_policy: Some("balanced"),
            session_phase: Some(ExportSessionPhase::Streaming),
            streams: &streams,
        };
        let bundle = build_export_bundle(&snapshot).unwrap();
        assert!(
            bundle.len() <= MAX_EXPORT_BYTES,
            "realistic bundle must fit: {} > {}",
            bundle.len(),
            MAX_EXPORT_BYTES
        );
    }

    #[test]
    fn forbidden_byte_sequences_never_appear_in_bundles() {
        let stream = sample_stream();
        let streams = [stream];
        let snapshot = full_snapshot(&streams);
        let bundle = build_export_bundle(&snapshot).unwrap();
        for forbidden in [
            "192.168",
            "session_id",
            "stream_id",
            "key_epoch",
            "secret",
            "certificate",
            "pairing",
            "display_name",
        ] {
            assert!(
                !bundle.contains(forbidden),
                "bundle must not contain {forbidden:?}"
            );
        }
    }

    #[test]
    fn session_phase_tokens_are_stable() {
        assert_eq!(ExportSessionPhase::Idle.as_str(), "idle");
        assert_eq!(ExportSessionPhase::Pairing.as_str(), "pairing");
        assert_eq!(ExportSessionPhase::Ready.as_str(), "ready");
        assert_eq!(ExportSessionPhase::Streaming.as_str(), "streaming");
        assert_eq!(ExportSessionPhase::Reconnecting.as_str(), "reconnecting");
        assert_eq!(
            ExportSessionPhase::OutputUnavailable.as_str(),
            "output_unavailable"
        );
    }
}
