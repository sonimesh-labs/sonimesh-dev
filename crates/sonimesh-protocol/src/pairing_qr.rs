//! Bounded version-one pairing QR payload encoding and decoding.
//!
//! The QR payload is the only channel that carries the first-pair trust
//! inputs defined by ADR 0008: the desktop control endpoint, the desktop TLS
//! SPKI pin, the one-time pairing ID and secret, and the monotonic expiry.
//! Encoding and decoding are allocation-free and fail closed: every length is
//! bounded before any data is copied, the declared lengths must exactly match
//! the payload length, and the decoded pairing secret borrows the caller's
//! buffer so the caller owns its zeroization. This module performs no TLS,
//! pairing-record consumption, proof exchange, or trust storage; the
//! platform pairing owner consumes the decoded view after the QR image has
//! been scanned.

use core::fmt;

use crate::CONTROL_PROTOCOL_VERSION;

/// Packet-family marker for the version-one pairing QR payload (`SMQR`).
pub const QR_PAYLOAD_MAGIC: [u8; 4] = *b"SMQR";
/// Current pairing QR payload version.
pub const QR_PAYLOAD_VERSION: u8 = 1;
/// Length of the fixed QR payload prefix through the monotonic expiry field.
pub const QR_PREFIX_LEN: usize = 67;
/// Largest accepted control-host byte length.
pub const QR_HOST_MAX_LEN: usize = 255;
/// Smallest accepted desktop TLS SPKI length.
///
/// A DER `SubjectPublicKeyInfo` for any supported key type is longer than
/// this; a shorter declared pin is malformed rather than trusted.
pub const QR_SPKI_MIN_LEN: usize = 32;
/// Largest accepted desktop TLS SPKI length.
pub const QR_SPKI_MAX_LEN: usize = 512;
/// Largest complete QR payload: prefix, maximal host, and maximal SPKI.
pub const MAX_QR_PAYLOAD_LEN: usize = QR_PREFIX_LEN + QR_HOST_MAX_LEN + QR_SPKI_MAX_LEN;

const QR_HOST_LEN_OFFSET: usize = 6;
const QR_PORT_OFFSET: usize = 7;
const QR_SPKI_LEN_OFFSET: usize = 9;
const QR_PAIRING_ID_OFFSET: usize = 11;
const QR_PAIRING_SECRET_OFFSET: usize = 27;
const QR_EXPIRY_OFFSET: usize = 59;
const QR_HOST_OFFSET: usize = QR_PREFIX_LEN;

/// Failure returned while encoding or decoding a pairing QR payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QrPayloadError {
    /// Complete payload exceeds the fixed QR payload bound.
    QrPayloadTooLarge,
    /// Payload cannot contain the complete fixed prefix.
    Truncated,
    /// Payload marker belongs to another protocol.
    InvalidMagic,
    /// QR payload version is unsupported.
    UnsupportedQrVersion(u8),
    /// Control protocol version is not the supported version one.
    InvalidProtocolVersion(u8),
    /// Control-host length is zero or exceeds the byte bound.
    InvalidHostLength,
    /// Desktop TLS SPKI length is outside the accepted range.
    InvalidSpkiLength,
    /// Control port is zero.
    InvalidPort,
    /// Monotonic expiry is zero and can never be observed in time.
    InvalidExpiry,
    /// Declared host and SPKI lengths do not match the payload length.
    LengthMismatch,
    /// Control host is not UTF-8 or contains control characters.
    InvalidHostEncoding,
}

impl fmt::Display for QrPayloadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QrPayloadTooLarge => {
                formatter.write_str("pairing QR payload exceeds the hard limit")
            }
            Self::Truncated => formatter.write_str("pairing QR payload is truncated"),
            Self::InvalidMagic => {
                formatter.write_str("pairing QR payload has an invalid magic value")
            }
            Self::UnsupportedQrVersion(version) => {
                write!(
                    formatter,
                    "unsupported pairing QR payload version {version}"
                )
            }
            Self::InvalidProtocolVersion(version) => {
                write!(formatter, "unsupported pairing protocol version {version}")
            }
            Self::InvalidHostLength => formatter.write_str("pairing QR host length is invalid"),
            Self::InvalidSpkiLength => {
                formatter.write_str("pairing QR desktop SPKI length is invalid")
            }
            Self::InvalidPort => formatter.write_str("pairing QR control port is zero"),
            Self::InvalidExpiry => formatter.write_str("pairing QR expiry is zero"),
            Self::LengthMismatch => {
                formatter.write_str("pairing QR payload length does not match its lengths")
            }
            Self::InvalidHostEncoding => {
                formatter.write_str("pairing QR host is not UTF-8 text without control characters")
            }
        }
    }
}

impl std::error::Error for QrPayloadError {}

/// Validated pairing QR payload ready for bounded encoding.
///
/// The host and SPKI borrow caller storage; [`Self::encoded_len`] reports the
/// exact wire length and [`Self::encode_into`] writes it without allocating.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PairingQrPayload<'a> {
    /// Control protocol version the QR invitation was created for.
    pub protocol_version: u8,
    /// One-time 128-bit pairing identifier.
    pub pairing_id: [u8; 16],
    /// One-time 256-bit pairing secret.
    pub pairing_secret: [u8; 32],
    /// Desktop control endpoint host or IP literal.
    pub control_host: &'a str,
    /// Desktop control endpoint TCP port.
    pub control_port: u16,
    /// Exact DER desktop TLS `SubjectPublicKeyInfo` bytes to pin.
    pub desktop_tls_spki: &'a [u8],
    /// Monotonic expiry in milliseconds for the desktop pairing record.
    pub expires_at_ms: u64,
}

impl PairingQrPayload<'_> {
    /// Validates the payload fields against the version-one bounds.
    fn validate(&self) -> Result<(), QrPayloadError> {
        if u32::from(self.protocol_version) != CONTROL_PROTOCOL_VERSION {
            return Err(QrPayloadError::InvalidProtocolVersion(
                self.protocol_version,
            ));
        }
        if self.control_host.is_empty() || self.control_host.len() > QR_HOST_MAX_LEN {
            return Err(QrPayloadError::InvalidHostLength);
        }
        if !is_printable_host(self.control_host) {
            return Err(QrPayloadError::InvalidHostEncoding);
        }
        if self.control_port == 0 {
            return Err(QrPayloadError::InvalidPort);
        }
        if !(QR_SPKI_MIN_LEN..=QR_SPKI_MAX_LEN).contains(&self.desktop_tls_spki.len()) {
            return Err(QrPayloadError::InvalidSpkiLength);
        }
        if self.expires_at_ms == 0 {
            return Err(QrPayloadError::InvalidExpiry);
        }
        Ok(())
    }

    /// Returns the exact encoded byte length, or [`None`] when the host
    /// cannot be represented by the one-byte host-length field.
    #[must_use]
    pub fn encoded_len(&self) -> Option<usize> {
        let host_len = u8::try_from(self.control_host.len()).ok()?;
        Some(QR_PREFIX_LEN + usize::from(host_len) + self.desktop_tls_spki.len())
    }

    /// Encodes the payload into `out` and returns the written length.
    ///
    /// # Errors
    ///
    /// Returns [`QrPayloadError`] for any field outside the version-one
    /// bounds, or when `out` is shorter than [`Self::encoded_len`].
    pub fn encode_into(&self, out: &mut [u8]) -> Result<usize, QrPayloadError> {
        self.validate()?;
        let encoded_len = self
            .encoded_len()
            .ok_or(QrPayloadError::InvalidHostLength)?;
        let out = out
            .get_mut(..encoded_len)
            .ok_or(QrPayloadError::QrPayloadTooLarge)?;

        out[0..4].copy_from_slice(&QR_PAYLOAD_MAGIC);
        out[4] = QR_PAYLOAD_VERSION;
        out[5] = self.protocol_version;
        let host_len =
            u8::try_from(self.control_host.len()).map_err(|_| QrPayloadError::InvalidHostLength)?;
        out[QR_HOST_LEN_OFFSET] = host_len;
        out[QR_PORT_OFFSET..QR_PORT_OFFSET + 2].copy_from_slice(&self.control_port.to_be_bytes());
        let spki_len = u16::try_from(self.desktop_tls_spki.len())
            .map_err(|_| QrPayloadError::InvalidSpkiLength)?;
        out[QR_SPKI_LEN_OFFSET..QR_SPKI_LEN_OFFSET + 2].copy_from_slice(&spki_len.to_be_bytes());
        out[QR_PAIRING_ID_OFFSET..QR_PAIRING_ID_OFFSET + 16].copy_from_slice(&self.pairing_id);
        out[QR_PAIRING_SECRET_OFFSET..QR_PAIRING_SECRET_OFFSET + 32]
            .copy_from_slice(&self.pairing_secret);
        out[QR_EXPIRY_OFFSET..QR_EXPIRY_OFFSET + 8]
            .copy_from_slice(&self.expires_at_ms.to_be_bytes());
        let host_end = QR_HOST_OFFSET + self.control_host.len();
        out[QR_HOST_OFFSET..host_end].copy_from_slice(self.control_host.as_bytes());
        out[host_end..].copy_from_slice(self.desktop_tls_spki);
        Ok(encoded_len)
    }
}

impl fmt::Debug for PairingQrPayload<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PairingQrPayload")
            .field("protocol_version", &self.protocol_version)
            .field("pairing_id", &"[REDACTED]")
            .field("pairing_secret", &"[REDACTED]")
            .field("control_host", &"[REDACTED]")
            .field("control_port", &self.control_port)
            .field("desktop_tls_spki_len", &self.desktop_tls_spki.len())
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

/// Decoded view borrowing directly from the scanned QR payload bytes.
///
/// The pairing secret borrows the caller's buffer, so the caller keeps
/// ownership of its zeroization. Every field has already passed the
/// version-one bounds when this view exists.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PairingQrView<'a> {
    /// Control protocol version the QR invitation was created for.
    pub protocol_version: u8,
    /// One-time 128-bit pairing identifier.
    pub pairing_id: [u8; 16],
    /// One-time 256-bit pairing secret borrowed from the input buffer.
    pub pairing_secret: &'a [u8],
    /// Desktop control endpoint host or IP literal.
    pub control_host: &'a str,
    /// Desktop control endpoint TCP port.
    pub control_port: u16,
    /// Exact DER desktop TLS `SubjectPublicKeyInfo` bytes to pin.
    pub desktop_tls_spki: &'a [u8],
    /// Monotonic expiry in milliseconds for the desktop pairing record.
    pub expires_at_ms: u64,
}

impl fmt::Debug for PairingQrView<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PairingQrView")
            .field("protocol_version", &self.protocol_version)
            .field("pairing_id", &"[REDACTED]")
            .field("pairing_secret", &"[REDACTED]")
            .field("control_host", &"[REDACTED]")
            .field("control_port", &self.control_port)
            .field("desktop_tls_spki_len", &self.desktop_tls_spki.len())
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

/// Parses an untrusted pairing QR payload without allocation.
///
/// # Errors
///
/// Returns [`QrPayloadError`] before any data is copied when the payload is
/// oversized, truncated, has a foreign magic or unsupported version, declares
/// out-of-range lengths, carries a zero port or expiry, has lengths that do
/// not match its total size, or carries a host that is not printable UTF-8.
pub fn parse_pairing_qr_payload(payload: &[u8]) -> Result<PairingQrView<'_>, QrPayloadError> {
    if payload.len() > MAX_QR_PAYLOAD_LEN {
        return Err(QrPayloadError::QrPayloadTooLarge);
    }
    if payload.len() < QR_PREFIX_LEN {
        return Err(QrPayloadError::Truncated);
    }
    if payload[0..4] != QR_PAYLOAD_MAGIC {
        return Err(QrPayloadError::InvalidMagic);
    }
    if payload[4] != QR_PAYLOAD_VERSION {
        return Err(QrPayloadError::UnsupportedQrVersion(payload[4]));
    }
    let protocol_version = payload[5];
    if u32::from(protocol_version) != CONTROL_PROTOCOL_VERSION {
        return Err(QrPayloadError::InvalidProtocolVersion(protocol_version));
    }
    let host_len = usize::from(payload[QR_HOST_LEN_OFFSET]);
    if host_len == 0 {
        return Err(QrPayloadError::InvalidHostLength);
    }
    let control_port = u16::from_be_bytes([payload[QR_PORT_OFFSET], payload[QR_PORT_OFFSET + 1]]);
    if control_port == 0 {
        return Err(QrPayloadError::InvalidPort);
    }
    let spki_len = usize::from(u16::from_be_bytes([
        payload[QR_SPKI_LEN_OFFSET],
        payload[QR_SPKI_LEN_OFFSET + 1],
    ]));
    if !(QR_SPKI_MIN_LEN..=QR_SPKI_MAX_LEN).contains(&spki_len) {
        return Err(QrPayloadError::InvalidSpkiLength);
    }
    let expires_at_ms = u64::from_be_bytes([
        payload[QR_EXPIRY_OFFSET],
        payload[QR_EXPIRY_OFFSET + 1],
        payload[QR_EXPIRY_OFFSET + 2],
        payload[QR_EXPIRY_OFFSET + 3],
        payload[QR_EXPIRY_OFFSET + 4],
        payload[QR_EXPIRY_OFFSET + 5],
        payload[QR_EXPIRY_OFFSET + 6],
        payload[QR_EXPIRY_OFFSET + 7],
    ]);
    if expires_at_ms == 0 {
        return Err(QrPayloadError::InvalidExpiry);
    }
    if QR_HOST_OFFSET + host_len + spki_len != payload.len() {
        return Err(QrPayloadError::LengthMismatch);
    }

    let host_bytes = payload
        .get(QR_HOST_OFFSET..QR_HOST_OFFSET + host_len)
        .ok_or(QrPayloadError::LengthMismatch)?;
    let control_host = core::str::from_utf8(host_bytes)
        .ok()
        .filter(|host| is_printable_host(host))
        .ok_or(QrPayloadError::InvalidHostEncoding)?;
    let desktop_tls_spki = payload
        .get(QR_HOST_OFFSET + host_len..QR_HOST_OFFSET + host_len + spki_len)
        .ok_or(QrPayloadError::LengthMismatch)?;

    let mut pairing_id = [0_u8; 16];
    pairing_id.copy_from_slice(
        payload
            .get(QR_PAIRING_ID_OFFSET..QR_PAIRING_ID_OFFSET + 16)
            .ok_or(QrPayloadError::Truncated)?,
    );
    let pairing_secret = payload
        .get(QR_PAIRING_SECRET_OFFSET..QR_PAIRING_SECRET_OFFSET + 32)
        .ok_or(QrPayloadError::Truncated)?;

    Ok(PairingQrView {
        protocol_version,
        pairing_id,
        pairing_secret,
        control_host,
        control_port,
        desktop_tls_spki,
        expires_at_ms,
    })
}

/// Returns whether the host is printable text without control characters.
///
/// A QR host is presentation data on both ends; rejecting C0 controls and
/// DEL keeps the decoded endpoint safe to display and to log in redacted
/// diagnostics without a separate sanitization pass.
fn is_printable_host(host: &str) -> bool {
    host.bytes().all(|byte| (0x20..0x7F).contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    const VECTOR_ENCODED_HEX: &str = "534d5152010114b9fa0030000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f00000000001e84806465736b746f702d30312e6c61622e6c6f63616c404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f";

    const VECTOR_HOST: &str = "desktop-01.lab.local";
    const VECTOR_PORT: u16 = 47_610;
    const VECTOR_EXPIRY_MS: u64 = 2_000_000;

    const VECTOR_PAIRING_ID: [u8; 16] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E,
        0x0F,
    ];

    const VECTOR_PAIRING_SECRET: [u8; 32] = [
        0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E,
        0x1F, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x2B, 0x2C, 0x2D,
        0x2E, 0x2F,
    ];

    fn vector_spki() -> Vec<u8> {
        (0x40..0x70).collect()
    }

    fn vector_payload() -> PairingQrPayload<'static> {
        PairingQrPayload {
            protocol_version: 1,
            pairing_id: VECTOR_PAIRING_ID,
            pairing_secret: VECTOR_PAIRING_SECRET,
            control_host: VECTOR_HOST,
            control_port: VECTOR_PORT,
            desktop_tls_spki: Box::leak(vector_spki().into_boxed_slice()),
            expires_at_ms: VECTOR_EXPIRY_MS,
        }
    }

    fn unhex(value: &str) -> Vec<u8> {
        (0..value.len() / 2)
            .map(|index| u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn matches_published_pairing_qr_vector() {
        let payload = vector_payload();
        let expected = unhex(VECTOR_ENCODED_HEX);
        assert_eq!(payload.encoded_len(), Some(expected.len()));

        let mut encoded = [0_u8; MAX_QR_PAYLOAD_LEN];
        let written = payload.encode_into(&mut encoded).unwrap();
        assert_eq!(written, expected.len());
        assert_eq!(&encoded[..written], expected.as_slice());
    }

    #[test]
    fn decode_round_trips_the_published_vector() {
        let encoded = unhex(VECTOR_ENCODED_HEX);
        let view = parse_pairing_qr_payload(&encoded).unwrap();
        assert_eq!(view.protocol_version, 1);
        assert_eq!(view.pairing_id, VECTOR_PAIRING_ID);
        assert_eq!(view.pairing_secret, VECTOR_PAIRING_SECRET.as_slice());
        assert_eq!(view.control_host, VECTOR_HOST);
        assert_eq!(view.control_port, VECTOR_PORT);
        assert_eq!(view.desktop_tls_spki, vector_spki().as_slice());
        assert_eq!(view.expires_at_ms, VECTOR_EXPIRY_MS);

        let payload = PairingQrPayload {
            protocol_version: view.protocol_version,
            pairing_id: view.pairing_id,
            pairing_secret: VECTOR_PAIRING_SECRET,
            control_host: view.control_host,
            control_port: view.control_port,
            desktop_tls_spki: view.desktop_tls_spki,
            expires_at_ms: view.expires_at_ms,
        };
        let mut reencoded = [0_u8; MAX_QR_PAYLOAD_LEN];
        let written = payload.encode_into(&mut reencoded).unwrap();
        assert_eq!(&reencoded[..written], encoded.as_slice());
    }

    #[test]
    fn encode_rejects_a_short_output_buffer() {
        let payload = vector_payload();
        let encoded_len = payload.encoded_len().unwrap();
        let mut out = vec![0_u8; encoded_len - 1];
        assert_eq!(
            payload.encode_into(&mut out),
            Err(QrPayloadError::QrPayloadTooLarge)
        );
    }

    #[test]
    fn encode_rejects_out_of_range_fields() {
        let mut payload = vector_payload();
        payload.protocol_version = 2;
        assert!(matches!(
            payload.encode_into(&mut [0_u8; MAX_QR_PAYLOAD_LEN]),
            Err(QrPayloadError::InvalidProtocolVersion(2))
        ));

        let mut payload = vector_payload();
        payload.control_host = "";
        assert_eq!(
            payload.encode_into(&mut [0_u8; MAX_QR_PAYLOAD_LEN]),
            Err(QrPayloadError::InvalidHostLength)
        );

        let long_host = "a".repeat(QR_HOST_MAX_LEN + 1);
        let mut payload = vector_payload();
        payload.control_host = long_host.as_str();
        assert_eq!(
            payload.encode_into(&mut [0_u8; MAX_QR_PAYLOAD_LEN]),
            Err(QrPayloadError::InvalidHostLength)
        );

        let mut payload = vector_payload();
        payload.control_host = "desktop\u{7f}.lab.local";
        assert_eq!(
            payload.encode_into(&mut [0_u8; MAX_QR_PAYLOAD_LEN]),
            Err(QrPayloadError::InvalidHostEncoding)
        );

        let mut payload = vector_payload();
        payload.control_port = 0;
        assert_eq!(
            payload.encode_into(&mut [0_u8; MAX_QR_PAYLOAD_LEN]),
            Err(QrPayloadError::InvalidPort)
        );

        let short_spki = vec![0x30_u8; QR_SPKI_MIN_LEN - 1];
        let mut payload = vector_payload();
        payload.desktop_tls_spki = short_spki.as_slice();
        assert_eq!(
            payload.encode_into(&mut [0_u8; MAX_QR_PAYLOAD_LEN]),
            Err(QrPayloadError::InvalidSpkiLength)
        );

        let long_spki = vec![0x30_u8; QR_SPKI_MAX_LEN + 1];
        let mut payload = vector_payload();
        payload.desktop_tls_spki = long_spki.as_slice();
        assert_eq!(
            payload.encode_into(&mut [0_u8; MAX_QR_PAYLOAD_LEN]),
            Err(QrPayloadError::InvalidSpkiLength)
        );

        let mut payload = vector_payload();
        payload.expires_at_ms = 0;
        assert_eq!(
            payload.encode_into(&mut [0_u8; MAX_QR_PAYLOAD_LEN]),
            Err(QrPayloadError::InvalidExpiry)
        );
    }

    #[test]
    fn decode_rejects_oversized_and_truncated_payloads() {
        let oversized = vec![0_u8; MAX_QR_PAYLOAD_LEN + 1];
        assert_eq!(
            parse_pairing_qr_payload(&oversized),
            Err(QrPayloadError::QrPayloadTooLarge)
        );
        let truncated = vec![0_u8; QR_PREFIX_LEN - 1];
        assert_eq!(
            parse_pairing_qr_payload(&truncated),
            Err(QrPayloadError::Truncated)
        );
    }

    #[test]
    fn decode_rejects_foreign_magic_and_unsupported_versions() {
        let mut encoded = unhex(VECTOR_ENCODED_HEX);
        encoded[0] = b'X';
        assert_eq!(
            parse_pairing_qr_payload(&encoded),
            Err(QrPayloadError::InvalidMagic)
        );

        let mut encoded = unhex(VECTOR_ENCODED_HEX);
        encoded[4] = 2;
        assert_eq!(
            parse_pairing_qr_payload(&encoded),
            Err(QrPayloadError::UnsupportedQrVersion(2))
        );

        let mut encoded = unhex(VECTOR_ENCODED_HEX);
        encoded[5] = 2;
        assert_eq!(
            parse_pairing_qr_payload(&encoded),
            Err(QrPayloadError::InvalidProtocolVersion(2))
        );
    }

    #[test]
    fn decode_rejects_invalid_declared_fields() {
        let mut encoded = unhex(VECTOR_ENCODED_HEX);
        encoded[QR_HOST_LEN_OFFSET] = 0;
        assert_eq!(
            parse_pairing_qr_payload(&encoded),
            Err(QrPayloadError::InvalidHostLength)
        );

        let mut encoded = unhex(VECTOR_ENCODED_HEX);
        encoded[QR_PORT_OFFSET] = 0;
        encoded[QR_PORT_OFFSET + 1] = 0;
        assert_eq!(
            parse_pairing_qr_payload(&encoded),
            Err(QrPayloadError::InvalidPort)
        );

        let mut encoded = unhex(VECTOR_ENCODED_HEX);
        let short_spki = u16::try_from(QR_SPKI_MIN_LEN - 1).unwrap();
        let [spki_high, spki_low] = short_spki.to_be_bytes();
        encoded[QR_SPKI_LEN_OFFSET] = spki_high;
        encoded[QR_SPKI_LEN_OFFSET + 1] = spki_low;
        assert_eq!(
            parse_pairing_qr_payload(&encoded),
            Err(QrPayloadError::InvalidSpkiLength)
        );

        let mut encoded = unhex(VECTOR_ENCODED_HEX);
        let [declared_high, declared_low] =
            u16::try_from(QR_SPKI_MAX_LEN + 1).unwrap().to_be_bytes();
        encoded[QR_SPKI_LEN_OFFSET] = declared_high;
        encoded[QR_SPKI_LEN_OFFSET + 1] = declared_low;
        assert_eq!(
            parse_pairing_qr_payload(&encoded),
            Err(QrPayloadError::InvalidSpkiLength)
        );

        let mut encoded = unhex(VECTOR_ENCODED_HEX);
        for byte in &mut encoded[QR_EXPIRY_OFFSET..QR_EXPIRY_OFFSET + 8] {
            *byte = 0;
        }
        assert_eq!(
            parse_pairing_qr_payload(&encoded),
            Err(QrPayloadError::InvalidExpiry)
        );
    }

    #[test]
    fn decode_rejects_length_mismatch_and_non_utf8_host() {
        let mut encoded = unhex(VECTOR_ENCODED_HEX);
        encoded[QR_HOST_LEN_OFFSET] += 1;
        assert_eq!(
            parse_pairing_qr_payload(&encoded),
            Err(QrPayloadError::LengthMismatch)
        );

        let mut encoded = unhex(VECTOR_ENCODED_HEX);
        let host_start = QR_HOST_OFFSET;
        let host_len = encoded[QR_HOST_LEN_OFFSET] as usize;
        encoded[host_start] = 0xFF;
        let _ = host_len;
        assert_eq!(
            parse_pairing_qr_payload(&encoded),
            Err(QrPayloadError::InvalidHostEncoding)
        );

        let mut encoded = unhex(VECTOR_ENCODED_HEX);
        let last = encoded.len() - 1;
        encoded.truncate(last);
        assert_eq!(
            parse_pairing_qr_payload(&encoded),
            Err(QrPayloadError::LengthMismatch)
        );
    }

    #[test]
    fn decode_accepts_boundary_lengths() {
        let short_spki = vec![0x30_u8; QR_SPKI_MIN_LEN];
        let payload = PairingQrPayload {
            protocol_version: 1,
            pairing_id: VECTOR_PAIRING_ID,
            pairing_secret: VECTOR_PAIRING_SECRET,
            control_host: "a",
            control_port: 1,
            desktop_tls_spki: short_spki.as_slice(),
            expires_at_ms: 1,
        };
        let mut encoded = [0_u8; MAX_QR_PAYLOAD_LEN];
        let written = payload.encode_into(&mut encoded).unwrap();
        assert_eq!(written, QR_PREFIX_LEN + 1 + QR_SPKI_MIN_LEN);
        let view = parse_pairing_qr_payload(&encoded[..written]).unwrap();
        assert_eq!(view.control_host, "a");
        assert_eq!(view.desktop_tls_spki.len(), QR_SPKI_MIN_LEN);

        let long_host = "a".repeat(QR_HOST_MAX_LEN);
        let long_spki = vec![0x30_u8; QR_SPKI_MAX_LEN];
        let payload = PairingQrPayload {
            control_host: long_host.as_str(),
            desktop_tls_spki: long_spki.as_slice(),
            ..payload
        };
        let written = payload.encode_into(&mut encoded).unwrap();
        assert_eq!(written, MAX_QR_PAYLOAD_LEN);
        let view = parse_pairing_qr_payload(&encoded).unwrap();
        assert_eq!(view.control_host.len(), QR_HOST_MAX_LEN);
        assert_eq!(view.desktop_tls_spki.len(), QR_SPKI_MAX_LEN);
    }

    #[test]
    fn debug_output_is_redacted() {
        let payload = vector_payload();
        let debug = format!("{payload:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("desktop-01"));
        assert!(!debug.contains("1011"));

        let encoded = unhex(VECTOR_ENCODED_HEX);
        let view = parse_pairing_qr_payload(&encoded).unwrap();
        let debug = format!("{view:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("desktop-01"));
    }

    #[test]
    fn error_display_is_actionable() {
        assert_eq!(
            QrPayloadError::QrPayloadTooLarge.to_string(),
            "pairing QR payload exceeds the hard limit"
        );
        assert_eq!(
            QrPayloadError::LengthMismatch.to_string(),
            "pairing QR payload length does not match its lengths"
        );
        assert_eq!(
            QrPayloadError::InvalidHostEncoding.to_string(),
            "pairing QR host is not UTF-8 text without control characters"
        );
    }
}
