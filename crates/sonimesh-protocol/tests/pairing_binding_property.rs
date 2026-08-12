//! Deterministic property coverage for the version-one pairing binding.
//!
//! `PairingBinding` derives per-role proofs and a media root from TLS exporter
//! bytes that a pairing owner authenticated out of band. The security contract
//! under test: an endpoint's local proof verifies only as the peer's proof, a
//! reflected local proof never verifies, every derived secret depends on every
//! input byte, distinct pairings derive distinct media keys, construction
//! validates the protocol version and exporter length, and arbitrary inputs
//! never panic.

use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, MediaDirection, MediaKeyContext, MediaOpener, MediaSealer,
    PAIRING_TLS_EXPORTER_LEN, PairingBinding, PairingBindingError, PairingRole, parse_media_packet,
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

    #[allow(clippy::cast_possible_truncation)]
    fn next_byte(&mut self) -> u8 {
        self.next_u32() as u8
    }
}

fn protocol_version() -> u8 {
    u8::try_from(CONTROL_PROTOCOL_VERSION).expect("protocol version fits a u8")
}

fn random_bytes(rng: &mut XorShift64, length: usize) -> Vec<u8> {
    (0..length).map(|_| rng.next_byte()).collect()
}

fn random_id(rng: &mut XorShift64) -> [u8; 16] {
    random_bytes(rng, 16)
        .try_into()
        .expect("16 pairing id bytes")
}

fn random_secret(rng: &mut XorShift64) -> [u8; 32] {
    random_bytes(rng, 32)
        .try_into()
        .expect("32 pairing secret bytes")
}

fn random_exporter(rng: &mut XorShift64) -> [u8; PAIRING_TLS_EXPORTER_LEN] {
    random_bytes(rng, PAIRING_TLS_EXPORTER_LEN)
        .try_into()
        .expect("exporter length matches the contract")
}

type Inputs = ([u8; 16], [u8; 32], [u8; PAIRING_TLS_EXPORTER_LEN]);

fn random_inputs(rng: &mut XorShift64) -> Inputs {
    (random_id(rng), random_secret(rng), random_exporter(rng))
}

fn binding_from(inputs: &Inputs) -> PairingBinding {
    let (id, secret, exporter) = inputs;
    PairingBinding::new(protocol_version(), *id, *secret, exporter).expect("inputs construct")
}

/// Each endpoint's local proof verifies only as the opposite role's expected
/// peer proof; a reflected local proof never verifies; a proof from a
/// different pairing never verifies; and rebuilding the binding from the same
/// inputs reproduces the same proofs.
#[test]
fn peer_proofs_verify_cross_role_and_reject_reflections() {
    let mut rng = XorShift64(0x5041_0000_2026_0013);
    for iteration in 0..200 {
        let inputs = random_inputs(&mut rng);
        let binding = binding_from(&inputs);
        let client_proof = binding
            .local_proof(PairingRole::AndroidClient)
            .expect("client proof");
        let server_proof = binding
            .local_proof(PairingRole::DesktopServer)
            .expect("server proof");

        assert_eq!(
            binding.verify_peer_proof(PairingRole::DesktopServer, &client_proof),
            Ok(true),
            "iteration {iteration}: client proof verifies for the desktop"
        );
        assert_eq!(
            binding.verify_peer_proof(PairingRole::AndroidClient, &server_proof),
            Ok(true),
            "iteration {iteration}: server proof verifies for android"
        );

        assert_eq!(
            binding.verify_peer_proof(PairingRole::AndroidClient, &client_proof),
            Ok(false),
            "iteration {iteration}: reflected client proof rejected"
        );
        assert_eq!(
            binding.verify_peer_proof(PairingRole::DesktopServer, &server_proof),
            Ok(false),
            "iteration {iteration}: reflected server proof rejected"
        );

        let other_inputs = random_inputs(&mut rng);
        let other = binding_from(&other_inputs);
        assert_eq!(
            other.verify_peer_proof(PairingRole::DesktopServer, &client_proof),
            Ok(false),
            "iteration {iteration}: proof from another pairing rejected"
        );

        let rebuilt = binding_from(&inputs);
        assert_eq!(
            rebuilt
                .local_proof(PairingRole::AndroidClient)
                .expect("rebuilt client proof"),
            client_proof,
            "iteration {iteration}: deterministic"
        );
    }
}

/// Flipping any single byte of the pairing id, pairing secret, or TLS exporter
/// must change both role proofs; the binding is sensitive to every input byte.
#[test]
fn every_input_byte_affects_both_role_proofs() {
    let mut rng = XorShift64(0x5041_0000_2026_0014);
    let (id, secret, exporter) = random_inputs(&mut rng);
    let base = binding_from(&(id, secret, exporter));
    let base_client = base
        .local_proof(PairingRole::AndroidClient)
        .expect("base client");
    let base_server = base
        .local_proof(PairingRole::DesktopServer)
        .expect("base server");

    for index in 0..id.len() {
        let mut flipped = id;
        flipped[index] ^= 0x01;
        let binding = binding_from(&(flipped, secret, exporter));
        assert_ne!(
            binding
                .local_proof(PairingRole::AndroidClient)
                .expect("client"),
            base_client,
            "pairing id byte {index}"
        );
        assert_ne!(
            binding
                .local_proof(PairingRole::DesktopServer)
                .expect("server"),
            base_server,
            "pairing id byte {index}"
        );
    }
    for index in 0..secret.len() {
        let mut flipped = secret;
        flipped[index] ^= 0x01;
        let binding = binding_from(&(id, flipped, exporter));
        assert_ne!(
            binding
                .local_proof(PairingRole::AndroidClient)
                .expect("client"),
            base_client,
            "pairing secret byte {index}"
        );
        assert_ne!(
            binding
                .local_proof(PairingRole::DesktopServer)
                .expect("server"),
            base_server,
            "pairing secret byte {index}"
        );
    }
    for index in 0..exporter.len() {
        let mut flipped = exporter;
        flipped[index] ^= 0x01;
        let binding = binding_from(&(id, secret, flipped));
        assert_ne!(
            binding
                .local_proof(PairingRole::AndroidClient)
                .expect("client"),
            base_client,
            "tls exporter byte {index}"
        );
        assert_ne!(
            binding
                .local_proof(PairingRole::DesktopServer)
                .expect("server"),
            base_server,
            "tls exporter byte {index}"
        );
    }
}

/// Distinct pairings must derive distinct media roots, observable through the
/// media-key boundary: a packet sealed under one pairing's root must not open
/// under another pairing that shares the same secret and exporter but a
/// different QR pairing id. The root must also stay redacted in its debug form
/// and must reproduce exactly when the binding is rebuilt.
#[test]
fn distinct_pairings_derive_distinct_media_keys() {
    let mut rng = XorShift64(0x5041_0000_2026_0015);
    let context = MediaKeyContext::new(
        0x9EED_0000_0000_0001,
        7,
        0,
        MediaDirection::AndroidToDesktop,
    )
    .expect("valid media context");
    for iteration in 0..100 {
        let (id_a, secret, exporter) = random_inputs(&mut rng);
        let id_b = random_id(&mut rng);
        let binding_a = binding_from(&(id_a, secret, exporter));
        let binding_b = binding_from(&(id_b, secret, exporter));

        let root_a = binding_a.media_root().expect("media root a");
        let root_b = binding_b.media_root().expect("media root b");
        assert_eq!(
            format!("{root_a:?}"),
            "SessionSecret([REDACTED])",
            "iteration {iteration}: media root stays redacted"
        );

        let mut sealer_a = MediaSealer::new(&root_a, context).expect("sealer a");
        let opener_a = MediaOpener::new(&root_a, context).expect("opener a");
        let opener_a_rebuilt = MediaOpener::new(
            &binding_from(&(id_a, secret, exporter))
                .media_root()
                .expect("root a rebuilt"),
            context,
        )
        .expect("opener a rebuilt");
        let opener_b = MediaOpener::new(&root_b, context).expect("opener b");

        let protected = sealer_a
            .seal(0, 0, b"pairing-bound")
            .expect("seal under pairing a");
        let mut plaintext = [0_u8; 16];
        let length = opener_a
            .open(
                parse_media_packet(&protected.datagram).expect("parse a"),
                &mut plaintext,
            )
            .expect("opens under the same root");
        assert_eq!(
            &plaintext[..length],
            b"pairing-bound",
            "iteration {iteration}: plaintext round-trips"
        );

        let mut rebuilt = [0_u8; 16];
        let rebuilt_length = opener_a_rebuilt
            .open(
                parse_media_packet(&protected.datagram).expect("parse rebuilt"),
                &mut rebuilt,
            )
            .expect("rebuilding the same pairing reproduces the root");
        assert_eq!(
            &rebuilt[..rebuilt_length],
            b"pairing-bound",
            "iteration {iteration}: rebuilt root matches"
        );

        let mut wrong = [0_u8; 16];
        assert!(
            opener_b
                .open(
                    parse_media_packet(&protected.datagram).expect("parse b"),
                    &mut wrong
                )
                .is_err(),
            "iteration {iteration}: different pairing id must not open the media"
        );
    }
}

/// Construction rejects a protocol version other than the current one and any
/// TLS exporter length other than the 32-byte contract; verification rejects
/// every non-canonical proof length and any 32-byte proof that is not the
/// exact peer proof.
#[test]
fn noncanonical_versions_and_lengths_are_rejected() {
    let mut rng = XorShift64(0x5041_0000_2026_0016);
    let (id, secret, exporter) = random_inputs(&mut rng);

    assert!(matches!(
        PairingBinding::new(protocol_version().wrapping_add(1), id, secret, &exporter),
        Err(PairingBindingError::InvalidProtocolVersion)
    ));
    assert!(matches!(
        PairingBinding::new(0, id, secret, &exporter),
        Err(PairingBindingError::InvalidProtocolVersion)
    ));

    for length in [
        0_usize,
        PAIRING_TLS_EXPORTER_LEN - 1,
        PAIRING_TLS_EXPORTER_LEN + 1,
    ] {
        let exporter_variant = random_bytes(&mut rng, length);
        assert!(
            matches!(
                PairingBinding::new(protocol_version(), id, secret, &exporter_variant),
                Err(PairingBindingError::InvalidExporterLength)
            ),
            "exporter length {length} rejected"
        );
    }

    let binding = binding_from(&(id, secret, exporter));
    let client_proof = binding
        .local_proof(PairingRole::AndroidClient)
        .expect("client proof");
    for length in [
        0_usize,
        PAIRING_TLS_EXPORTER_LEN - 1,
        PAIRING_TLS_EXPORTER_LEN + 1,
    ] {
        let proof_variant = random_bytes(&mut rng, length);
        assert_eq!(
            binding.verify_peer_proof(PairingRole::DesktopServer, &proof_variant),
            Ok(false),
            "proof length {length} rejected"
        );
    }

    let mut near_miss = client_proof;
    near_miss[0] ^= 0x01;
    assert_eq!(
        binding.verify_peer_proof(PairingRole::DesktopServer, &near_miss),
        Ok(false),
        "single-bit proof difference rejected"
    );
}

/// Arbitrary random inputs never panic: construction succeeds, both role
/// proofs derive 32 bytes and differ, peer verification succeeds, and the
/// media root is available and redacted.
#[test]
fn arbitrary_inputs_never_panic_and_are_deterministic() {
    let mut rng = XorShift64(0x5041_0000_2026_0017);
    for iteration in 0..1_000 {
        let inputs = random_inputs(&mut rng);
        let binding = binding_from(&inputs);
        let client = binding
            .local_proof(PairingRole::AndroidClient)
            .expect("client proof");
        let server = binding
            .local_proof(PairingRole::DesktopServer)
            .expect("server proof");
        assert_eq!(
            client.len(),
            PAIRING_TLS_EXPORTER_LEN,
            "iteration {iteration}"
        );
        assert_eq!(
            server.len(),
            PAIRING_TLS_EXPORTER_LEN,
            "iteration {iteration}"
        );
        assert_ne!(client, server, "iteration {iteration}: roles stay distinct");

        assert_eq!(
            binding.verify_peer_proof(PairingRole::AndroidClient, &server),
            Ok(true),
            "iteration {iteration}: peer proof verifies"
        );

        let root = binding.media_root().expect("media root");
        assert_eq!(
            format!("{root:?}"),
            "SessionSecret([REDACTED])",
            "iteration {iteration}: media root redacted"
        );

        let rebuilt = binding_from(&inputs);
        assert_eq!(
            rebuilt
                .local_proof(PairingRole::AndroidClient)
                .expect("rebuilt client"),
            client,
            "iteration {iteration}: deterministic"
        );
    }
}
