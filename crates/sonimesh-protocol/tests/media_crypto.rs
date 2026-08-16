//! AEAD, context, and caller-owned-output tests for protected media packets.

use sonimesh_protocol::{
    MAX_MEDIA_DATAGRAM_LEN, MAX_MEDIA_PLAINTEXT_LEN, MEDIA_HEADER_LEN, MediaCryptoError,
    MediaDirection, MediaKeyContext, MediaOpener, MediaSealer, SessionSecret, parse_media_packet,
};

fn context(stream_id: u32, direction: MediaDirection) -> MediaKeyContext {
    MediaKeyContext::new(0xA1B2_C3D4_E5F6_1020, stream_id, 2, direction).unwrap()
}

fn secret() -> SessionSecret {
    SessionSecret::new([0x5A; 32])
}

#[test]
fn seals_and_opens_with_full_header_authentication() {
    let secret = secret();
    let context = context(7, MediaDirection::AndroidToDesktop);
    let mut sealer = MediaSealer::new(&secret, context).unwrap();
    let opener = MediaOpener::new(&secret, context).unwrap();
    let protected = sealer.seal(0x03, 9_600, b"opus-like test payload").unwrap();
    let packet = parse_media_packet(&protected.datagram).unwrap();
    let mut plaintext = [0_u8; 64];

    let length = opener.open(packet, &mut plaintext).unwrap();

    assert_eq!(&plaintext[..length], b"opus-like test payload");
    assert_eq!(protected.header.sequence, 0);
}

#[test]
fn rejects_tampered_header_and_ciphertext_without_plaintext() {
    let secret = secret();
    let context = context(7, MediaDirection::AndroidToDesktop);
    let mut sealer = MediaSealer::new(&secret, context).unwrap();
    let opener = MediaOpener::new(&secret, context).unwrap();
    let protected = sealer.seal(0, 0, b"secret audio").unwrap();

    let mut header_tampered = protected.datagram.clone();
    header_tampered[5] ^= 0x01;
    let mut plaintext = [0xFF; 32];
    assert_eq!(
        opener.open(
            parse_media_packet(&header_tampered).unwrap(),
            &mut plaintext
        ),
        Err(MediaCryptoError::AuthenticationFailed)
    );
    assert_eq!(&plaintext[..12], [0; 12]);
    assert_eq!(&plaintext[12..], [0xFF; 20]);

    let mut ciphertext_tampered = protected.datagram;
    let last = ciphertext_tampered.len() - 1;
    ciphertext_tampered[last] ^= 0x80;
    plaintext.fill(0xFF);
    assert_eq!(
        opener.open(
            parse_media_packet(&ciphertext_tampered).unwrap(),
            &mut plaintext
        ),
        Err(MediaCryptoError::AuthenticationFailed)
    );
    assert_eq!(&plaintext[..12], [0; 12]);
    assert_eq!(&plaintext[12..], [0xFF; 20]);
}

#[test]
fn rejects_the_reverse_direction_without_leaking_ciphertext_to_output() {
    let secret = secret();
    let sender_context = context(7, MediaDirection::AndroidToDesktop);
    let mut sealer = MediaSealer::new(&secret, sender_context).unwrap();
    let protected = sealer.seal(0, 0, b"secret audio").unwrap();
    let reverse_direction =
        MediaOpener::new(&secret, context(7, MediaDirection::DesktopToAndroid)).unwrap();
    let mut plaintext = [0xCC; 32];

    assert_eq!(
        reverse_direction.open(
            parse_media_packet(&protected.datagram).unwrap(),
            &mut plaintext
        ),
        Err(MediaCryptoError::AuthenticationFailed)
    );
    assert_eq!(&plaintext[..12], [0; 12]);
    assert_eq!(&plaintext[12..], [0xCC; 20]);
}

#[test]
fn rejects_wrong_context_and_insufficient_output_before_decode() {
    let secret = secret();
    let sender_context = context(7, MediaDirection::AndroidToDesktop);
    let mut sealer = MediaSealer::new(&secret, sender_context).unwrap();
    let protected = sealer.seal(0, 0, b"longer payload").unwrap();
    let packet = parse_media_packet(&protected.datagram).unwrap();

    let wrong_context =
        MediaOpener::new(&secret, context(8, MediaDirection::AndroidToDesktop)).unwrap();
    let mut output = [0xAA; 32];
    assert_eq!(
        wrong_context.open(packet, &mut output),
        Err(MediaCryptoError::ContextMismatch)
    );
    assert_eq!(output, [0xAA; 32]);

    let opener = MediaOpener::new(&secret, sender_context).unwrap();
    let mut too_small = [0xBB; 2];
    assert_eq!(
        opener.open(
            parse_media_packet(&protected.datagram).unwrap(),
            &mut too_small
        ),
        Err(MediaCryptoError::OutputTooSmall)
    );
    assert_eq!(too_small, [0xBB; 2]);
}

#[test]
fn rejects_a_truncated_authentication_tag_before_plaintext() {
    let secret = secret();
    let context = context(7, MediaDirection::AndroidToDesktop);
    let mut sealer = MediaSealer::new(&secret, context).unwrap();
    let opener = MediaOpener::new(&secret, context).unwrap();
    let protected = sealer.seal(0, 0, b"secret audio").unwrap();

    // A peer can declare a short payload that still parses: the fixed header
    // plus five ciphertext bytes, with the length field rewritten to match.
    // The opener must reject with TruncatedTag before copying anything into
    // caller-owned plaintext.
    let mut datagram = protected.datagram;
    datagram.truncate(MEDIA_HEADER_LEN + 5);
    let short_payload_len = u16::try_from(datagram.len() - MEDIA_HEADER_LEN).unwrap();
    datagram[32..34].copy_from_slice(&short_payload_len.to_be_bytes());

    let mut plaintext = [0xA5_u8; 32];
    assert_eq!(
        opener.open(parse_media_packet(&datagram).unwrap(), &mut plaintext),
        Err(MediaCryptoError::TruncatedTag)
    );
    assert_eq!(plaintext, [0xA5_u8; 32]);
}

#[test]
fn rejects_plaintext_larger_than_media_capacity() {
    let secret = secret();
    let mut sealer =
        MediaSealer::new(&secret, context(7, MediaDirection::AndroidToDesktop)).unwrap();
    let oversized = vec![0_u8; MAX_MEDIA_PLAINTEXT_LEN + 1];

    assert_eq!(
        sealer.seal(0, 0, &oversized),
        Err(MediaCryptoError::PayloadTooLarge)
    );
    assert_eq!(
        sealer
            .seal(0, 0, b"valid after rejection")
            .unwrap()
            .header
            .sequence,
        0
    );
}

#[test]
fn seals_and_opens_an_empty_plaintext_with_a_tag_only_ciphertext() {
    let secret = secret();
    let context = context(7, MediaDirection::AndroidToDesktop);
    let mut sealer = MediaSealer::new(&secret, context).unwrap();
    let opener = MediaOpener::new(&secret, context).unwrap();
    let protected = sealer.seal(0, 0, b"").unwrap();

    // An empty plaintext still carries a full 16-byte Poly1305 tag, which sits
    // exactly on the truncated-tag threshold and must round-trip as zero bytes.
    let packet = parse_media_packet(&protected.datagram).unwrap();
    assert_eq!(packet.ciphertext.len(), 16);
    let mut plaintext = [0xA5_u8; 16];
    assert_eq!(opener.open(packet, &mut plaintext).unwrap(), 0);
    assert_eq!(plaintext, [0xA5_u8; 16]);
}

#[test]
fn seals_and_opens_at_the_exact_media_plaintext_capacity() {
    let secret = secret();
    let context = context(7, MediaDirection::AndroidToDesktop);
    let mut sealer = MediaSealer::new(&secret, context).unwrap();
    let opener = MediaOpener::new(&secret, context).unwrap();
    let plaintext = vec![0x7F_u8; MAX_MEDIA_PLAINTEXT_LEN];

    let protected = sealer.seal(0, 0, &plaintext).unwrap();

    // At the exact capacity the sealed datagram lands on the no-fragmentation
    // limit, so the bounded UDP receiver also accepts it.
    assert_eq!(protected.datagram.len(), MAX_MEDIA_DATAGRAM_LEN);
    let mut output = vec![0_u8; MAX_MEDIA_PLAINTEXT_LEN];
    let length = opener
        .open(
            parse_media_packet(&protected.datagram).unwrap(),
            &mut output,
        )
        .unwrap();

    assert_eq!(length, MAX_MEDIA_PLAINTEXT_LEN);
    assert_eq!(output, plaintext);
}

#[test]
fn matches_published_cross_language_media_vector() {
    let secret = SessionSecret::new([
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E,
        0x0F, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D,
        0x1E, 0x1F,
    ]);
    let context = MediaKeyContext::new(
        0x0102_0304_0506_0708,
        0x0A0B_0C0D,
        0x0E0F,
        MediaDirection::AndroidToDesktop,
    )
    .unwrap();
    let mut sealer = MediaSealer::new(&secret, context).unwrap();
    let protected = sealer.seal(0x02, 960, b"SoniMesh media vector").unwrap();

    assert_eq!(
        protected.datagram,
        [
            0x53, 0x4E, 0x4D, 0x53, 0x01, 0x02, 0x00, 0x24, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06,
            0x07, 0x08, 0x0A, 0x0B, 0x0C, 0x0D, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x03, 0xC0, 0x00, 0x25, 0x0E, 0x0F, 0x7C, 0xCE, 0x62, 0xEA, 0x96, 0x0F,
            0xC2, 0x7C, 0xF5, 0xA6, 0xFF, 0xCE, 0x2A, 0xA5, 0x93, 0x43, 0xA5, 0xF1, 0x29, 0x97,
            0x79, 0x13, 0x96, 0xCA, 0x10, 0xBD, 0xFC, 0x7C, 0xC0, 0x60, 0xC9, 0xDF, 0xE4, 0x9F,
            0xD6, 0x71, 0x31,
        ]
    );
}

/// The canonical sealed datagram is pinned byte-for-byte so an AEAD crate,
/// key, nonce, or header change is caught, including by the cross-language
/// verifier (RFC 8439 ChaCha20-Poly1305).
#[test]
fn matches_published_cross_language_aead_packet_vector() {
    let secret = SessionSecret::new([4; 32]);
    let context = MediaKeyContext::new(7, 9, 3, MediaDirection::AndroidToDesktop).unwrap();
    let mut sealer = MediaSealer::new(&secret, context).unwrap();
    let opener = MediaOpener::new(&secret, context).unwrap();
    let protected = sealer
        .seal(0x2A, 0x1122_3344_5566_7788, b"cross-language packet vector")
        .unwrap();

    assert_eq!(
        protected.datagram,
        [
            0x53, 0x4E, 0x4D, 0x53, 0x01, 0x2A, 0x00, 0x24, // magic, version, flags, length
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07, // session_id 7
            0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x00, // stream_id 9, sequence 0
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, // sample_timestamp
            0x00, 0x2C, 0x00, 0x03, // payload length 44, key_epoch 3
            0x33, 0x7E, 0x80, 0x52, 0xF3, 0x0B, 0x4E, 0x20, 0x47, 0x9A, 0x0E, 0x40, 0x80, 0xA7,
            0x45, 0x11, 0xAA, 0x39, 0xC2, 0xDE, 0xAB, 0xAF, 0xA1, 0x32, 0xDB, 0xFD, 0x6B, 0x85,
            0xC7, 0xC8, 0x51, 0x45, 0x36, 0x0F, 0x62, 0xF4, 0x36, 0x1E, 0xFE, 0x33, 0x5F, 0x16,
            0x14, 0x9A, // Poly1305 tag
        ]
    );

    let packet = parse_media_packet(&protected.datagram).unwrap();
    let mut plaintext = [0_u8; 64];
    let length = opener.open(packet, &mut plaintext).unwrap();
    assert_eq!(&plaintext[..length], b"cross-language packet vector");
}

/// The reverse-direction (desktop-to-Android) sealed datagram is pinned
/// byte-for-byte so the direction byte in both the HKDF media-key info and the
/// 96-bit nonce stays domain-separated from the Android-to-desktop path
/// (architecture section 10); a third-party implementation can verify its
/// reverse path against this vector.
#[test]
fn matches_published_cross_language_reverse_direction_vector() {
    let secret = SessionSecret::new([
        0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E,
        0x1F, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x2B, 0x2C, 0x2D,
        0x2E, 0x2F,
    ]);
    let context = MediaKeyContext::new(
        0x0F0E_0D0C_0B0A_0908,
        0x1234_5678,
        0x002A,
        MediaDirection::DesktopToAndroid,
    )
    .unwrap();
    let mut sealer = MediaSealer::new(&secret, context).unwrap();
    let opener = MediaOpener::new(&secret, context).unwrap();
    let protected = sealer
        .seal(
            0x00,
            0x8899_AABB_CCDD_EEFF,
            b"SoniMesh reverse direction vector",
        )
        .unwrap();

    assert_eq!(
        protected.datagram,
        [
            0x53, 0x4E, 0x4D, 0x53, 0x01, 0x00, 0x00, 0x24, // magic, version, flags, length
            0x0F, 0x0E, 0x0D, 0x0C, 0x0B, 0x0A, 0x09, 0x08, // session_id
            0x12, 0x34, 0x56, 0x78, 0x00, 0x00, 0x00, 0x00, // stream_id, sequence 0
            0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, // sample_timestamp
            0x00, 0x31, 0x00, 0x2A, // payload length 49, key_epoch 0x002a
            0xA0, 0x85, 0x33, 0x60, 0x82, 0x5E, 0xFA, 0x8A, 0xB1, 0x52, 0x99, 0x00, 0xAA, 0x24,
            0x03, 0xA2, 0xE8, 0x2B, 0x06, 0xC3, 0xC8, 0xFB, 0xB2, 0xB8, 0x0B, 0x20, 0x53, 0x93,
            0xDF, 0xFF, 0x19, 0x15, 0x94, 0x30, 0xB6, 0xA0, 0x33, 0xE6, 0x99, 0xE1, 0x61, 0x3D,
            0x86, 0x89, 0x45, 0x5D, 0xC9, 0x5A, 0x99, // Poly1305 tag
        ]
    );

    let packet = parse_media_packet(&protected.datagram).unwrap();
    let mut plaintext = [0_u8; 64];
    let length = opener.open(packet, &mut plaintext).unwrap();
    assert_eq!(&plaintext[..length], b"SoniMesh reverse direction vector");
}
