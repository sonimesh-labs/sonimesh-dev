//! Deterministic property coverage for the protected media data plane.
//!
//! The media plane's security contract is that a sealed packet round-trips to
//! exactly its plaintext and header, that the 96-bit AEAD nonce is unique
//! across stream, direction, epoch, and sequence, and that the sender sequence
//! source never reuses a value. This test drives random contexts, payloads,
//! and sequences against the public API and asserts every one of those rules.

use std::collections::HashSet;

use sonimesh_protocol::{
    MAX_MEDIA_PLAINTEXT_LEN, MediaCryptoError, MediaDirection, MediaKeyContext, MediaOpener,
    MediaSealer, MediaSequence, SessionSecret, media_nonce, parse_media_packet,
};

/// Small deterministic xorshift64 generator; no external dependency.
struct XorShift64(u64);

impl XorShift64 {
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 32) as u32
    }

    fn next_u64(&mut self) -> u64 {
        (u64::from(self.next_u32()) << 32) | u64::from(self.next_u32())
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        self.next_u32() as usize % bound
    }

    #[allow(clippy::cast_possible_truncation)]
    fn next_byte(&mut self) -> u8 {
        self.next_u32() as u8
    }
}

/// Random 32-byte session secret.
fn random_secret(rng: &mut XorShift64) -> SessionSecret {
    let mut bytes = [0_u8; 32];
    for byte in &mut bytes {
        *byte = rng.next_byte();
    }
    SessionSecret::new(bytes)
}

/// Random valid media key context with nonzero session and stream IDs.
#[allow(clippy::cast_possible_truncation)]
fn random_context(rng: &mut XorShift64) -> MediaKeyContext {
    let session_id = rng.next_u64().max(1);
    let stream_id = rng.next_u32().max(1);
    let key_epoch = (rng.next_u32() % 65_536) as u16;
    let direction = if rng.next_u32() % 2 == 0 {
        MediaDirection::AndroidToDesktop
    } else {
        MediaDirection::DesktopToAndroid
    };
    MediaKeyContext::new(session_id, stream_id, key_epoch, direction).unwrap()
}

/// `sealer`/`opener`/`protected` are short local names, not a design smell, so
/// the noisy `similar_names` lint is allowed here.
#[allow(clippy::similar_names)]
#[test]
fn random_payloads_round_trip_through_seal_parse_and_open() {
    let mut rng = XorShift64(0x5EED_0000_2026_0007);
    for iteration in 0..1_000 {
        let secret = random_secret(&mut rng);
        let context = random_context(&mut rng);
        let flags = rng.next_byte();
        let sample_timestamp = rng.next_u64();
        let payload_len = rng.next_usize(MAX_MEDIA_PLAINTEXT_LEN + 1);
        let payload: Vec<u8> = (0..payload_len).map(|_| rng.next_byte()).collect();

        let mut sealer = MediaSealer::new(&secret, context).unwrap();
        let opener = MediaOpener::new(&secret, context).unwrap();
        let protected = sealer.seal(flags, sample_timestamp, &payload).unwrap();
        let packet = parse_media_packet(&protected.datagram).unwrap();

        assert_eq!(
            packet.header.session_id, context.session_id,
            "iteration {iteration}"
        );
        assert_eq!(
            packet.header.stream_id, context.stream_id,
            "iteration {iteration}"
        );
        assert_eq!(
            packet.header.key_epoch, context.key_epoch,
            "iteration {iteration}"
        );
        assert_eq!(packet.header.flags, flags, "iteration {iteration}");
        assert_eq!(
            packet.header.sample_timestamp, sample_timestamp,
            "iteration {iteration}"
        );
        assert_eq!(
            packet.header.sequence, 0,
            "iteration {iteration} fresh sealer"
        );

        let mut plaintext = vec![0_u8; MAX_MEDIA_PLAINTEXT_LEN];
        let length = opener.open(packet, &mut plaintext).unwrap();
        assert_eq!(
            length, payload_len,
            "iteration {iteration} plaintext length"
        );
        assert_eq!(
            &plaintext[..length],
            &payload[..],
            "iteration {iteration} plaintext"
        );
    }
}

/// One sealer over a run of packets must never reuse a sequence and each
/// packet must still round-trip.
#[allow(clippy::cast_possible_truncation, clippy::similar_names)]
#[test]
fn one_sealer_never_reuses_a_sequence_across_a_run() {
    let mut rng = XorShift64(0x5EED_0000_2026_0008);
    for iteration in 0..500 {
        let secret = random_secret(&mut rng);
        let context = random_context(&mut rng);
        let mut sealer = MediaSealer::new(&secret, context).unwrap();
        let opener = MediaOpener::new(&secret, context).unwrap();
        let run = 1 + rng.next_usize(40);
        let mut seen = HashSet::new();
        for expected in 0..run {
            let protected = sealer
                .seal(rng.next_byte(), rng.next_u64(), b"audio")
                .unwrap();
            assert_eq!(
                protected.header.sequence, expected as u32,
                "iteration {iteration} sequence order"
            );
            assert!(
                seen.insert(protected.header.sequence),
                "iteration {iteration} sequence reuse"
            );
            let packet = parse_media_packet(&protected.datagram).unwrap();
            let mut plaintext = [0_u8; 8];
            let length = opener.open(packet, &mut plaintext).unwrap();
            assert_eq!(
                &plaintext[..length],
                b"audio",
                "iteration {iteration} payload"
            );
        }
    }
}

/// The 96-bit nonce layout occupies disjoint bytes per dimension, which is
/// what guarantees uniqueness; a large random sample must never collide and
/// every nonce must match the documented byte layout.
#[test]
fn nonces_are_injective_across_random_contexts_and_sequences() {
    let mut rng = XorShift64(0x5EED_0000_2026_0009);
    let mut nonces = HashSet::new();
    for _ in 0..100_000 {
        let context = random_context(&mut rng);
        let sequence = rng.next_u32();
        let nonce = media_nonce(context, sequence);

        assert_eq!(&nonce[0..2], &context.key_epoch.to_be_bytes());
        assert_eq!(nonce[2], context.direction as u8);
        assert_eq!(nonce[3], 0);
        assert_eq!(&nonce[4..8], &context.stream_id.to_be_bytes());
        assert_eq!(&nonce[8..12], &sequence.to_be_bytes());
        assert!(nonces.insert(nonce), "nonce collision in the random sample");
    }
}

/// The sender sequence source is strictly increasing while available and stays
/// exhausted once it reports [`MediaCryptoError::SequenceExhausted`].
#[allow(clippy::cast_possible_truncation)]
#[test]
fn sequences_are_strictly_increasing_and_exhaust_cleanly() {
    let mut rng = XorShift64(0x5EED_0000_2026_000A);
    for iteration in 0..10_000 {
        let initial = if rng.next_u32() % 2 == 0 {
            rng.next_u32()
        } else {
            u32::MAX - (rng.next_usize(200) as u32)
        };
        let mut sequence = MediaSequence::new(initial);
        let mut previous: Option<u32> = None;
        for _ in 0..64 {
            match sequence.take_next() {
                Ok(next) => {
                    if let Some(last) = previous {
                        assert!(next > last, "iteration {iteration} not increasing");
                    }
                    previous = Some(next);
                }
                Err(error) => {
                    assert_eq!(
                        error,
                        MediaCryptoError::SequenceExhausted,
                        "iteration {iteration} unexpected error"
                    );
                    assert_eq!(
                        sequence.take_next(),
                        Err(MediaCryptoError::SequenceExhausted),
                        "iteration {iteration} recovered after exhaustion"
                    );
                    break;
                }
            }
        }
    }
}
