use core::fmt;

use crate::control::validate_hello;
use crate::{
    CONTROL_PROTOCOL_VERSION, Capability, ControlFrameError, Hello, MAX_CONTROL_CAPABILITIES,
};

/// Canonical set of version-one capabilities understood by this implementation.
const KNOWN_CAPABILITIES: [Capability; 4] = [
    Capability::FrameDuration10Ms,
    Capability::FrameDuration20Ms,
    Capability::ManualAddress,
    Capability::NetworkDiagnostics,
];

/// Result of bounded capability negotiation for the current control protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NegotiatedControl {
    version: u32,
    capabilities: [Capability; MAX_CONTROL_CAPABILITIES],
    capability_count: usize,
}

impl NegotiatedControl {
    /// Returns the exact control protocol version selected by both peers.
    #[must_use]
    pub const fn version(&self) -> u32 {
        self.version
    }

    /// Returns canonical capabilities selected by both peers.
    #[must_use]
    pub fn capabilities(&self) -> &[Capability] {
        &self.capabilities[..self.capability_count]
    }

    /// Returns whether a version-one capability was selected by both peers.
    #[must_use]
    pub fn supports(&self, capability: Capability) -> bool {
        self.capabilities().contains(&capability)
    }
}

/// Failure while selecting a protocol version and common capabilities.
#[derive(Debug)]
pub enum ControlNegotiationError {
    /// The local capability policy exceeds the protocol's fixed maximum.
    LocalTooManyCapabilities,
    /// The peer's hello message violated the bounded control schema.
    PeerHello(ControlFrameError),
    /// The peer did not advertise the exact control version this build supports.
    NoCommonProtocolVersion,
}

impl fmt::Display for ControlNegotiationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LocalTooManyCapabilities => {
                formatter.write_str("local control capability policy exceeds the hard limit")
            }
            Self::PeerHello(error) => write!(formatter, "peer hello is invalid: {error}"),
            Self::NoCommonProtocolVersion => {
                formatter.write_str("peer does not support this control protocol version")
            }
        }
    }
}

impl std::error::Error for ControlNegotiationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::PeerHello(error) => Some(error),
            Self::LocalTooManyCapabilities | Self::NoCommonProtocolVersion => None,
        }
    }
}

/// Selects the current protocol version and canonical common capabilities.
///
/// `local_capabilities` is an application policy, not untrusted wire input. It
/// is still bounded so callers cannot accidentally introduce an unbounded
/// configuration surface. Unknown numeric capabilities advertised by the peer
/// are ignored for additive compatibility and never become selected features.
///
/// # Errors
///
/// Returns [`ControlNegotiationError`] when the local policy exceeds the hard
/// bound, the peer hello is malformed, or no common control version exists.
pub fn negotiate_hello(
    peer: &Hello,
    local_capabilities: &[Capability],
) -> Result<NegotiatedControl, ControlNegotiationError> {
    if local_capabilities.len() > MAX_CONTROL_CAPABILITIES {
        return Err(ControlNegotiationError::LocalTooManyCapabilities);
    }
    validate_hello(peer).map_err(ControlNegotiationError::PeerHello)?;
    if !peer.supported_versions.contains(&CONTROL_PROTOCOL_VERSION) {
        return Err(ControlNegotiationError::NoCommonProtocolVersion);
    }

    let mut capabilities = [Capability::Unspecified; MAX_CONTROL_CAPABILITIES];
    let mut capability_count = 0;
    for capability in KNOWN_CAPABILITIES {
        if local_capabilities.contains(&capability)
            && peer.capabilities.contains(&(capability as i32))
        {
            capabilities[capability_count] = capability;
            capability_count += 1;
        }
    }

    Ok(NegotiatedControl {
        version: CONTROL_PROTOCOL_VERSION,
        capabilities,
        capability_count,
    })
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;
    use crate::{MAX_CONTROL_DISPLAY_NAME_BYTES, MAX_CONTROL_SUPPORTED_VERSIONS};

    fn hello(supported_versions: Vec<u32>, capabilities: Vec<i32>, display_name: &str) -> Hello {
        Hello {
            supported_versions,
            capabilities,
            display_name: display_name.to_owned(),
        }
    }

    fn cap(capability: Capability) -> i32 {
        capability as i32
    }

    #[test]
    fn selects_current_version_and_canonical_common_capabilities() {
        let peer = hello(
            vec![99, CONTROL_PROTOCOL_VERSION, CONTROL_PROTOCOL_VERSION],
            vec![
                cap(Capability::NetworkDiagnostics),
                cap(Capability::FrameDuration20Ms),
                cap(Capability::FrameDuration20Ms),
                9_999,
            ],
            "Lab sender",
        );
        let local = [
            Capability::NetworkDiagnostics,
            Capability::FrameDuration20Ms,
            Capability::ManualAddress,
        ];

        let negotiated = negotiate_hello(&peer, &local).unwrap();

        assert_eq!(negotiated.version(), CONTROL_PROTOCOL_VERSION);
        assert_eq!(
            negotiated.capabilities(),
            &[
                Capability::FrameDuration20Ms,
                Capability::NetworkDiagnostics
            ]
        );
    }

    #[test]
    fn unknown_peer_capabilities_never_become_selected() {
        let peer = hello(
            vec![CONTROL_PROTOCOL_VERSION],
            vec![9_999, cap(Capability::ManualAddress)],
            "Lab sender",
        );

        let negotiated = negotiate_hello(&peer, &[Capability::ManualAddress]).unwrap();

        assert_eq!(negotiated.capabilities(), &[Capability::ManualAddress]);
        assert!(negotiated.supports(Capability::ManualAddress));
        assert!(!negotiated.supports(Capability::NetworkDiagnostics));
        assert!(!negotiated.supports(Capability::Unspecified));
    }

    #[test]
    fn unspecified_peer_value_never_selects_a_capability() {
        let peer = hello(
            vec![CONTROL_PROTOCOL_VERSION],
            vec![cap(Capability::Unspecified), cap(Capability::ManualAddress)],
            "Lab sender",
        );

        let negotiated =
            negotiate_hello(&peer, &[Capability::Unspecified, Capability::ManualAddress]).unwrap();

        assert_eq!(negotiated.capabilities(), &[Capability::ManualAddress]);
        assert!(!negotiated.supports(Capability::Unspecified));
    }

    #[test]
    fn canonical_order_is_independent_of_advertisement_order() {
        // Peer and local orders both differ from the canonical KNOWN_CAPABILITIES order.
        let peer = hello(
            vec![CONTROL_PROTOCOL_VERSION],
            vec![
                cap(Capability::NetworkDiagnostics),
                cap(Capability::ManualAddress),
                cap(Capability::FrameDuration10Ms),
            ],
            "Lab sender",
        );
        let local = [
            Capability::NetworkDiagnostics,
            Capability::FrameDuration10Ms,
            Capability::ManualAddress,
        ];

        let negotiated = negotiate_hello(&peer, &local).unwrap();

        assert_eq!(
            negotiated.capabilities(),
            &[
                Capability::FrameDuration10Ms,
                Capability::ManualAddress,
                Capability::NetworkDiagnostics,
            ]
        );
    }

    #[test]
    fn local_capability_not_advertised_by_peer_is_not_selected() {
        let peer = hello(
            vec![CONTROL_PROTOCOL_VERSION],
            vec![cap(Capability::FrameDuration20Ms)],
            "Lab sender",
        );

        let negotiated = negotiate_hello(
            &peer,
            &[Capability::FrameDuration10Ms, Capability::FrameDuration20Ms],
        )
        .unwrap();

        assert_eq!(negotiated.capabilities(), &[Capability::FrameDuration20Ms]);
        assert!(negotiated.supports(Capability::FrameDuration20Ms));
        assert!(!negotiated.supports(Capability::FrameDuration10Ms));
    }

    #[test]
    fn empty_intersection_selects_version_but_no_capabilities() {
        let peer = hello(vec![CONTROL_PROTOCOL_VERSION], vec![], "Lab sender");

        let negotiated = negotiate_hello(&peer, &[Capability::ManualAddress]).unwrap();

        assert_eq!(negotiated.version(), CONTROL_PROTOCOL_VERSION);
        assert!(negotiated.capabilities().is_empty());
    }

    #[test]
    fn rejects_when_peer_lacks_the_current_protocol_version() {
        let peer = hello(vec![2], vec![], "Lab sender");

        let error = negotiate_hello(&peer, &[]).unwrap_err();

        assert!(matches!(
            error,
            ControlNegotiationError::NoCommonProtocolVersion
        ));
    }

    #[test]
    fn rejects_oversized_local_policy() {
        let peer = hello(vec![CONTROL_PROTOCOL_VERSION], vec![], "Lab sender");
        let local = vec![Capability::ManualAddress; MAX_CONTROL_CAPABILITIES + 1];

        let error = negotiate_hello(&peer, &local).unwrap_err();

        assert!(matches!(
            error,
            ControlNegotiationError::LocalTooManyCapabilities
        ));
    }

    #[test]
    fn local_policy_boundary_is_accepted() {
        let peer = hello(vec![CONTROL_PROTOCOL_VERSION], vec![], "Lab sender");
        let local = vec![Capability::ManualAddress; MAX_CONTROL_CAPABILITIES];

        assert!(negotiate_hello(&peer, &local).is_ok());
    }

    #[test]
    fn rejects_invalid_peer_hello_before_selecting_features() {
        let oversized = vec![cap(Capability::ManualAddress); MAX_CONTROL_CAPABILITIES + 1];
        let peer = hello(vec![CONTROL_PROTOCOL_VERSION], oversized, "Lab sender");

        let error = negotiate_hello(&peer, &[Capability::ManualAddress]).unwrap_err();

        assert!(matches!(
            error,
            ControlNegotiationError::PeerHello(ControlFrameError::TooManyCapabilities)
        ));
    }

    #[test]
    fn rejects_oversized_version_list() {
        let peer = hello(
            vec![0; MAX_CONTROL_SUPPORTED_VERSIONS + 1],
            vec![],
            "Lab sender",
        );

        let error = negotiate_hello(&peer, &[]).unwrap_err();

        assert!(matches!(
            error,
            ControlNegotiationError::PeerHello(ControlFrameError::TooManySupportedVersions)
        ));
    }

    #[test]
    fn rejects_oversized_display_name_and_accepts_byte_boundary() {
        let oversized = "a".repeat(MAX_CONTROL_DISPLAY_NAME_BYTES + 1);
        let error = negotiate_hello(
            &hello(vec![CONTROL_PROTOCOL_VERSION], vec![], &oversized),
            &[],
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ControlNegotiationError::PeerHello(ControlFrameError::DisplayNameTooLong)
        ));

        let boundary = "a".repeat(MAX_CONTROL_DISPLAY_NAME_BYTES);
        let negotiated = negotiate_hello(
            &hello(vec![CONTROL_PROTOCOL_VERSION], vec![], &boundary),
            &[],
        )
        .unwrap();
        assert_eq!(negotiated.version(), CONTROL_PROTOCOL_VERSION);
    }

    #[test]
    fn display_name_limit_is_utf8_bytes_not_characters() {
        // "界" is three UTF-8 bytes: 42 chars fit (126 bytes), 43 do not (129 bytes).
        let accepted = "界".repeat(42);
        assert!(
            negotiate_hello(
                &hello(vec![CONTROL_PROTOCOL_VERSION], vec![], &accepted),
                &[]
            )
            .is_ok()
        );

        let rejected = "界".repeat(43);
        let error = negotiate_hello(
            &hello(vec![CONTROL_PROTOCOL_VERSION], vec![], &rejected),
            &[],
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ControlNegotiationError::PeerHello(ControlFrameError::DisplayNameTooLong)
        ));
    }

    #[test]
    fn negotiation_errors_keep_stable_display_and_cause() {
        for (error, expected) in [
            (
                ControlNegotiationError::LocalTooManyCapabilities,
                "local control capability policy exceeds the hard limit",
            ),
            (
                ControlNegotiationError::NoCommonProtocolVersion,
                "peer does not support this control protocol version",
            ),
        ] {
            assert_eq!(error.to_string(), expected);
            assert!(Error::source(&error).is_none());
        }

        let peer_error = ControlNegotiationError::PeerHello(ControlFrameError::TooManyCapabilities);
        assert_eq!(
            peer_error.to_string(),
            "peer hello is invalid: control hello has too many capabilities"
        );
        assert!(Error::source(&peer_error).is_some());
    }
}
