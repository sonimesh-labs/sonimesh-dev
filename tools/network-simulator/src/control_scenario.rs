//! Deterministic control-session reconnect regression scenario.

use sonimesh_app::{
    ControlStreamAction, ControlStreamCoordinator, PairingAttemptOutcome, PairingId,
    PairingInvitationController, PairingSource, SessionEvent, SessionState,
};
use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, Envelope, Hello, StartStream, envelope,
};

/// Observable result of one reconnect lifecycle simulation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlSimulationReport {
    /// The delayed start completion was rejected after reconnection.
    pub stale_completion_rejected: bool,
    /// The receiver required a fresh hello before accepting a replacement start.
    pub fresh_hello_required: bool,
}

/// Failure while building the fixed deterministic control simulation.
#[derive(Debug)]
pub enum ControlScenarioError {
    /// A bounded pairing operation failed.
    Pairing,
    /// A control envelope or transition violated its expected contract.
    Control,
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
        .map_err(|_| ControlScenarioError::Control)?;
    pair(&mut coordinator, &mut pairing, 1)?;
    hello(&mut coordinator, &capabilities)?;
    let ControlStreamAction::Start { token, request } = coordinator
        .accept(&start())
        .map_err(|_| ControlScenarioError::Control)?
    else {
        return Err(ControlScenarioError::Control);
    };
    coordinator
        .acknowledge_platform_event(SessionEvent::ConnectionLost)
        .map_err(|_| ControlScenarioError::Control)?;
    coordinator
        .acknowledge_platform_event(SessionEvent::Reconnected)
        .map_err(|_| ControlScenarioError::Control)?;
    let stale_completion_rejected = coordinator.complete_start(token, request).is_err();
    let fresh_hello_required = coordinator.accept(&start()).is_err();
    Ok(ControlSimulationReport {
        stale_completion_rejected,
        fresh_hello_required,
    })
}

fn pair(
    c: &mut ControlStreamCoordinator,
    p: &mut PairingInvitationController<1>,
    n: u8,
) -> Result<(), ControlScenarioError> {
    c.begin_pairing(p, PairingId::new([n; 16]), 0, 10, 1)
        .map_err(|_| ControlScenarioError::Pairing)?;
    match c
        .record_pairing_proof(p, 1, PairingSource::new([n; 16]), true)
        .map_err(|_| ControlScenarioError::Pairing)?
    {
        PairingAttemptOutcome::Paired => Ok(()),
        PairingAttemptOutcome::Rejected { .. } => Err(ControlScenarioError::Pairing),
    }
}

fn hello(
    c: &mut ControlStreamCoordinator,
    caps: &[Capability],
) -> Result<(), ControlScenarioError> {
    let frame = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 1,
        payload: Some(envelope::Payload::Hello(Hello {
            supported_versions: vec![CONTROL_PROTOCOL_VERSION],
            capabilities: caps.iter().map(|x| *x as i32).collect(),
            display_name: "sim".into(),
        })),
    };
    matches!(c.accept(&frame), Ok(ControlStreamAction::Negotiated(_)))
        .then_some(())
        .ok_or(ControlScenarioError::Control)
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
    use super::*;
    #[test]
    fn reconnect_retires_old_control_work() {
        let report = simulate_control_reconnect().unwrap();
        assert!(report.stale_completion_rejected && report.fresh_hello_required);
    }
}
