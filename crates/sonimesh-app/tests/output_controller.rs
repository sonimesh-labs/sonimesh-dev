//! Control-domain output recovery tests using a deterministic platform double.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use sonimesh_app::{
    OutputController, OutputLifecycleError, OutputSelection, SessionEvent, SessionState,
};
use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceInfo, OutputError, OutputHealth, RenderSource,
    RenderSourceFactory, SampleFormat,
};

#[derive(Default)]
struct Silence;

impl RenderSource for Silence {
    fn render(&mut self, output: &mut [f32]) {
        output.fill(0.0);
    }
}

struct TestFactory {
    created: Arc<AtomicUsize>,
}

impl RenderSourceFactory for TestFactory {
    fn create(&mut self) -> Box<dyn RenderSource> {
        self.created.fetch_add(1, Ordering::Relaxed);
        Box::new(Silence)
    }
}

struct TestOutput {
    health: OutputHealth,
    start_calls: usize,
    stop_calls: usize,
    fail_start: bool,
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
        self.start_calls += 1;
        if self.fail_start {
            return Err(OutputError::Backend("test start failure".to_owned()));
        }
        Ok(())
    }

    fn stop(&mut self) {
        self.stop_calls += 1;
    }

    fn health(&self) -> OutputHealth {
        self.health
    }
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

fn controller(created: Arc<AtomicUsize>) -> OutputController<TestOutput, TestFactory> {
    OutputController::new(
        TestOutput {
            health: OutputHealth::Healthy,
            start_calls: 0,
            stop_calls: 0,
            fail_start: false,
        },
        TestFactory { created },
    )
}

#[test]
fn faults_stop_and_recover_without_leaving_streaming_state() {
    let created = Arc::new(AtomicUsize::new(0));
    let mut controller = controller(Arc::clone(&created));
    let mut session = SessionState::Ready;
    controller.start(&mut session, selection()).unwrap();
    assert_eq!(session, SessionState::Streaming);

    controller.output_mut().health = OutputHealth::StreamError;
    assert_eq!(
        controller.monitor(&mut session).unwrap(),
        Some(OutputHealth::StreamError)
    );
    assert_eq!(session, SessionState::OutputUnavailable);
    assert_eq!(controller.output().stop_calls, 1);

    controller.output_mut().health = OutputHealth::Healthy;
    controller.recover(&mut session).unwrap();
    assert_eq!(session, SessionState::Streaming);
    assert_eq!(controller.output().start_calls, 2);
    assert_eq!(created.load(Ordering::Relaxed), 2);
}

#[test]
fn failed_recovery_preserves_retryable_output_state() {
    let created = Arc::new(AtomicUsize::new(0));
    let mut controller = controller(created);
    let mut session = SessionState::Ready;
    controller.start(&mut session, selection()).unwrap();
    controller.output_mut().health = OutputHealth::CallbackCapacityExceeded;
    controller.monitor(&mut session).unwrap();
    controller.output_mut().fail_start = true;

    assert!(matches!(
        controller.recover(&mut session),
        Err(OutputLifecycleError::Output(OutputError::Backend(_)))
    ));
    assert_eq!(session, SessionState::OutputUnavailable);
}

#[test]
fn start_requires_trusted_ready_state_before_backend_call() {
    let created = Arc::new(AtomicUsize::new(0));
    let mut controller = controller(created);
    let mut session = SessionState::Idle;

    assert!(matches!(
        controller.start(&mut session, selection()),
        Err(OutputLifecycleError::Transition(error)) if error.event == SessionEvent::StartStream
    ));
    assert_eq!(controller.output().start_calls, 0);
}
