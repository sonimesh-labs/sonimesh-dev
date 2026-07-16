//! AEAD, context, and caller-owned-output tests for protected media packets.

use sonimesh_protocol::{
    MAX_MEDIA_PLAINTEXT_LEN, MediaCryptoError, MediaDirection, MediaKeyContext, MediaOpener,
    MediaSealer, SessionSecret, parse_media_packet,
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
    assert!(plaintext.iter().all(|byte| *byte == 0 || *byte == 0xFF));

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
    assert!(plaintext.iter().all(|byte| *byte == 0 || *byte == 0xFF));
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
fn rejects_plaintext_larger_than_media_capacity() {
    let secret = secret();
    let mut sealer =
        MediaSealer::new(&secret, context(7, MediaDirection::AndroidToDesktop)).unwrap();
    let oversized = vec![0_u8; MAX_MEDIA_PLAINTEXT_LEN + 1];

    assert_eq!(
        sealer.seal(0, 0, &oversized),
        Err(MediaCryptoError::PayloadTooLarge)
    );
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
