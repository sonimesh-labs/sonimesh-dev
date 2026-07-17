//! Deterministic control-session reconnect regression scenario.

use sonimesh_app::{
    ControlStreamAction, ControlStreamCoordinator, PairingAttemptOutcome, PairingId,
    PairingInvitationController, PairingSource, SessionEvent, SessionState,
};
use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, Envelope, Hello, StartStream, encode_control_frame,
    envelope,
};

use crate::{NetworkProfile, VirtualNetwork};

/// Observable result of one reconnect lifecycle simulation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlSimulationReport {
    /// The delayed start completion was rejected after reconnection.
    pub stale_completion_rejected: bool,
    /// The receiver required a fresh hello before accepting a replacement start.
    pub fresh_hello_required: bool,
    /// A reordered old start arriving after reconnect was rejected.
    pub delayed_command_rejected: bool,
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
    let mut profile = NetworkProfile::stable(0xC011_2026);
    profile.base_delay_ms = 15;
    profile.jitter_ms = 0;
    let mut network = VirtualNetwork::new(profile).map_err(|_| ControlScenarioError::Control)?;
    let hello = deliver(&mut network, hello_frame(&capabilities))?;
    if !matches!(
        coordinator.accept(&hello),
        Ok(ControlStreamAction::Negotiated(_))
    ) {
        return Err(ControlScenarioError::Control);
    }
    let start_frame = deliver(&mut network, start())?;
    let ControlStreamAction::Start { token, request } = coordinator
        .accept(&start_frame)
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
    let mut delayed_profile = NetworkProfile::stable(0xC011_2027);
    delayed_profile.base_delay_ms = 15;
    delayed_profile.jitter_ms = 0;
    delayed_profile.reorder_percent = 100;
    delayed_profile.reorder_delay_ms = 30;
    let mut delayed_network =
        VirtualNetwork::new(delayed_profile).map_err(|_| ControlScenarioError::Control)?;
    submit(&mut delayed_network, start())?;
    let delayed = delayed_network.drain_ready(45);
    let delayed_command_rejected = delayed
        .first()
        .is_some_and(|delivery| coordinator.accept(&delivery.payload).is_err());
    let restarted_start = deliver(&mut network, start())?;
    let fresh_hello_required = coordinator.accept(&restarted_start).is_err();
    Ok(ControlSimulationReport {
        stale_completion_rejected,
        fresh_hello_required,
        delayed_command_rejected,
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
        return Err(ControlScenarioError::Control);
    }
    Ok(ready.pop().ok_or(ControlScenarioError::Control)?.payload)
}

fn submit(
    network: &mut VirtualNetwork<Envelope>,
    frame: Envelope,
) -> Result<(), ControlScenarioError> {
    let payload_bytes = u64::try_from(
        encode_control_frame(&frame)
            .map_err(|_| ControlScenarioError::Control)?
            .len(),
    )
    .map_err(|_| ControlScenarioError::Control)?;
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
    use super::*;
    #[test]
    fn reconnect_retires_old_control_work() {
        let report = simulate_control_reconnect().unwrap();
        assert!(
            report.stale_completion_rejected
                && report.fresh_hello_required
                && report.delayed_command_rejected
        );
    }
}
