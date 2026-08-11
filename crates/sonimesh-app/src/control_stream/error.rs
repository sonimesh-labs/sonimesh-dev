use core::fmt;

use sonimesh_protocol::{ControlFrameError, ControlNegotiationError};

use crate::{ReconnectGraceError, SessionEvent, SessionState, SessionTransitionError};

/// Failure while coordinating a validated control command with lifecycle state.
#[derive(Debug)]
pub enum ControlStreamError {
    /// Envelope exceeded the protocol framing or semantic contract.
    Protocol(ControlFrameError),
    /// A protocol-valid epoch could not convert to the media `u16` type.
    InvalidEpoch,
    /// A new stream was requested while another session/stream is active.
    ActiveStreamConflict,
    /// A media identity was reused with different validated audio parameters.
    StreamConfigurationConflict,
    /// A pending platform or media change must finish before another one begins.
    PendingChange,
    /// A start command arrived in a state that cannot prepare output.
    StartRequiresReady(SessionState),
    /// A key rotation arrived outside active streaming.
    RotationRequiresStreaming(SessionState),
    /// The local policy exceeds the protocol's fixed capability bound.
    LocalCapabilityPolicyTooLarge,
    /// The authenticated peer hello could not select a compatible control session.
    Negotiation(ControlNegotiationError),
    /// A hello was received after a capability set was already fixed.
    Renegotiation,
    /// An authenticated hello arrived before pairing established a ready session.
    HelloRequiresReady(SessionState),
    /// A stream command arrived before an authenticated hello selected capabilities.
    MissingNegotiation,
    /// Start requested a frame duration that peers did not negotiate.
    UnsupportedFrameDuration(u32),
    /// A start attempted to move a stream back to an earlier epoch.
    StaleEpoch {
        /// Existing active key epoch.
        active: u16,
        /// Rejected key epoch from the command.
        requested: u16,
    },
    /// Completion did not correspond to the prepared action or current state.
    CompletionMismatch,
    /// A control-domain lifecycle acknowledgement was invalid for this state.
    Transition(SessionTransitionError),
    /// This event is owned by a dedicated coordinator operation.
    UnsupportedLifecycleEvent(SessionEvent),
    /// An authenticated control transport can only be installed in `Ready`.
    ControlSessionRequiresReady(SessionState),
    /// The current authenticated control transport must be retired before replacement.
    ControlSessionAlreadyInstalled,
    /// Envelope belongs to a retired or unknown authenticated control transport.
    StaleControlSession,
    /// The coordinator cannot issue another non-reusable control-session token.
    ControlSessionTokenExhausted,
    /// The coordinator cannot issue another non-reusable operation token.
    OperationTokenExhausted,
    /// Reconnect grace was requested outside the reconnecting lifecycle.
    GraceRequiresReconnecting(SessionState),
    /// A reconnect grace window could not be armed or observed.
    ReconnectGrace(ReconnectGraceError),
}

impl fmt::Display for ControlStreamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Protocol(error) => write!(formatter, "control command rejected: {error}"),
            Self::InvalidEpoch => formatter.write_str("control epoch cannot fit media key context"),
            Self::ActiveStreamConflict => {
                formatter.write_str("a different media stream is already active")
            }
            Self::StreamConfigurationConflict => formatter
                .write_str("media stream identity conflicts with existing audio parameters"),
            Self::PendingChange => formatter.write_str("a stream lifecycle change is pending"),
            Self::StartRequiresReady(state) => {
                write!(
                    formatter,
                    "stream start requires Ready state, found {state:?}"
                )
            }
            Self::RotationRequiresStreaming(state) => {
                write!(
                    formatter,
                    "key rotation requires Streaming state, found {state:?}"
                )
            }
            Self::LocalCapabilityPolicyTooLarge => {
                formatter.write_str("local control capability policy exceeds the hard limit")
            }
            Self::Negotiation(error) => write!(formatter, "control negotiation failed: {error}"),
            Self::Renegotiation => {
                formatter.write_str("control capabilities were already negotiated")
            }
            Self::HelloRequiresReady(state) => {
                write!(
                    formatter,
                    "control hello requires Ready state, found {state:?}"
                )
            }
            Self::MissingNegotiation => {
                formatter.write_str("stream command requires a negotiated control hello")
            }
            Self::UnsupportedFrameDuration(duration) => {
                write!(formatter, "frame duration {duration} ms was not negotiated")
            }
            Self::StaleEpoch { active, requested } => {
                write!(
                    formatter,
                    "media key epoch {requested} is older than active epoch {active}"
                )
            }
            Self::CompletionMismatch => {
                formatter.write_str("stream lifecycle completion did not match pending state")
            }
            Self::Transition(error) => {
                write!(formatter, "control lifecycle transition failed: {error}")
            }
            Self::UnsupportedLifecycleEvent(event) => {
                write!(
                    formatter,
                    "session event {event:?} is not a platform acknowledgement"
                )
            }
            Self::ControlSessionRequiresReady(state) => {
                write!(
                    formatter,
                    "authenticated control session requires Ready state, found {state:?}"
                )
            }
            Self::ControlSessionAlreadyInstalled => {
                formatter.write_str("an authenticated control session is already installed")
            }
            Self::StaleControlSession => {
                formatter.write_str("control envelope belongs to a retired session")
            }
            Self::ControlSessionTokenExhausted => {
                formatter.write_str("control session token sequence is exhausted")
            }
            Self::OperationTokenExhausted => {
                formatter.write_str("control operation token sequence is exhausted")
            }
            Self::GraceRequiresReconnecting(state) => {
                write!(
                    formatter,
                    "reconnect grace requires Reconnecting state, found {state:?}"
                )
            }
            Self::ReconnectGrace(error) => {
                write!(formatter, "reconnect grace rejected the operation: {error}")
            }
        }
    }
}

impl std::error::Error for ControlStreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Protocol(error) => Some(error),
            Self::Negotiation(error) => Some(error),
            Self::Transition(error) => Some(error),
            Self::ReconnectGrace(error) => Some(error),
            _ => None,
        }
    }
}
