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
