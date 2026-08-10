use core::fmt;

/// Fixed portion of the version-one media header.
pub const MEDIA_HEADER_LEN: usize = 36;
const MEDIA_HEADER_LEN_U16: u16 = 36;
/// Largest accepted header, including negotiated extensions.
pub const MAX_HEADER_LEN: usize = 128;
/// Maximum complete media datagram to avoid relying on IP fragmentation.
pub const MAX_MEDIA_DATAGRAM_LEN: usize = 1_200;
/// Maximum encrypted payload with a fixed header, including authentication tag.
pub const MAX_ENCRYPTED_PAYLOAD_LEN: usize = MAX_MEDIA_DATAGRAM_LEN - MEDIA_HEADER_LEN;
/// Packet-family marker (`SNMS` in network byte order).
pub const MEDIA_MAGIC: u32 = 0x534E_4D53;
/// Current data-plane protocol version.
pub const MEDIA_VERSION: u8 = 1;

/// Parsed fixed fields from a SoniMesh media packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaHeader {
    /// Negotiated packet flags.
    pub flags: u8,
    /// Random identifier for the current media session.
    pub session_id: u64,
    /// Stream identifier within a media session.
    pub stream_id: u32,
    /// Monotonic packet sequence within a key epoch.
    pub sequence: u32,
    /// Sender sample position.
    pub sample_timestamp: u64,
    /// Explicit key-rotation generation.
    pub key_epoch: u16,
}

impl MediaHeader {
    /// Encodes the fixed version-one header for an encrypted payload.
    ///
    /// # Errors
    ///
    /// Returns [`PacketParseError::PayloadTooLarge`] when the encrypted payload
    /// exceeds the datagram limit.
    pub fn encode(self, payload_len: usize) -> Result<[u8; MEDIA_HEADER_LEN], PacketParseError> {
        let payload_len = u16::try_from(payload_len)
            .ok()
            .filter(|length| usize::from(*length) <= MAX_ENCRYPTED_PAYLOAD_LEN)
            .ok_or(PacketParseError::PayloadTooLarge)?;

        let mut bytes = [0_u8; MEDIA_HEADER_LEN];
        bytes[0..4].copy_from_slice(&MEDIA_MAGIC.to_be_bytes());
        bytes[4] = MEDIA_VERSION;
        bytes[5] = self.flags;
        bytes[6..8].copy_from_slice(&MEDIA_HEADER_LEN_U16.to_be_bytes());
        bytes[8..16].copy_from_slice(&self.session_id.to_be_bytes());
        bytes[16..20].copy_from_slice(&self.stream_id.to_be_bytes());
        bytes[20..24].copy_from_slice(&self.sequence.to_be_bytes());
        bytes[24..32].copy_from_slice(&self.sample_timestamp.to_be_bytes());
        bytes[32..34].copy_from_slice(&payload_len.to_be_bytes());
        bytes[34..36].copy_from_slice(&self.key_epoch.to_be_bytes());
        Ok(bytes)
    }
}

/// A bounded packet view borrowing directly from the received datagram.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct MediaPacket<'a> {
    /// Parsed fixed fields.
    pub header: MediaHeader,
    /// Complete header, including extensions, used as AEAD associated data.
    pub authenticated_header: &'a [u8],
    /// Encrypted payload, including its authentication tag.
    pub ciphertext: &'a [u8],
}

impl fmt::Debug for MediaPacket<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MediaPacket")
            .field("header", &self.header)
            .field("authenticated_header_len", &self.authenticated_header.len())
            .field("ciphertext_len", &self.ciphertext.len())
            .finish()
    }
}

/// Failure returned before authentication or allocation is attempted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PacketParseError {
    /// Datagram cannot contain a complete fixed header.
    Truncated,
    /// Packet marker belongs to another protocol.
    InvalidMagic,
    /// Data-plane version is unsupported.
    UnsupportedVersion(u8),
    /// Header length is smaller than the fixed header or exceeds the hard limit.
    InvalidHeaderLength,
    /// Encrypted payload exceeds the hard limit.
    PayloadTooLarge,
    /// Complete datagram exceeds the no-fragmentation design limit.
    DatagramTooLarge,
    /// Declared header and payload lengths do not match the datagram.
    LengthMismatch,
}

impl fmt::Display for PacketParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => formatter.write_str("media datagram is truncated"),
            Self::InvalidMagic => formatter.write_str("media datagram has an invalid magic value"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported media protocol version {version}")
            }
            Self::InvalidHeaderLength => formatter.write_str("media header length is invalid"),
            Self::PayloadTooLarge => formatter.write_str("media payload exceeds the hard limit"),
            Self::DatagramTooLarge => formatter.write_str("media datagram exceeds the hard limit"),
            Self::LengthMismatch => {
                formatter.write_str("media datagram length does not match header")
            }
        }
    }
}

impl std::error::Error for PacketParseError {}

/// Parses an untrusted media datagram without allocation.
///
/// # Errors
///
/// Returns [`PacketParseError`] for malformed, unsupported, oversized, or
/// inconsistent datagrams.
pub fn parse_media_packet(datagram: &[u8]) -> Result<MediaPacket<'_>, PacketParseError> {
    if datagram.len() > MAX_MEDIA_DATAGRAM_LEN {
        return Err(PacketParseError::DatagramTooLarge);
    }
    if datagram.len() < MEDIA_HEADER_LEN {
        return Err(PacketParseError::Truncated);
    }
    if read_u32(datagram, 0) != MEDIA_MAGIC {
        return Err(PacketParseError::InvalidMagic);
    }
    if datagram[4] != MEDIA_VERSION {
        return Err(PacketParseError::UnsupportedVersion(datagram[4]));
    }

    let header_len = usize::from(read_u16(datagram, 6));
    if !(MEDIA_HEADER_LEN..=MAX_HEADER_LEN).contains(&header_len) {
        return Err(PacketParseError::InvalidHeaderLength);
    }
    let payload_len = usize::from(read_u16(datagram, 32));
    if payload_len > MAX_ENCRYPTED_PAYLOAD_LEN {
        return Err(PacketParseError::PayloadTooLarge);
    }
    if header_len.checked_add(payload_len) != Some(datagram.len()) {
        return Err(PacketParseError::LengthMismatch);
    }

    Ok(MediaPacket {
        header: MediaHeader {
            flags: datagram[5],
            session_id: read_u64(datagram, 8),
            stream_id: read_u32(datagram, 16),
            sequence: read_u32(datagram, 20),
            sample_timestamp: read_u64(datagram, 24),
            key_epoch: read_u16(datagram, 34),
        },
        authenticated_header: &datagram[..header_len],
        ciphertext: &datagram[header_len..],
    })
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("checked packet slice"),
    )
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_be_bytes(
        bytes[offset..offset + 8]
            .try_into()
            .expect("checked packet slice"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> MediaHeader {
        MediaHeader {
            flags: 2,
            session_id: 7,
            stream_id: 11,
            sequence: 42,
            sample_timestamp: 960,
            key_epoch: 3,
        }
    }

    #[test]
    fn packet_debug_omits_authenticated_header_and_ciphertext_bytes() {
        let packet = MediaPacket {
            header: header(),
            authenticated_header: b"authenticated header",
            ciphertext: b"encrypted packet bytes",
        };

        let debug = format!("{packet:?}");

        assert!(debug.contains("authenticated_header_len: 20"));
        assert!(debug.contains("ciphertext_len: 22"));
        assert!(!debug.contains("authenticated_header:"));
        assert!(!debug.contains("ciphertext:"));
        assert!(!debug.contains("101, 110, 99"));
    }

    #[test]
    fn round_trips_fixed_header() {
        let encoded = header().encode(4).unwrap();
        let mut datagram = encoded.to_vec();
        datagram.extend_from_slice(&[1, 2, 3, 4]);

        let packet = parse_media_packet(&datagram).unwrap();

        assert_eq!(packet.header, header());
        assert_eq!(packet.authenticated_header, encoded);
        assert_eq!(packet.ciphertext, [1, 2, 3, 4]);
    }

    #[test]
    fn preserves_extension_bytes_as_authenticated_data() {
        let encoded = header().encode(2).unwrap();
        let mut datagram = encoded.to_vec();
        datagram[6..8].copy_from_slice(&40_u16.to_be_bytes());
        datagram.extend_from_slice(&[9, 8, 7, 6]);
        datagram.extend_from_slice(&[1, 2]);

        let packet = parse_media_packet(&datagram).unwrap();

        assert_eq!(packet.authenticated_header.len(), 40);
        assert_eq!(&packet.authenticated_header[36..], &[9, 8, 7, 6]);
    }

    #[test]
    fn rejects_declared_length_mismatch() {
        let encoded = header().encode(3).unwrap();
        let mut datagram = encoded.to_vec();
        datagram.extend_from_slice(&[1, 2]);

        assert_eq!(
            parse_media_packet(&datagram),
            Err(PacketParseError::LengthMismatch)
        );
    }

    #[test]
    fn rejects_oversized_payload_before_encoding() {
        assert_eq!(
            header().encode(MAX_ENCRYPTED_PAYLOAD_LEN + 1),
            Err(PacketParseError::PayloadTooLarge)
        );
    }

    #[test]
    fn rejects_complete_datagram_over_fragmentation_limit() {
        let mut datagram = header().encode(MAX_ENCRYPTED_PAYLOAD_LEN).unwrap().to_vec();
        datagram.extend(std::iter::repeat_n(0, MAX_ENCRYPTED_PAYLOAD_LEN));
        datagram[6..8].copy_from_slice(&37_u16.to_be_bytes());
        datagram.insert(MEDIA_HEADER_LEN, 0);

        assert_eq!(
            parse_media_packet(&datagram),
            Err(PacketParseError::DatagramTooLarge)
        );
    }
    #[test]
    fn accepts_a_complete_datagram_at_the_fragmentation_limit() {
        let encoded = header().encode(MAX_ENCRYPTED_PAYLOAD_LEN).unwrap();
        assert_eq!(encoded.len(), MEDIA_HEADER_LEN);
        let mut datagram = encoded.to_vec();
        datagram.extend(std::iter::repeat_n(0, MAX_ENCRYPTED_PAYLOAD_LEN));
        assert_eq!(datagram.len(), MAX_MEDIA_DATAGRAM_LEN);

        let packet = parse_media_packet(&datagram).unwrap();

        assert_eq!(packet.authenticated_header.len(), MEDIA_HEADER_LEN);
        assert_eq!(packet.ciphertext.len(), MAX_ENCRYPTED_PAYLOAD_LEN);
    }

    #[test]
    fn accepts_the_maximum_extended_header_as_authenticated_data() {
        let encoded = header().encode(2).unwrap();
        let mut datagram = encoded.to_vec();
        datagram[6..8].copy_from_slice(&u16::try_from(MAX_HEADER_LEN).unwrap().to_be_bytes());
        datagram.extend(std::iter::repeat_n(0xA5, MAX_HEADER_LEN - MEDIA_HEADER_LEN));
        datagram.extend_from_slice(&[1, 2]);

        let packet = parse_media_packet(&datagram).unwrap();

        assert_eq!(packet.authenticated_header.len(), MAX_HEADER_LEN);
        assert_eq!(
            &packet.authenticated_header[..MEDIA_HEADER_LEN],
            &datagram[..MEDIA_HEADER_LEN]
        );
        assert!(
            packet.authenticated_header[MEDIA_HEADER_LEN..]
                .iter()
                .all(|byte| *byte == 0xA5)
        );
        assert_eq!(packet.ciphertext, [1, 2]);
    }

    #[test]
    fn arbitrary_short_inputs_never_panic() {
        let mut state = 0xA5A5_2026_u64;
        for length in 0..=MAX_MEDIA_DATAGRAM_LEN + 8 {
            let mut bytes = vec![0_u8; length];
            for byte in &mut bytes {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                *byte = state.to_le_bytes()[4];
            }
            let _ = parse_media_packet(&bytes);
        }
    }

    #[test]
    fn round_trips_extreme_field_values() {
        let header = MediaHeader {
            flags: 0xFF,
            session_id: u64::MAX,
            stream_id: u32::MAX,
            sequence: u32::MAX,
            sample_timestamp: u64::MAX,
            key_epoch: u16::MAX,
        };
        let encoded = header.encode(MAX_ENCRYPTED_PAYLOAD_LEN).unwrap();
        let mut datagram = encoded.to_vec();
        datagram.extend(std::iter::repeat_n(0x5A, MAX_ENCRYPTED_PAYLOAD_LEN));

        let packet = parse_media_packet(&datagram).unwrap();

        assert_eq!(packet.header, header);
        assert_eq!(packet.ciphertext.len(), MAX_ENCRYPTED_PAYLOAD_LEN);
    }

    #[test]
    fn round_trips_zero_fields_and_empty_payload() {
        let header = MediaHeader {
            flags: 0,
            session_id: 0,
            stream_id: 0,
            sequence: 0,
            sample_timestamp: 0,
            key_epoch: 0,
        };
        let encoded = header.encode(0).unwrap();

        let packet = parse_media_packet(&encoded).unwrap();

        assert_eq!(packet.header, header);
        assert!(packet.ciphertext.is_empty());
    }

    #[test]
    fn round_trips_every_payload_length() {
        for payload_len in 0..=MAX_ENCRYPTED_PAYLOAD_LEN {
            let encoded = header().encode(payload_len).unwrap();
            let mut datagram = encoded.to_vec();
            datagram.extend(std::iter::repeat_n(0x11, payload_len));

            let packet = parse_media_packet(&datagram).unwrap();

            assert_eq!(packet.header, header());
            assert_eq!(packet.ciphertext.len(), payload_len);
        }
    }

    #[test]
    fn rejects_bad_magic_and_unsupported_version() {
        let mut datagram = header().encode(0).unwrap();
        datagram[0] = 0;
        assert_eq!(
            parse_media_packet(&datagram),
            Err(PacketParseError::InvalidMagic)
        );

        let mut datagram = header().encode(0).unwrap();
        datagram[4] = 2;
        assert_eq!(
            parse_media_packet(&datagram),
            Err(PacketParseError::UnsupportedVersion(2))
        );
    }

    #[test]
    fn rejects_header_length_outside_the_bounded_range() {
        let mut below = header().encode(0).unwrap();
        below[6..8].copy_from_slice(
            &u16::try_from(MEDIA_HEADER_LEN - 1)
                .expect("limit fits u16")
                .to_be_bytes(),
        );
        assert_eq!(
            parse_media_packet(&below),
            Err(PacketParseError::InvalidHeaderLength)
        );

        let mut above = header().encode(0).unwrap();
        above[6..8].copy_from_slice(
            &u16::try_from(MAX_HEADER_LEN + 1)
                .expect("limit fits u16")
                .to_be_bytes(),
        );
        assert_eq!(
            parse_media_packet(&above),
            Err(PacketParseError::InvalidHeaderLength)
        );
    }

    #[test]
    fn rejects_payload_over_the_hard_limit_on_parse() {
        // Keep the datagram at the fragmentation limit so the oversized
        // declared payload, not the total size, triggers the rejection.
        let mut datagram = header().encode(0).unwrap().to_vec();
        datagram[32..34].copy_from_slice(
            &u16::try_from(MAX_ENCRYPTED_PAYLOAD_LEN + 1)
                .expect("limit fits u16")
                .to_be_bytes(),
        );
        datagram.extend(std::iter::repeat_n(
            0,
            MAX_MEDIA_DATAGRAM_LEN - MEDIA_HEADER_LEN,
        ));
        assert_eq!(datagram.len(), MAX_MEDIA_DATAGRAM_LEN);
        assert_eq!(
            parse_media_packet(&datagram),
            Err(PacketParseError::PayloadTooLarge)
        );
    }

    #[test]
    fn rejects_truncated_datagrams_below_the_fixed_header() {
        for len in 0..MEDIA_HEADER_LEN {
            assert_eq!(
                parse_media_packet(&vec![0; len]),
                Err(PacketParseError::Truncated)
            );
        }
    }
}
