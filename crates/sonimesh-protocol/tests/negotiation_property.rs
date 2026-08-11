//! Deterministic property coverage for bounded capability negotiation.
//!
//! Capability selection is a protocol boundary: a peer must never receive a
//! feature this build does not understand, and the selected set must always be
//! the canonical intersection of the local policy and the peer advertisement.
//! This test drives negotiation with a fixed pseudo-random mix of local
//! policies and peer hellos and compares every successful selection against an
//! independent set-based model, so ordering, deduplication, or unknown-flag
//! bugs cannot silently change the negotiation contract.

use std::collections::HashSet;

use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, ControlNegotiationError, Hello, MAX_CONTROL_CAPABILITIES,
    MAX_CONTROL_DISPLAY_NAME_BYTES, MAX_CONTROL_SUPPORTED_VERSIONS, negotiate_hello,
};

/// Canonical version-one capabilities in declaration order, mirroring the
/// published hello frame vector and the implementation's known capability list.
const CANONICAL_CAPABILITIES: [Capability; 4] = [
    Capability::FrameDuration10Ms,
    Capability::FrameDuration20Ms,
    Capability::ManualAddress,
    Capability::NetworkDiagnostics,
];

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
}

fn random_local_policy(rng: &mut XorShift64) -> Vec<Capability> {
    const POOL: [Capability; 5] = [
        Capability::Unspecified,
        Capability::FrameDuration10Ms,
        Capability::FrameDuration20Ms,
        Capability::ManualAddress,
        Capability::NetworkDiagnostics,
    ];
    let len = rng.next_usize(POOL.len() + 1);
    (0..len).map(|_| POOL[rng.next_usize(POOL.len())]).collect()
}

/// Generates a peer hello that always passes bounded hello validation, so the
/// remaining contract is version agreement plus capability selection.
fn random_valid_peer_hello(rng: &mut XorShift64) -> Hello {
    let mut supported_versions = Vec::new();
    let version_count = rng.next_usize(MAX_CONTROL_SUPPORTED_VERSIONS + 1);
    for _ in 0..version_count {
        let roll = rng.next_u32() % 4;
        supported_versions.push(match roll {
            0 => CONTROL_PROTOCOL_VERSION,
            1 => CONTROL_PROTOCOL_VERSION + 1,
            2 => 0,
            _ => 10 + rng.next_u32() % 90,
        });
    }

    let mut capabilities = Vec::new();
    let capability_count = rng.next_usize(MAX_CONTROL_CAPABILITIES + 1);
    for _ in 0..capability_count {
        let roll = rng.next_u32() % 6;
        capabilities.push(match roll {
            // CAPABILITY_UNSPECIFIED is never a negotiated feature.
            0 => 0,
            1 => Capability::FrameDuration10Ms as i32,
            2 => Capability::FrameDuration20Ms as i32,
            3 => Capability::ManualAddress as i32,
            4 => Capability::NetworkDiagnostics as i32,
            // An unknown numeric capability must never be selected.
            _ => i32::try_from(100 + rng.next_u32() % 10_000)
                .expect("unknown capability fits in i32"),
        });
    }

    let display_name = match rng.next_u32() % 3 {
        0 => String::new(),
        1 => "Lab peer".to_owned(),
        _ => "x".repeat(1 + rng.next_usize(MAX_CONTROL_DISPLAY_NAME_BYTES)),
    };

    Hello {
        supported_versions,
        capabilities,
        display_name,
    }
}

fn expected_selection(local: &[Capability], peer_capabilities: &[i32]) -> Vec<Capability> {
    let local: HashSet<i32> = local.iter().map(|capability| *capability as i32).collect();
    let peer: HashSet<i32> = peer_capabilities.iter().copied().collect();
    CANONICAL_CAPABILITIES
        .iter()
        .copied()
        .filter(|capability| {
            local.contains(&(*capability as i32)) && peer.contains(&(*capability as i32))
        })
        .collect()
}

#[test]
fn negotiation_selection_matches_an_independent_capability_model() {
    let mut rng = XorShift64(0x0F00_BA5E_C0DE_CAFE);

    for _ in 0..100_000 {
        let local = random_local_policy(&mut rng);
        let peer = random_valid_peer_hello(&mut rng);

        let result = negotiate_hello(&peer, &local);

        if !peer.supported_versions.contains(&CONTROL_PROTOCOL_VERSION) {
            assert!(matches!(
                result,
                Err(ControlNegotiationError::NoCommonProtocolVersion)
            ));
            continue;
        }

        let negotiated = result.expect("valid hello with a common version must negotiate");

        assert_eq!(negotiated.version(), CONTROL_PROTOCOL_VERSION);
        let expected = expected_selection(&local, &peer.capabilities);
        assert_eq!(negotiated.capabilities(), expected.as_slice());

        for capability in negotiated.capabilities() {
            assert!(
                local.contains(capability),
                "selected capability missing locally"
            );
            assert!(
                peer.capabilities.contains(&(*capability as i32)),
                "selected capability missing from peer advertisement"
            );
        }
        let selected: HashSet<i32> = negotiated
            .capabilities()
            .iter()
            .map(|capability| *capability as i32)
            .collect();
        assert_eq!(selected.len(), negotiated.capabilities().len());
        assert!(negotiated.capabilities().len() <= MAX_CONTROL_CAPABILITIES);

        for capability in CANONICAL_CAPABILITIES {
            assert_eq!(
                negotiated.supports(capability),
                expected.contains(&capability)
            );
        }
    }
}
