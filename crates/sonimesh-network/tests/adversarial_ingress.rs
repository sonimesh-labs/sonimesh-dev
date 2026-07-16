//! Deterministic invalid-AEAD regression for the authenticated ingress boundary.

use sonimesh_network::{IngressError, ProtectedMediaIngress};
use sonimesh_protocol::{MediaDirection, MediaHeader, MediaKeyContext, SessionSecret};

fn context() -> MediaKeyContext {
    MediaKeyContext::new(
        0xA1B2_C3D4_E5F6_1020,
        9,
        2,
        MediaDirection::AndroidToDesktop,
    )
    .unwrap()
}

#[test]
fn invalid_aead_payloads_never_reach_plaintext_storage() {
    let secret = SessionSecret::new([0xA5; 32]);
    let context = context();
    let mut ingress = ProtectedMediaIngress::new(&secret, context).unwrap();
    let mut state = 0xA11C_E5ED_C0DE_2026_u64;
    let mut plaintext = [0xFF; 64];

    for sequence in 0..512_u32 {
        let mut ciphertext = [0_u8; 32];
        for byte in &mut ciphertext {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            *byte = state.to_le_bytes()[4];
        }
        let header = MediaHeader {
            flags: 0,
            session_id: context.session_id,
            stream_id: context.stream_id,
            sequence,
            sample_timestamp: u64::from(sequence) * 960,
            key_epoch: context.key_epoch,
        };
        let mut datagram = header.encode(ciphertext.len()).unwrap().to_vec();
        datagram.extend_from_slice(&ciphertext);
        plaintext.fill(0xFF);

        assert!(matches!(
            ingress.receive(&datagram, &mut plaintext),
            Err(IngressError::Authentication(_))
        ));
        assert!(plaintext.iter().all(|byte| *byte == 0));
    }
}
