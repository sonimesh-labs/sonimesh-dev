//! Low-rate control snapshot tests without UI or platform dependencies.

use sonimesh_app::{
    OutputController, OutputSelection, PairingAttemptOutcome, PairingId,
    PairingInvitationController, PairingSnapshotState, PairingSource, SessionEvent, SessionPhase,
    SessionState,
};
use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceInfo, OutputError, OutputHealth, RenderSource,
    RenderSourceFactory, RenderSourceFactoryError, SampleFormat,
};

struct Silence;

impl RenderSource for Silence {
    fn render(&mut self, output: &mut [f32]) {
        output.fill(0.0);
    }
}

struct TestFactory;

impl RenderSourceFactory for TestFactory {
    fn create(
        &mut self,
        _config: &OutputConfig,
    ) -> Result<Box<dyn RenderSource>, RenderSourceFactoryError> {
        Ok(Box::new(Silence))
    }
}

struct TestOutput {
    health: OutputHealth,
}

impl AudioOutput for TestOutput {
    fn devices(&self) -> Result<Vec<OutputDeviceInfo>, OutputError> {
        Ok(Vec::new())
    }

    fn start(
        &mut self,
        _device: Option<&sonimesh_platform::OutputDeviceId>,
        _config: OutputConfig,
        _source: Box<dyn RenderSource>,
    ) -> Result<(), OutputError> {
        Ok(())
    }

    fn stop(&mut self) {}

    fn health(&self) -> OutputHealth {
        self.health
    }
}

fn controller() -> OutputController<TestOutput, TestFactory> {
    OutputController::new(
        TestOutput {
            health: OutputHealth::Healthy,
        },
        TestFactory,
    )
}

fn selection() -> OutputSelection {
    OutputSelection {
        device: None,
        config: OutputConfig {
            sample_rate_hz: 48_000,
            channels: 2,
            sample_format: SampleFormat::Float32,
            requested_callback_frames: Some(480),
            maximum_callback_frames: 1_024,
        },
    }
}

#[test]
fn empty_pairing_owner_projects_an_inactive_idle_snapshot() {
    let mut pairing = PairingInvitationController::<1>::new();
    let mut session = SessionState::IDLE;
    let output = controller();

    let snapshot = pairing.snapshot_at(&mut session, 100, &output).unwrap();

    assert_eq!(snapshot.session(), SessionPhase::Idle);
    assert_eq!(snapshot.pairing(), PairingSnapshotState::Inactive);
    assert_eq!(snapshot.output(), output.snapshot());
}

#[test]
fn active_invitation_projects_the_pairing_receiver_phase() {
    let mut pairing = PairingInvitationController::<1>::new();
    let mut session = SessionState::IDLE;
    let output = controller();
    pairing
        .begin(&mut session, PairingId::new([3; 16]), 100, 50, 1)
        .unwrap();

    let snapshot = pairing.snapshot_at(&mut session, 101, &output).unwrap();

    assert_eq!(snapshot.session(), SessionPhase::Pairing);
    assert_eq!(snapshot.pairing(), PairingSnapshotState::Active);
    assert_eq!(snapshot.output(), output.snapshot());
}

#[test]
fn accepted_pairing_projects_the_ready_receiver_phase() {
    let mut pairing = PairingInvitationController::<1>::new();
    let mut session = SessionState::IDLE;
    let output = controller();
    pairing
        .begin(&mut session, PairingId::new([7; 16]), 100, 50, 1)
        .unwrap();
    pairing
        .record_proof(&mut session, 101, PairingSource::new([9; 16]), true)
        .unwrap();

    let snapshot = pairing.snapshot_at(&mut session, 102, &output).unwrap();

    assert_eq!(snapshot.session(), SessionPhase::Ready);
    assert_eq!(snapshot.pairing(), PairingSnapshotState::Consumed);
    assert_eq!(snapshot.output(), output.snapshot());
}

#[test]
fn snapshot_observation_expires_an_invitation_and_returns_session_to_idle() {
    let mut pairing = PairingInvitationController::<1>::new();
    let mut session = SessionState::IDLE;
    let output = controller();
    pairing
        .begin(&mut session, PairingId::new([11; 16]), 100, 50, 1)
        .unwrap();

    let snapshot = pairing.snapshot_at(&mut session, 150, &output).unwrap();

    assert_eq!(snapshot.session(), SessionPhase::Idle);
    assert_eq!(snapshot.pairing(), PairingSnapshotState::Expired);
    assert_eq!(snapshot.output(), output.snapshot());
    assert_eq!(session.phase(), SessionPhase::Idle);
}

#[test]
fn snapshot_clock_regression_fails_closed_and_retires_the_invitation() {
    let mut pairing = PairingInvitationController::<1>::new();
    let mut session = SessionState::IDLE;
    let output = controller();
    pairing
        .begin(&mut session, PairingId::new([13; 16]), 100, 50, 1)
        .unwrap();
    pairing.snapshot_at(&mut session, 110, &output).unwrap();

    assert!(pairing.snapshot_at(&mut session, 109, &output).is_err());
    assert_eq!(session.phase(), SessionPhase::Idle);

    let snapshot = pairing.snapshot_at(&mut session, 150, &output).unwrap();
    assert_eq!(snapshot.session(), SessionPhase::Idle);
    assert_eq!(snapshot.pairing(), PairingSnapshotState::Expired);
    assert_eq!(snapshot.output(), output.snapshot());
}

#[test]
fn consumed_pairing_record_remains_visible_while_the_trusted_stream_runs() {
    let mut pairing = PairingInvitationController::<1>::new();
    let mut session = SessionState::IDLE;
    let mut output = controller();
    pairing
        .begin(&mut session, PairingId::new([17; 16]), 100, 50, 1)
        .unwrap();
    pairing
        .record_proof(&mut session, 101, PairingSource::new([19; 16]), true)
        .unwrap();
    output.start(selection()).unwrap();
    session.apply(SessionEvent::StartStream).unwrap();

    let streaming = pairing.snapshot_at(&mut session, 102, &output).unwrap();
    assert_eq!(streaming.session(), SessionPhase::Streaming);
    assert_eq!(streaming.pairing(), PairingSnapshotState::Consumed);
    assert_eq!(streaming.output(), output.snapshot());

    session.apply(SessionEvent::ConnectionLost).unwrap();
    output.output_mut().health = OutputHealth::StreamError;
    assert_eq!(output.monitor(), Some(OutputHealth::StreamError));
    let reconnecting = pairing.snapshot_at(&mut session, 103, &output).unwrap();
    assert_eq!(reconnecting.session(), SessionPhase::Reconnecting);
    assert_eq!(reconnecting.pairing(), PairingSnapshotState::Consumed);
    assert_eq!(reconnecting.output(), output.snapshot());
}

#[test]
fn revoked_pairing_projects_a_redacted_terminal_state() {
    let mut pairing = PairingInvitationController::<1>::new();
    let mut session = SessionState::IDLE;
    let output = controller();
    pairing
        .begin(&mut session, PairingId::new([23; 16]), 100, 50, 1)
        .unwrap();
    assert_eq!(
        pairing.record_proof(&mut session, 101, PairingSource::new([29; 16]), true),
        Ok(PairingAttemptOutcome::Paired)
    );
    pairing.revoke(&mut session).unwrap();

    let snapshot = pairing.snapshot_at(&mut session, 102, &output).unwrap();
    assert_eq!(snapshot.session(), SessionPhase::Idle);
    assert_eq!(snapshot.pairing(), PairingSnapshotState::Revoked);
}
