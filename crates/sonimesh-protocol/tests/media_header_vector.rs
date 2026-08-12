//! Wire-format boundary tests for the fixed version-one media header.
//!
//! `MediaHeader::encode` produces the canonical 36-byte header consumed by the
//! AEAD ingress as associated data. This file pins the published wire vector
//! and proves a full datagram round-trips through [`parse_media_packet`]
//! without reordering or truncation.

use sonimesh_protocol::{
    MAX_ENCRYPTED_PAYLOAD_LEN, MEDIA_HEADER_LEN, MEDIA_MAGIC, MEDIA_VERSION, MediaHeader,
    PacketParseError, parse_media_packet,
};

fn published_header() -> MediaHeader {
    MediaHeader {
        flags: 0,
        session_id: 0x0102_0304_0506_0708,
        stream_id: 9,
        sequence: 7,
        sample_timestamp: 2_880,
        key_epoch: 0x2345,
    }
}

/// The canonical big-endian wire layout is pinned byte-for-byte so a schema
/// or ordering change is caught, including by the cross-language verifier.
#[test]
fn matches_published_media_header_vector() {
    let encoded = published_header().encode(64).unwrap();

    assert_eq!(
        encoded,
        [
            0x53, 0x4E, 0x4D, 0x53, // MEDIA_MAGIC "SNMS"
            0x01, // MEDIA_VERSION
            0x00, // flags
            0x00, 0x24, // header length 36
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, // session_id
            0x00, 0x00, 0x00, 0x09, // stream_id
            0x00, 0x00, 0x00, 0x07, // sequence
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0B, 0x40, // sample_timestamp 2880
            0x00, 0x40, // payload length 64
            0x23, 0x45, // key_epoch 0x2345
        ]
    );
    assert_eq!(encoded.len(), MEDIA_HEADER_LEN);
}

/// A header followed by its encrypted payload must parse back to the exact
/// fields, the full authenticated header, and the untouched ciphertext.
#[test]
fn round_trips_through_parse_media_packet() {
    let payload = vec![0xA5_u8; 64];
    let mut datagram = published_header().encode(payload.len()).unwrap().to_vec();
    datagram.extend_from_slice(&payload);

    let packet = parse_media_packet(&datagram).unwrap();
    assert_eq!(packet.header, published_header());
    assert_eq!(packet.authenticated_header.len(), MEDIA_HEADER_LEN);
    assert_eq!(
        packet.authenticated_header,
        &datagram[..MEDIA_HEADER_LEN],
        "the complete header is authenticated"
    );
    assert_eq!(packet.ciphertext, &payload[..]);
}

/// The payload bound is the datagram limit minus the fixed header.
#[test]
fn rejects_payloads_beyond_the_encrypted_bound() {
    assert!(matches!(
        published_header().encode(MAX_ENCRYPTED_PAYLOAD_LEN + 1),
        Err(PacketParseError::PayloadTooLarge)
    ));
    assert!(published_header().encode(MAX_ENCRYPTED_PAYLOAD_LEN).is_ok());
}

/// The published header layout matches the documented constants exactly.
#[test]
fn published_layout_matches_documented_constants() {
    let encoded = published_header().encode(0).unwrap();
    assert_eq!(&encoded[0..4], &MEDIA_MAGIC.to_be_bytes());
    assert_eq!(encoded[4], MEDIA_VERSION);
    assert_eq!(
        &encoded[6..8],
        &u16::try_from(MEDIA_HEADER_LEN).unwrap().to_be_bytes()
    );
}
