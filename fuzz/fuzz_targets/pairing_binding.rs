#![no_main]

use libfuzzer_sys::fuzz_target;
use sonimesh_protocol::{PairingBinding, PairingRole};

const PAIRING_MATERIAL_LEN: usize = 16 + 32 + 32;

fn take_array<const N: usize>(data: &[u8], offset: usize) -> [u8; N] {
    let mut output = [0_u8; N];
    let remaining = data.get(offset..).unwrap_or_default();
    let copied = remaining.len().min(N);
    output[..copied].copy_from_slice(&remaining[..copied]);
    output
}

fuzz_target!(|data: &[u8]| {
    let pairing_id = take_array::<16>(data, 0);
    let pairing_secret = take_array::<32>(data, 16);
    let tls_exporter = take_array::<32>(data, 48);
    let binding = PairingBinding::new(1, pairing_id, pairing_secret, &tls_exporter)
        .expect("fixed fuzz binding inputs are valid");
    let untrusted_proof = data.get(PAIRING_MATERIAL_LEN..).unwrap_or_default();
    for (local_role, peer_role) in [
        (PairingRole::AndroidClient, PairingRole::DesktopServer),
        (PairingRole::DesktopServer, PairingRole::AndroidClient),
    ] {
        let peer_proof = binding
            .local_proof(peer_role)
            .expect("fixed pairing proof derivation succeeds");
        assert!(
            binding
                .verify_peer_proof(local_role, &peer_proof)
                .expect("fixed peer proof verification succeeds")
        );

        let local_proof = binding
            .local_proof(local_role)
            .expect("fixed pairing proof derivation succeeds");
        assert!(
            !binding
                .verify_peer_proof(local_role, &local_proof)
                .expect("reflected proof verification succeeds")
        );

        let mut altered_peer_proof = peer_proof;
        let alteration_index =
            usize::from(data.first().copied().unwrap_or_default()) % altered_peer_proof.len();
        let alteration = data.get(1).copied().unwrap_or(1).max(1);
        altered_peer_proof[alteration_index] ^= alteration;
        assert!(
            !binding
                .verify_peer_proof(local_role, &altered_peer_proof)
                .expect("altered proof verification succeeds")
        );

        let accepted = binding
            .verify_peer_proof(local_role, untrusted_proof)
            .expect("untrusted proof verification succeeds");
        assert_eq!(
            accepted,
            untrusted_proof == peer_proof.as_slice(),
            "untrusted proof verification must match the derived peer proof",
        );
    }
});
