//! Deterministic property coverage for the authenticated media ingress.
//!
//! The ingress is the security boundary where a datagram becomes decrypted
//! Opus: bounded parsing, then stream lookup (session/stream/epoch), then AEAD
//! authentication, and only then replay-window advancement, with every failure
//! path zeroing caller-owned plaintext storage. This test drives randomized
//! deliveries (in-window reorder, duplicates, tampered ciphertext, wrong
//! stream, wrong secret) against an independent `ReplayWindow`-based model and
//! asserts every outcome, payload, and cleared-storage contract agree. A
//! second test proves key-epoch rotation is atomic and never admits packets
//! from a retired epoch.

use sonimesh_network::{IngressError, ProtectedIngressError, ProtectedMediaIngress};
use sonimesh_protocol::{
    MediaCryptoError, MediaDirection, MediaKeyContext, MediaSealer, ReplayDecision, ReplayWindow,
    SessionSecret,
};

/// Packets sealed per flight; small enough to keep re-deliveries inside the
/// 64-bit replay window while still covering reorder and duplicate cases.
const FLIGHT: usize = 12;
const DELIVERIES_PER_ROUND: usize = 80;
const ROUNDS: usize = 16;

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

/// Drives randomized in-window deliveries plus tampered and foreign packets
/// against an independent replay model, asserting the ingress outcome, the
/// decrypted payload, and the cleared-storage contract on every failure.
///
/// `sealer`/`model`/`flight` are short local names, not a design smell, so the
/// noisy `similar_names` lint is allowed here.
#[allow(clippy::similar_names, clippy::too_many_lines)]
#[test]
fn random_deliveries_match_an_independent_replay_model() {
    let mut rng = XorShift64(0xBEEF_0000_2026_000B);
    let secret = random_secret(&mut rng);
    let context = MediaKeyContext::new(
        0xA1B2_C3D4_E5F6_1020,
        9,
        2,
        MediaDirection::AndroidToDesktop,
    )
    .unwrap();
    let mut ingress = ProtectedMediaIngress::new(&secret, context).unwrap();
    let mut model = ReplayWindow::new();
    // One sealer keeps the sender sequence monotonic across rounds so the
    // model replay window stays in lockstep with the ingress.
    let mut sealer = MediaSealer::new(&secret, context).unwrap();
    let mut next_sequence = 0_u32;

    // Foreign packets reused across every round.
    let wrong_stream_context = MediaKeyContext::new(
        context.session_id,
        context.stream_id + 1,
        context.key_epoch,
        context.direction,
    )
    .unwrap();
    let mut wrong_stream_sealer = MediaSealer::new(&secret, wrong_stream_context).unwrap();
    let wrong_stream_packet = wrong_stream_sealer.seal(0, 0, b"wrong stream").unwrap();
    let other_secret = random_secret(&mut rng);
    let mut wrong_secret_sealer = MediaSealer::new(&other_secret, context).unwrap();
    let wrong_secret_packet = wrong_secret_sealer.seal(0, 0, b"wrong secret").unwrap();

    for round in 0..ROUNDS {
        // Seal the next in-order flight of packets.
        let mut flight: Vec<(u32, Vec<u8>)> = Vec::new();
        for _ in 0..FLIGHT {
            let protected = sealer.seal(0, 0, b"authenticated payload").unwrap();
            assert_eq!(
                protected.header.sequence, next_sequence,
                "flight sealing order"
            );
            flight.push((protected.header.sequence, protected.datagram));
            next_sequence += 1;
        }

        for delivery in 0..DELIVERIES_PER_ROUND {
            let mut plaintext = [0_u8; 64];
            // `probe` carries an unseen valid packet whose tampered twin is
            // delivered first, proving failed authentication never consumes
            // the sequence.
            let mut probe: Option<(u32, Vec<u8>)> = None;
            let datagram: Vec<u8>;
            let expected: Result<u32, IngressError<MediaCryptoError>> = match rng.next_usize(7) {
                // Valid in-flight packet: reorder or duplicate within the window.
                0..=2 => {
                    let index = rng.next_usize(FLIGHT);
                    let (sequence, packet) = &flight[index];
                    datagram = packet.clone();
                    match model.accept_authenticated(*sequence) {
                        ReplayDecision::Accepted => Ok(*sequence),
                        other => Err(IngressError::Replay(other)),
                    }
                }
                // Tampered ciphertext of an in-flight packet: AEAD must fail
                // without consuming the sequence.
                3 => {
                    let (_sequence, packet) = &flight[rng.next_usize(FLIGHT)];
                    let mut tampered = packet.clone();
                    let last = tampered.len() - 1;
                    tampered[last] ^= 0x80;
                    datagram = tampered;
                    Err(IngressError::Authentication(
                        MediaCryptoError::AuthenticationFailed,
                    ))
                }
                // Same secret, wrong stream: rejected before authentication.
                4 => {
                    datagram = wrong_stream_packet.datagram.clone();
                    Err(IngressError::UnknownStream)
                }
                // Different secret, matching context: authentication fails.
                5 => {
                    datagram = wrong_secret_packet.datagram.clone();
                    Err(IngressError::Authentication(
                        MediaCryptoError::AuthenticationFailed,
                    ))
                }
                // Fresh unseen sequence: the tampered delivery must fail, and
                // the valid twin must still be admitted afterwards.
                _ => {
                    let protected = sealer.seal(0, 0, b"authenticated payload").unwrap();
                    assert_eq!(
                        protected.header.sequence, next_sequence,
                        "probe sealing order"
                    );
                    next_sequence += 1;
                    probe = Some((protected.header.sequence, protected.datagram.clone()));
                    let mut tampered = protected.datagram;
                    let last = tampered.len() - 1;
                    tampered[last] ^= 0x80;
                    datagram = tampered;
                    Err(IngressError::Authentication(
                        MediaCryptoError::AuthenticationFailed,
                    ))
                }
            };

            let actual = match ingress.receive(&datagram, &mut plaintext) {
                Ok(frame) => {
                    assert_eq!(
                        frame.opus_payload, b"authenticated payload",
                        "delivery {delivery} payload"
                    );
                    Ok(frame.header.sequence)
                }
                Err(error) => Err(error),
            };
            assert_eq!(
                actual, expected,
                "delivery {delivery} round {round} outcome mismatch"
            );
            if actual.is_err() {
                assert!(
                    plaintext.iter().all(|byte| *byte == 0),
                    "delivery {delivery} failure left plaintext storage dirty"
                );
            }

            // The failed probe must not have consumed the fresh sequence.
            if let Some((sequence, valid)) = probe {
                assert_eq!(
                    model.accept_authenticated(sequence),
                    ReplayDecision::Accepted,
                    "delivery {delivery} probe sequence must stay unseen"
                );
                let mut twin = [0_u8; 64];
                let frame = ingress.receive(&valid, &mut twin).unwrap();
                assert_eq!(
                    frame.opus_payload, b"authenticated payload",
                    "delivery {delivery} valid twin payload"
                );
            }
        }
    }
}

/// Rotations must advance the key epoch atomically: failed rotations leave the
/// ingress untouched, a successful rotation rejects every packet from the
/// retired epoch, and the fresh epoch starts with a clean replay window.
#[allow(
    clippy::cast_possible_truncation,
    clippy::similar_names,
    clippy::too_many_lines
)]
#[test]
fn rotations_advance_epoch_atomically_and_never_accept_retired_packets() {
    use std::collections::HashMap;

    let mut rng = XorShift64(0xBEEF_0000_2026_000C);
    let secret = random_secret(&mut rng);
    let context_at = |epoch: u16| {
        MediaKeyContext::new(
            0xA1B2_C3D4_E5F6_1020,
            9,
            epoch,
            MediaDirection::AndroidToDesktop,
        )
        .unwrap()
    };

    let mut current_epoch = 1 + (rng.next_usize(10) as u16);
    // One sealer per epoch keeps the sender sequence aligned with the ingress
    // replay window when an epoch is revisited as the next rotation target.
    let mut sealers: HashMap<u16, MediaSealer> = HashMap::new();
    let mut ingress = ProtectedMediaIngress::new(&secret, context_at(current_epoch)).unwrap();
    for round in 0..200 {
        // The current epoch admits fresh packets and rejects duplicates.
        let first = sealers
            .entry(current_epoch)
            .or_insert_with(|| MediaSealer::new(&secret, context_at(current_epoch)).unwrap())
            .seal(0, 0, b"current epoch")
            .unwrap();
        let mut plaintext = [0_u8; 64];
        assert_eq!(
            ingress
                .receive(&first.datagram, &mut plaintext)
                .unwrap()
                .opus_payload,
            b"current epoch",
            "round {round} current epoch admission"
        );
        assert_eq!(
            ingress.receive(&first.datagram, &mut plaintext),
            Err(IngressError::Replay(ReplayDecision::Duplicate)),
            "round {round} duplicate rejection"
        );
        let second = sealers
            .get_mut(&current_epoch)
            .unwrap()
            .seal(0, 0, b"current epoch")
            .unwrap();
        assert!(
            ingress.receive(&second.datagram, &mut plaintext).is_ok(),
            "round {round} next in-order packet"
        );

        // Invalid rotations must not mutate the installed context.
        assert_eq!(
            ingress.rotate(&secret, context_at(current_epoch)),
            Err(ProtectedIngressError::NonIncreasingEpoch),
            "round {round} same epoch"
        );
        if current_epoch > 1 {
            assert_eq!(
                ingress.rotate(&secret, context_at(current_epoch - 1)),
                Err(ProtectedIngressError::NonIncreasingEpoch),
                "round {round} lower epoch"
            );
        }
        let wrong_stream = MediaKeyContext::new(
            context_at(current_epoch).session_id,
            context_at(current_epoch).stream_id + 1,
            current_epoch + 1,
            context_at(current_epoch).direction,
        )
        .unwrap();
        assert_eq!(
            ingress.rotate(&secret, wrong_stream),
            Err(ProtectedIngressError::RotationContextMismatch),
            "round {round} changed stream"
        );
        assert_eq!(
            ingress.context(),
            context_at(current_epoch),
            "round {round} context"
        );

        // A valid rotation atomically replaces AEAD and replay state.
        let old_packet = sealers
            .get_mut(&current_epoch)
            .unwrap()
            .seal(0, 0, b"retired epoch")
            .unwrap();
        let next_epoch = current_epoch + 1 + (rng.next_usize(4) as u16);
        ingress.rotate(&secret, context_at(next_epoch)).unwrap();
        assert_eq!(
            ingress.context(),
            context_at(next_epoch),
            "round {round} rotation"
        );
        assert_eq!(
            ingress.identity().key_epoch,
            next_epoch,
            "round {round} identity"
        );

        plaintext.fill(0xFF);
        assert_eq!(
            ingress.receive(&old_packet.datagram, &mut plaintext),
            Err(IngressError::UnknownStream),
            "round {round} retired epoch admission"
        );
        assert!(
            plaintext.iter().all(|byte| *byte == 0),
            "round {round} retired packet left storage dirty"
        );

        let fresh = sealers
            .entry(next_epoch)
            .or_insert_with(|| MediaSealer::new(&secret, context_at(next_epoch)).unwrap())
            .seal(0, 0, b"fresh epoch")
            .unwrap();
        assert_eq!(
            ingress
                .receive(&fresh.datagram, &mut plaintext)
                .unwrap()
                .opus_payload,
            b"fresh epoch",
            "round {round} fresh epoch admission"
        );
        current_epoch = next_epoch;
    }
}
