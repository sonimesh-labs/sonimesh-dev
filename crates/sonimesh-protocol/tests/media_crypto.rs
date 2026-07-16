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
