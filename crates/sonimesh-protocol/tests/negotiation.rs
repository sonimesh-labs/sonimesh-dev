//! Capability negotiation tests independent of transport authentication.

use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, ControlNegotiationError, Hello, MAX_CONTROL_CAPABILITIES,
    negotiate_hello,
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
