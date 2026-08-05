//! Deterministic control-session reconnect regression scenario.

use sonimesh_app::{
    ControlSessionToken, ControlStreamAction, ControlStreamCoordinator, ControlStreamError,
    PairingAttemptOutcome, PairingId, PairingInvitationController, PairingInvitationError,
    PairingProofVerification, PairingSource, SessionEvent, SessionState,
};
use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, ControlFrameError, Envelope, Hello, StartStream,
    encode_control_frame, envelope,
};

use crate::{NetworkProfile, ProfileError, VirtualNetwork};

/// Rejections observed for work owned by the retired control transport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetiredControlSessionReport {
    /// The delayed start completion was rejected after reconnection.
    pub completion_rejected: bool,
    /// A reordered old start arriving after fresh negotiation was rejected.
    pub command_rejected: bool,
}

/// Admission behavior observed for the freshly installed control transport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FreshControlSessionReport {
    /// The receiver required a fresh hello before accepting a replacement start.
    pub hello_required: bool,
    /// The same command from the freshly installed transport was accepted.
    pub command_accepted: bool,
}

/// Observable result of one reconnect lifecycle simulation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlSimulationReport {
    /// Outcomes for delayed work owned by the retired transport.
    pub retired: RetiredControlSessionReport,
    /// Outcomes for work delivered through the replacement transport.
    pub fresh: FreshControlSessionReport,
}

/// Failure while building the fixed deterministic control simulation.
#[derive(Debug)]
pub enum ControlScenarioError {
    /// A bounded pairing operation failed.
    Pairing(PairingInvitationError),
    /// A control envelope or transition violated its expected contract.
    Control(ControlStreamError),
    /// A control envelope could not be encoded for deterministic transport.
    Protocol(ControlFrameError),
    /// The deterministic virtual network profile was invalid.
    Network(ProfileError),
    /// The fixed scenario would require a frame length larger than `u64`.
    FrameLengthOverflow,
    /// The fixed deterministic scenario violated its expected state machine.
    UnexpectedScenarioState,
}

impl core::fmt::Display for ControlScenarioError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Pairing(error) => write!(formatter, "control scenario pairing failed: {error}"),
            Self::Control(error) => {
                write!(formatter, "control scenario control step failed: {error}")
            }
            Self::Protocol(error) => write!(formatter, "control scenario envelope failed: {error}"),
            Self::Network(error) => write!(
                formatter,
                "control scenario network profile failed: {error}"
            ),
            Self::FrameLengthOverflow => {
                formatter.write_str("control scenario frame length overflowed")
            }
            Self::UnexpectedScenarioState => {
                formatter.write_str("control scenario violated its expected state")
            }
        }
    }
}

impl std::error::Error for ControlScenarioError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Pairing(error) => Some(error),
            Self::Control(error) => Some(error),
            Self::Protocol(error) => Some(error),
            Self::Network(error) => Some(error),
            Self::FrameLengthOverflow | Self::UnexpectedScenarioState => None,
        }
    }
}

/// Verifies that reconnect with fresh session material invalidates old work.
///
/// # Errors
///
/// Returns [`ControlScenarioError`] when the fixed pairing or control lifecycle
/// does not reach its expected state.
pub fn simulate_control_reconnect() -> Result<ControlSimulationReport, ControlScenarioError> {
    let capabilities = [Capability::FrameDuration20Ms];
    let mut pairing = PairingInvitationController::<1>::new();
    let mut coordinator = ControlStreamCoordinator::new(SessionState::IDLE, &capabilities)
        .map_err(ControlScenarioError::Control)?;
    let retired_session = pair(&mut coordinator, &mut pairing, 1)?;
    let mut profile = NetworkProfile::stable(0xC011_2026);
    profile.base_delay_ms = 15;
    profile.jitter_ms = 0;
    let mut network = VirtualNetwork::new(profile).map_err(ControlScenarioError::Network)?;
    let hello = deliver(&mut network, hello_frame(&capabilities))?;
    if !matches!(
        coordinator.accept(retired_session, &hello),
        Ok(ControlStreamAction::Negotiated(_))
    ) {
        return Err(ControlScenarioError::UnexpectedScenarioState);
    }
    let start_frame = deliver(&mut network, start())?;
    let ControlStreamAction::Start { token, request } = coordinator
        .accept(retired_session, &start_frame)
        .map_err(ControlScenarioError::Control)?
    else {
        return Err(ControlScenarioError::UnexpectedScenarioState);
    };

    let mut delayed_profile = NetworkProfile::stable(0xC011_2027);
    delayed_profile.base_delay_ms = 15;
    delayed_profile.jitter_ms = 0;
    delayed_profile.reorder_percent = 100;
    delayed_profile.reorder_delay_ms = 30;
    let mut delayed_network =
        VirtualNetwork::new(delayed_profile).map_err(ControlScenarioError::Network)?;
    submit(&mut delayed_network, start())?;

    coordinator
        .acknowledge_platform_event(SessionEvent::ConnectionLost)
        .map_err(ControlScenarioError::Control)?;
    coordinator
        .acknowledge_platform_event(SessionEvent::Reconnected)
        .map_err(ControlScenarioError::Control)?;
    let stale_completion_rejected = coordinator.complete_start(token, request).is_err();
    let fresh_session = coordinator
        .install_authenticated_control_session()
        .map_err(ControlScenarioError::Control)?;
    let restarted_start = deliver(&mut network, start())?;
    let fresh_hello_required = matches!(
        coordinator.accept(fresh_session, &restarted_start),
        Err(ControlStreamError::MissingNegotiation)
    );
    let fresh_hello = deliver(&mut network, hello_frame(&capabilities))?;
    if !matches!(
        coordinator.accept(fresh_session, &fresh_hello),
        Ok(ControlStreamAction::Negotiated(_))
    ) {
        return Err(ControlScenarioError::UnexpectedScenarioState);
    }

    let delayed = delayed_network.drain_ready(45);
    let stale_session_command_rejected = delayed.first().is_some_and(|delivery| {
        matches!(
            coordinator.accept(retired_session, &delivery.payload),
            Err(ControlStreamError::StaleControlSession)
        )
    });
    let fresh_command_accepted = matches!(
        coordinator.accept(fresh_session, &restarted_start),
        Ok(ControlStreamAction::Start { .. })
    );
    Ok(ControlSimulationReport {
        retired: RetiredControlSessionReport {
            completion_rejected: stale_completion_rejected,
            command_rejected: stale_session_command_rejected,
        },
        fresh: FreshControlSessionReport {
            hello_required: fresh_hello_required,
            command_accepted: fresh_command_accepted,
        },
    })
}

fn pair(
    c: &mut ControlStreamCoordinator,
    p: &mut PairingInvitationController<1>,
    n: u8,
) -> Result<ControlSessionToken, ControlScenarioError> {
    c.begin_pairing(p, PairingId::new([n; 16]), 0, 10, 1)
        .map_err(ControlScenarioError::Pairing)?;
    match c
        .verify_pairing_proof(p, 1, PairingSource::new([n; 16]), || {
            PairingProofVerification::Verified
        })
        .map_err(ControlScenarioError::Pairing)?
    {
        PairingAttemptOutcome::Paired => c
            .install_authenticated_control_session()
            .map_err(ControlScenarioError::Control),
        PairingAttemptOutcome::Rejected { .. } => {
            Err(ControlScenarioError::UnexpectedScenarioState)
        }
    }
}

fn hello_frame(caps: &[Capability]) -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 1,
        payload: Some(envelope::Payload::Hello(Hello {
            supported_versions: vec![CONTROL_PROTOCOL_VERSION],
            capabilities: caps.iter().map(|x| *x as i32).collect(),
            display_name: "sim".into(),
        })),
    }
}

fn deliver(
    network: &mut VirtualNetwork<Envelope>,
    frame: Envelope,
) -> Result<Envelope, ControlScenarioError> {
    submit(network, frame)?;
    let mut ready = network.drain_ready(15);
    if ready.len() != 1 {
        return Err(ControlScenarioError::UnexpectedScenarioState);
    }
    Ok(ready
        .pop()
        .ok_or(ControlScenarioError::UnexpectedScenarioState)?
        .payload)
}

fn submit(
    network: &mut VirtualNetwork<Envelope>,
    frame: Envelope,
) -> Result<(), ControlScenarioError> {
    let payload_bytes = u64::try_from(
        encode_control_frame(&frame)
            .map_err(ControlScenarioError::Protocol)?
            .len(),
    )
    .map_err(|_| ControlScenarioError::FrameLengthOverflow)?;
    network.send(0, frame, payload_bytes);
    Ok(())
}

fn start() -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 2,
        payload: Some(envelope::Payload::StartStream(StartStream {
            media_session_id: 1,
            stream_id: 1,
            sample_rate_hz: 48_000,
            channels: 2,
            frame_duration_ms: 20,
            media_key_epoch: 0,
        })),
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    #[test]
    fn reconnect_retires_old_control_work() {
        let report = simulate_control_reconnect().unwrap();
        assert!(
            report.retired.completion_rejected
                && report.retired.command_rejected
                && report.fresh.hello_required
                && report.fresh.command_accepted
        );
    }

    #[test]
    fn preserved_error_variants_expose_their_causes() {
        let pairing = ControlScenarioError::Pairing(PairingInvitationError::MissingInvitation);
        assert!(
            Error::source(&pairing)
                .and_then(|source| source.downcast_ref::<PairingInvitationError>())
                .is_some()
        );

        let control = ControlScenarioError::Control(ControlStreamError::MissingNegotiation);
        assert!(
            Error::source(&control)
                .and_then(|source| source.downcast_ref::<ControlStreamError>())
                .is_some()
        );

        let protocol = ControlScenarioError::Protocol(ControlFrameError::TruncatedLength);
        assert!(
            Error::source(&protocol)
                .and_then(|source| source.downcast_ref::<ControlFrameError>())
                .is_some()
        );

        let network = ControlScenarioError::Network(ProfileError::DelayTooLarge);
        assert!(
            Error::source(&network)
                .and_then(|source| source.downcast_ref::<ProfileError>())
                .is_some()
        );

        assert!(Error::source(&ControlScenarioError::UnexpectedScenarioState).is_none());
    }
}
