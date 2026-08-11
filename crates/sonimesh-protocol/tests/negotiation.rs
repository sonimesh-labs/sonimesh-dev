//! Capability negotiation tests independent of transport authentication.

use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, ControlNegotiationError, Hello, MAX_CONTROL_CAPABILITIES,
    decode_control_frame, envelope, negotiate_hello,
};

fn peer(versions: Vec<u32>, capabilities: Vec<i32>) -> Hello {
    Hello {
        supported_versions: versions,
        capabilities,
        display_name: "Lab sender".to_owned(),
    }
}

#[test]
fn selects_current_version_and_canonical_common_capabilities() {
    let peer = peer(
        vec![99, CONTROL_PROTOCOL_VERSION, CONTROL_PROTOCOL_VERSION],
        vec![
            Capability::NetworkDiagnostics as i32,
            Capability::FrameDuration20Ms as i32,
            Capability::FrameDuration20Ms as i32,
            9_999,
        ],
    );

    let negotiated = negotiate_hello(
        &peer,
        &[
            Capability::NetworkDiagnostics,
            Capability::FrameDuration20Ms,
            Capability::ManualAddress,
        ],
    )
    .unwrap();

    assert_eq!(negotiated.version(), CONTROL_PROTOCOL_VERSION);
    assert_eq!(
        negotiated.capabilities(),
        [
            Capability::FrameDuration20Ms,
            Capability::NetworkDiagnostics,
        ]
    );
    assert!(negotiated.supports(Capability::FrameDuration20Ms));
    assert!(!negotiated.supports(Capability::ManualAddress));
}

#[test]
fn rejects_incompatible_versions_and_bounded_policy_violations() {
    assert!(matches!(
        negotiate_hello(&peer(vec![2], Vec::new()), &[]),
        Err(ControlNegotiationError::NoCommonProtocolVersion)
    ));

    let oversized_policy = vec![Capability::ManualAddress; MAX_CONTROL_CAPABILITIES + 1];
    assert!(matches!(
        negotiate_hello(
            &peer(vec![CONTROL_PROTOCOL_VERSION], Vec::new()),
            &oversized_policy
        ),
        Err(ControlNegotiationError::LocalTooManyCapabilities)
    ));
}

#[test]
fn rejects_invalid_peer_hello_before_selecting_features() {
    let invalid = peer(
        vec![CONTROL_PROTOCOL_VERSION],
        vec![Capability::ManualAddress as i32; MAX_CONTROL_CAPABILITIES + 1],
    );

    assert!(matches!(
        negotiate_hello(&invalid, &[Capability::ManualAddress]),
        Err(ControlNegotiationError::PeerHello(_))
    ));
}

#[test]
fn negotiates_the_published_hello_frame_and_survives_byte_mutations() {
    const PUBLISHED_HELLO_FRAME: [u8; 28] = [
        0x00, 0x00, 0x00, 0x18, 0x08, 0x01, 0x10, 0x07, 0x52, 0x12, 0x0A, 0x01, 0x01, 0x12, 0x01,
        0x02, 0x1A, 0x0A, 0x4C, 0x61, 0x62, 0x20, 0x73, 0x65, 0x6E, 0x64, 0x65, 0x72,
    ];
    const LOCAL: [Capability; 2] = [Capability::FrameDuration10Ms, Capability::FrameDuration20Ms];

    let envelope =
        decode_control_frame(&PUBLISHED_HELLO_FRAME).expect("published hello frame decodes");
    let Some(envelope::Payload::Hello(hello)) = envelope.payload.as_ref() else {
        panic!("published hello frame carries a hello");
    };
    let negotiated = negotiate_hello(hello, &LOCAL).expect("published hello negotiates");
    assert!(negotiated.supports(Capability::FrameDuration20Ms));
    assert!(!negotiated.supports(Capability::FrameDuration10Ms));

    // The negotiation fuzz target mutates the published frame; every mutation
    // that still decodes must reach negotiation without panicking.
    for mutation in 0_u8..=255 {
        let mut near_valid = PUBLISHED_HELLO_FRAME;
        near_valid[0] ^= mutation;
        if let Ok(envelope) = decode_control_frame(&near_valid) {
            if let Some(envelope::Payload::Hello(hello)) = envelope.payload.as_ref() {
                let _ = negotiate_hello(hello, &LOCAL);
            }
        }
    }
}
