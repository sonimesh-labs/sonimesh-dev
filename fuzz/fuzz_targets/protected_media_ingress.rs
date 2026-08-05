#![no_main]

use libfuzzer_sys::fuzz_target;
use sonimesh_network::ProtectedMediaIngress;
use sonimesh_protocol::{
    MAX_MEDIA_PLAINTEXT_LEN, MEDIA_HEADER_LEN, MediaDirection, MediaKeyContext, MediaSealer,
    SessionSecret,
};

/// Offset of `MediaHeader::flags` in the canonical fixed header encoding.
const FLAGS_OFFSET: usize = 5;
const SESSION_ID: u64 = 0x0102_0304_0506_0708;
const STREAM_ID: u32 = 0x0A0B_0C0D;
const KEY_EPOCH: u16 = 0x0E0F;
const SESSION_SECRET_BYTES: [u8; 32] = [0xA5; 32];

fn context() -> MediaKeyContext {
    MediaKeyContext::new(
        SESSION_ID,
        STREAM_ID,
        KEY_EPOCH,
        MediaDirection::AndroidToDesktop,
    )
    .expect("fixed fuzz context is valid")
}

fn ingress(secret: &SessionSecret) -> ProtectedMediaIngress {
    ProtectedMediaIngress::new(secret, context()).expect("fixed fuzz key context is valid")
}

fn exercise_untrusted_datagram(secret: &SessionSecret, datagram: &[u8]) {
    let mut ingress = ingress(secret);
    let mut plaintext = [0xA5; MAX_MEDIA_PLAINTEXT_LEN];
    if ingress.receive(datagram, &mut plaintext).is_err() {
        assert!(plaintext.iter().all(|byte| *byte == 0));
    }
}

fn assert_tampered_byte_is_rejected_and_cleared(
    secret: &SessionSecret,
    datagram: &[u8],
    index: usize,
    mutation: u8,
) {
    let mut tampered = datagram.to_vec();
    tampered[index] ^= mutation;
    let mut ingress = ingress(secret);
    let mut plaintext = [0xA5; MAX_MEDIA_PLAINTEXT_LEN];
    assert!(ingress.receive(&tampered, &mut plaintext).is_err());
    assert!(plaintext.iter().all(|byte| *byte == 0));
}

fuzz_target!(|data: &[u8]| {
    let secret = SessionSecret::new(SESSION_SECRET_BYTES);
    exercise_untrusted_datagram(&secret, data);

    let payload = &data[..data.len().min(MAX_MEDIA_PLAINTEXT_LEN)];
    let mut sealer = MediaSealer::new(&secret, context()).expect("fixed fuzz key context is valid");
    let packet = sealer
        .seal(data.first().copied().unwrap_or_default(), 960, payload)
        .expect("bounded fuzz payload is sealable");

    let mut valid_ingress = ingress(&secret);
    let mut plaintext = [0xA5; MAX_MEDIA_PLAINTEXT_LEN];
    {
        let frame = valid_ingress
            .receive(&packet.datagram, &mut plaintext)
            .expect("fresh matching ingress accepts a sealed packet");
        assert_eq!(frame.opus_payload, payload);
    }

    plaintext.fill(0xA5);
    assert!(
        valid_ingress
            .receive(&packet.datagram, &mut plaintext)
            .is_err()
    );
    assert!(plaintext.iter().all(|byte| *byte == 0));

    if !payload.is_empty() {
        let mut short_ingress = ingress(&secret);
        let mut short_plaintext = [0xA5; MAX_MEDIA_PLAINTEXT_LEN];
        let short_length = payload.len() - 1;
        assert!(
            short_ingress
                .receive(&packet.datagram, &mut short_plaintext[..short_length])
                .is_err()
        );
        assert!(
            short_plaintext[..short_length]
                .iter()
                .all(|byte| *byte == 0)
        );
    }

    let mutation = data.get(2).copied().unwrap_or(1).max(1);
    assert_tampered_byte_is_rejected_and_cleared(&secret, &packet.datagram, FLAGS_OFFSET, mutation);
    if !payload.is_empty() {
        assert_tampered_byte_is_rejected_and_cleared(
            &secret,
            &packet.datagram,
            MEDIA_HEADER_LEN,
            mutation,
        );
    }
    assert_tampered_byte_is_rejected_and_cleared(
        &secret,
        &packet.datagram,
        packet.datagram.len() - 1,
        mutation,
    );
});
