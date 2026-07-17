//! Control-domain output recovery tests using a deterministic platform double.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use sonimesh_app::{OutputController, OutputLifecycleError, OutputSelection};
use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceInfo, OutputError, OutputHealth, RenderSource,
    RenderSourceFactory, RenderSourceFactoryError, SampleFormat,
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
    last_output_channels: Arc<AtomicUsize>,
    fail_create: Arc<AtomicBool>,
    starts: Arc<AtomicUsize>,
}

impl RenderSourceFactory for TestFactory {
    fn create(
        &mut self,
        config: &OutputConfig,
    ) -> Result<Box<dyn RenderSource>, RenderSourceFactoryError> {
        self.created.fetch_add(1, Ordering::Relaxed);
        if self.fail_create.load(Ordering::Relaxed) {
            return Err(RenderSourceFactoryError::Build(
                "test source construction failure".to_owned(),
            ));
        }
        self.last_output_channels
            .store(usize::from(config.channels), Ordering::Relaxed);
        Ok(Box::new(Silence))
    }

    fn output_started(&mut self, _config: &OutputConfig) {
        self.starts.fetch_add(1, Ordering::Relaxed);
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

fn controller(
    created: Arc<AtomicUsize>,
    last_output_channels: Arc<AtomicUsize>,
    fail_create: Arc<AtomicBool>,
    starts: Arc<AtomicUsize>,
) -> OutputController<TestOutput, TestFactory> {
    OutputController::new(
        TestOutput {
            health: OutputHealth::Healthy,
            start_calls: 0,
            stop_calls: 0,
            fail_start: false,
        },
        TestFactory {
            created,
            last_output_channels,
            fail_create,
            starts,
        },
    )
}

#[test]
fn faults_stop_and_recover_without_leaving_streaming_state() {
    let created = Arc::new(AtomicUsize::new(0));
    let last_output_channels = Arc::new(AtomicUsize::new(0));
    let starts = Arc::new(AtomicUsize::new(0));
    let mut controller = controller(
        Arc::clone(&created),
        Arc::clone(&last_output_channels),
        Arc::new(AtomicBool::new(false)),
        Arc::clone(&starts),
    );
    controller.start(selection()).unwrap();
    assert_eq!(last_output_channels.load(Ordering::Relaxed), 2);
    assert_eq!(starts.load(Ordering::Relaxed), 1);

    controller.output_mut().health = OutputHealth::StreamError;
    assert_eq!(controller.monitor(), Some(OutputHealth::StreamError));
    assert_eq!(controller.output().stop_calls, 1);
    assert_eq!(controller.monitor(), None);
    assert_eq!(controller.output().stop_calls, 1);

    controller.output_mut().health = OutputHealth::Healthy;
    controller.recover().unwrap();
    assert_eq!(controller.output().start_calls, 2);
    assert_eq!(created.load(Ordering::Relaxed), 2);
    assert_eq!(starts.load(Ordering::Relaxed), 2);

    controller.output_mut().health = OutputHealth::StreamError;
    assert_eq!(controller.monitor(), Some(OutputHealth::StreamError));
    assert_eq!(controller.output().stop_calls, 2);
}

#[test]
fn failed_recovery_preserves_retryable_output_state() {
    let created = Arc::new(AtomicUsize::new(0));
    let last_output_channels = Arc::new(AtomicUsize::new(0));
    let starts = Arc::new(AtomicUsize::new(0));
    let mut controller = controller(
        created,
        last_output_channels,
        Arc::new(AtomicBool::new(false)),
        Arc::clone(&starts),
    );
    controller.start(selection()).unwrap();
    assert_eq!(starts.load(Ordering::Relaxed), 1);
    controller.output_mut().health = OutputHealth::CallbackCapacityExceeded;
    controller.monitor();
    controller.output_mut().fail_start = true;

    assert!(matches!(
        controller.recover(),
        Err(OutputLifecycleError::Output(OutputError::Backend(_)))
    ));
    assert_eq!(starts.load(Ordering::Relaxed), 1);
}

#[test]
fn starts_without_mutating_external_session_state() {
    let created = Arc::new(AtomicUsize::new(0));
    let last_output_channels = Arc::new(AtomicUsize::new(0));
    let mut controller = controller(
        created,
        last_output_channels,
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicUsize::new(0)),
    );
    controller.start(selection()).unwrap();
    assert_eq!(controller.output().start_calls, 1);
}

#[test]
fn explicit_stop_disarms_stale_health_until_a_new_start() {
    let created = Arc::new(AtomicUsize::new(0));
    let last_output_channels = Arc::new(AtomicUsize::new(0));
    let mut controller = controller(
        created,
        last_output_channels,
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicUsize::new(0)),
    );
    controller.start(selection()).unwrap();
    controller.output_mut().health = OutputHealth::StreamError;

    controller.stop();

    assert_eq!(controller.monitor(), None);
    assert_eq!(controller.output().stop_calls, 1);

    controller.output_mut().health = OutputHealth::Healthy;
    controller.start(selection()).unwrap();
    controller.output_mut().health = OutputHealth::StreamError;

    assert_eq!(controller.monitor(), Some(OutputHealth::StreamError));
    assert_eq!(controller.output().stop_calls, 2);
}

#[test]
fn source_creation_failure_does_not_start_the_backend_and_recovery_can_retry() {
    let created = Arc::new(AtomicUsize::new(0));
    let last_output_channels = Arc::new(AtomicUsize::new(0));
    let fail_create = Arc::new(AtomicBool::new(false));
    let mut controller = controller(
        Arc::clone(&created),
        last_output_channels,
        Arc::clone(&fail_create),
        Arc::new(AtomicUsize::new(0)),
    );
    controller.start(selection()).unwrap();
    controller.output_mut().health = OutputHealth::StreamError;
    controller.monitor();
    fail_create.store(true, Ordering::Relaxed);

    assert!(matches!(
        controller.recover(),
        Err(OutputLifecycleError::SourceFactory(
            RenderSourceFactoryError::Build(_)
        ))
    ));
    assert_eq!(controller.output().start_calls, 1);

    fail_create.store(false, Ordering::Relaxed);
    controller.recover().unwrap();
    assert_eq!(controller.output().start_calls, 2);
    assert_eq!(created.load(Ordering::Relaxed), 3);
}
