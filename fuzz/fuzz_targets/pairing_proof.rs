#![no_main]

use libfuzzer_sys::fuzz_target;
use sonimesh_protocol::{
    PROOF_FRAME_LEN, PairingProofFrame, PairingRole, parse_pairing_proof_frame,
};

const PUBLISHED_PROOF_FRAME: &[u8] = &[
    0x53, 0x4D, 0x50, 0x45, 0x01, 0x01, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
    0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0xC2, 0x74, 0xDF, 0xE7, 0x21, 0x35, 0xAE, 0x55, 0x80,
    0x34, 0xCA, 0x1E, 0x8A, 0x27, 0xA4, 0xEA, 0xB7, 0xAA, 0xC9, 0xB8, 0x7A, 0x0C, 0xD9, 0x3C, 0x17,
    0xBF, 0x17, 0x12, 0x6E, 0x27, 0xF4, 0xF2,
];

fuzz_target!(|data: &[u8]| {
    // Arbitrary bytes must never panic or produce a frame outside the
    // documented fixed layout.
    if let Ok(frame) = parse_pairing_proof_frame(data) {
        assert_eq!(frame.protocol_version, 1);
        assert!(matches!(
            frame.role,
            PairingRole::AndroidClient | PairingRole::DesktopServer
        ));
        assert_eq!(frame.pairing_id.len(), 16);
        assert_eq!(frame.proof.len(), 32);
    }

    // Deterministic mutations of the published vector exercise each header
    // field; every accepted mutation must re-encode byte-exactly.
    let mut near_valid = [0_u8; PROOF_FRAME_LEN];
    near_valid.copy_from_slice(PUBLISHED_PROOF_FRAME);
    for (target, mutation) in near_valid.iter_mut().zip(data) {
        *target ^= mutation;
    }
    if let Ok(view) = parse_pairing_proof_frame(&near_valid) {
        let payload = PairingProofFrame {
            protocol_version: view.protocol_version,
            role: view.role,
            pairing_id: view.pairing_id,
            proof: view.proof,
        };
        let mut encoded = [0_u8; PROOF_FRAME_LEN];
        payload
            .encode_into(&mut encoded)
            .expect("a validated frame always re-encodes");
        assert_eq!(encoded.as_slice(), &near_valid);

        let redecoded =
            parse_pairing_proof_frame(&encoded).expect("the encoder output must decode");
        assert_eq!(redecoded, payload);
    }

    // Truncations and one-byte extensions of the published vector must stay
    // rejected without panicking.
    for cut in [PROOF_FRAME_LEN - 1, PROOF_FRAME_LEN + 1] {
        let mut candidate = PUBLISHED_PROOF_FRAME.to_vec();
        candidate.truncate(cut.min(PROOF_FRAME_LEN));
        if cut > PROOF_FRAME_LEN {
            candidate.push(0);
        }
        let _ = parse_pairing_proof_frame(&candidate);
    }
});
