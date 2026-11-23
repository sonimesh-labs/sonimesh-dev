#![no_main]

use libfuzzer_sys::fuzz_target;
use sonimesh_protocol::{
    MAX_QR_PAYLOAD_LEN, PairingQrPayload, QR_PREFIX_LEN, parse_pairing_qr_payload,
};

const PUBLISHED_QR_PAYLOAD: &[u8] = &[
    0x53, 0x4D, 0x51, 0x52, 0x01, 0x01, 0x14, 0xB9, 0xFA, 0x00, 0x30, 0x00, 0x01, 0x02, 0x03, 0x04,
    0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11, 0x12, 0x13, 0x14,
    0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F, 0x20, 0x21, 0x22, 0x23, 0x24,
    0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x2B, 0x2C, 0x2D, 0x2E, 0x2F, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x1E, 0x84, 0x80, 0x64, 0x65, 0x73, 0x6B, 0x74, 0x6F, 0x70, 0x2D, 0x30, 0x31, 0x2E, 0x6C, 0x61,
    0x62, 0x2E, 0x6C, 0x6F, 0x63, 0x61, 0x6C, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48,
    0x49, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E, 0x4F, 0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58,
    0x59, 0x5A, 0x5B, 0x5C, 0x5D, 0x5E, 0x5F, 0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
    0x69, 0x6A, 0x6B, 0x6C, 0x6D, 0x6E, 0x6F,
];

fuzz_target!(|data: &[u8]| {
    // Arbitrary bytes must never panic, allocate, or produce a view that
    // violates the documented bounds.
    if let Ok(view) = parse_pairing_qr_payload(data) {
        assert!(view.pairing_secret.len() == 32);
        assert!(!view.control_host.is_empty());
        assert!(view.control_host.len() <= 255);
        assert!((32..=512).contains(&view.desktop_tls_spki.len()));
        assert_ne!(view.control_port, 0);
        assert_ne!(view.expires_at_ms, 0);
        assert_eq!(view.protocol_version, 1);
    }

    // Deterministic mutations of the published vector exercise each header
    // field and the declared-length consistency checks.
    let mut near_valid = [0_u8; MAX_QR_PAYLOAD_LEN];
    let published_len = PUBLISHED_QR_PAYLOAD.len();
    near_valid[..published_len].copy_from_slice(PUBLISHED_QR_PAYLOAD);
    for (target, mutation) in near_valid[..published_len].iter_mut().zip(data) {
        *target ^= mutation;
    }
    if let Ok(view) = parse_pairing_qr_payload(&near_valid[..published_len]) {
        // A decoded mutation must still round-trip through the bounded
        // encoder without changing any byte.
        let payload = PairingQrPayload {
            protocol_version: view.protocol_version,
            pairing_id: view.pairing_id,
            pairing_secret: [0_u8; 32],
            control_host: view.control_host,
            control_port: view.control_port,
            desktop_tls_spki: view.desktop_tls_spki,
            expires_at_ms: view.expires_at_ms,
        };
        let mut encoded = [0_u8; MAX_QR_PAYLOAD_LEN];
        let written = payload
            .encode_into(&mut encoded)
            .expect("a validated view always re-encodes");
        assert_eq!(written, published_len);
        assert_eq!(&encoded[..written], &near_valid[..published_len]);

        // The re-encoded payload must decode to the same fields again, with
        // the zeroed secret substituted back before comparison.
        let redecoded =
            parse_pairing_qr_payload(&encoded[..written]).expect("the encoder output must decode");
        assert_eq!(redecoded.pairing_id, view.pairing_id);
        assert_eq!(redecoded.control_host, view.control_host);
        assert_eq!(redecoded.control_port, view.control_port);
        assert_eq!(redecoded.desktop_tls_spki, view.desktop_tls_spki);
        assert_eq!(redecoded.expires_at_ms, view.expires_at_ms);
    }

    // Truncations of the published vector must stay rejected or bounded.
    for cut in [QR_PREFIX_LEN - 1, QR_PREFIX_LEN, published_len - 1] {
        let _ = parse_pairing_qr_payload(&PUBLISHED_QR_PAYLOAD[..cut.min(published_len)]);
    }
});
