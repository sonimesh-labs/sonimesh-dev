//! Deterministic property coverage for the control-domain output recovery seam.
//!
//! [`OutputController`] owns the start/monitor/recover/stop state machine that
//! keeps the desktop lifecycle honest about output health: a stream starts only
//! after the platform accepts its source, a fault is reported at most once and
//! retires the active stream, recovery reuses the last successful selection,
//! and every platform call is mirrored by the source-factory bookkeeping. This
//! test walks a real controller plus a deterministic platform double against an
//! independent state model and compares the result, the redacted snapshot, and
//! every platform/factory call counter after each step.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use sonimesh_app::{OutputController, OutputLifecycleError, OutputSelection};
use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceInfo, OutputError, OutputHealth, RenderSource,
    RenderSourceFactory, RenderSourceFactoryError, SampleFormat,
};

const FAULT_CREATE_TEXT: &str = "test source construction failure";
const FAULT_START_TEXT: &str = "test start failure";

/// Small deterministic xorshift64 generator; no external dependency.
struct XorShift64(u64);

impl XorShift64 {
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 32) as u32
    }
}

#[derive(Default)]
struct Silence;

impl RenderSource for Silence {
    fn render(&mut self, output: &mut [f32]) {
        output.fill(0.0);
    }
}

struct TestFactory {
    created: Arc<AtomicUsize>,
    fail_create: Arc<AtomicBool>,
    starts: Arc<AtomicUsize>,
}

impl RenderSourceFactory for TestFactory {
    fn create(
        &mut self,
        _config: &OutputConfig,
    ) -> Result<Box<dyn RenderSource>, RenderSourceFactoryError> {
        self.created.fetch_add(1, Ordering::Relaxed);
        if self.fail_create.load(Ordering::Relaxed) {
            return Err(RenderSourceFactoryError::Build(
                FAULT_CREATE_TEXT.to_owned(),
            ));
        }
        Ok(Box::new(Silence))
    }

    fn output_started(&mut self, _config: &OutputConfig) {
        self.starts.fetch_add(1, Ordering::Relaxed);
    }

    fn output_stopped(&mut self) {
        self.starts.fetch_sub(1, Ordering::Relaxed);
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
            return Err(OutputError::Backend(FAULT_START_TEXT.to_owned()));
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

fn selection(valid: bool) -> OutputSelection {
    OutputSelection {
        device: None,
        config: OutputConfig {
            sample_rate_hz: if valid { 48_000 } else { 0 },
            channels: 2,
            sample_format: SampleFormat::Float32,
            requested_callback_frames: Some(480),
            maximum_callback_frames: 1_024,
        },
    }
}

/// Independent spec model of the output recovery state machine. It owns the
/// platform double's health and failure flags plus every observable counter,
/// and derives each rule from the documented contract: start and recover admit
/// only when no stream is active, validate the config before touching the
/// platform, charge the source factory and output backend in order, and clear
/// the fault marker on success; monitor reports a fault at most once; stop is
/// unconditional.
#[allow(clippy::struct_excessive_bools)]
struct OutputModel {
    selection_retained: bool,
    stream_active: bool,
    fault_reported: bool,
    health: OutputHealth,
    fail_start: bool,
    fail_create: bool,
    start_calls: usize,
    stop_calls: usize,
    created: usize,
    starts: usize,
}

impl OutputModel {
    fn new() -> Self {
        Self {
            selection_retained: false,
            stream_active: false,
            fault_reported: false,
            health: OutputHealth::Healthy,
            fail_start: false,
            fail_create: false,
            start_calls: 0,
            stop_calls: 0,
            created: 0,
            starts: 0,
        }
    }

    fn start(&mut self, chosen: &OutputSelection) -> Result<(), OutputLifecycleError> {
        if self.stream_active {
            return Err(OutputLifecycleError::StreamAlreadyActive);
        }
        chosen
            .config
            .validate()
            .map_err(RenderSourceFactoryError::InvalidConfig)
            .map_err(OutputLifecycleError::SourceFactory)?;
        self.created += 1;
        if self.fail_create {
            return Err(OutputLifecycleError::SourceFactory(
                RenderSourceFactoryError::Build(FAULT_CREATE_TEXT.to_owned()),
            ));
        }
        self.start_calls += 1;
        if self.fail_start {
            return Err(OutputLifecycleError::Output(OutputError::Backend(
                FAULT_START_TEXT.to_owned(),
            )));
        }
        self.starts += 1;
        self.selection_retained = true;
        self.stream_active = true;
        self.fault_reported = false;
        Ok(())
    }

    fn monitor(&mut self) -> Option<OutputHealth> {
        if !self.stream_active || self.fault_reported {
            return None;
        }
        if self.health == OutputHealth::Healthy {
            return None;
        }
        self.stop_calls += 1;
        self.starts = self
            .starts
            .checked_sub(1)
            .expect("active stream implies a matching output_started call");
        self.stream_active = false;
        self.fault_reported = true;
        Some(self.health)
    }

    fn recover(&mut self) -> Result<(), OutputLifecycleError> {
        if self.stream_active {
            return Err(OutputLifecycleError::StreamAlreadyActive);
        }
        if !self.selection_retained {
            return Err(OutputLifecycleError::MissingSelection);
        }
        // The retained selection was validated by the successful start that
        // stored it, so no invalid-config path exists for recovery.
        self.created += 1;
        if self.fail_create {
            return Err(OutputLifecycleError::SourceFactory(
                RenderSourceFactoryError::Build(FAULT_CREATE_TEXT.to_owned()),
            ));
        }
        self.start_calls += 1;
        if self.fail_start {
            return Err(OutputLifecycleError::Output(OutputError::Backend(
                FAULT_START_TEXT.to_owned(),
            )));
        }
        self.starts += 1;
        self.stream_active = true;
        self.fault_reported = false;
        Ok(())
    }

    fn stop(&mut self) {
        self.stop_calls += 1;
        if self.stream_active {
            self.starts = self
                .starts
                .checked_sub(1)
                .expect("active stream implies a matching output_started call");
        }
        self.stream_active = false;
        self.fault_reported = true;
    }
}

/// Branch counters accumulated across the seeded walk.
#[derive(Default)]
struct WalkCounters {
    start_ok: usize,
    start_active: usize,
    start_invalid_config: usize,
    start_create_failed: usize,
    start_backend_failed: usize,
    monitor_active_healthy: usize,
    monitor_not_active: usize,
    monitor_capacity: usize,
    monitor_stream_error: usize,
    recover_ok: usize,
    recover_active: usize,
    recover_missing_selection: usize,
    recover_create_failed: usize,
    recover_backend_failed: usize,
    stop_count: usize,
    set_health: usize,
    set_fail_start: usize,
    set_fail_create: usize,
}

struct Walk<'a> {
    controller: OutputController<TestOutput, TestFactory>,
    model: OutputModel,
    rng: XorShift64,
    counters: &'a mut WalkCounters,
    created: Arc<AtomicUsize>,
    fail_create: Arc<AtomicBool>,
    starts: Arc<AtomicUsize>,
}

impl Walk<'_> {
    fn assert_state(&self, context: &str) {
        let snapshot = self.controller.snapshot();
        assert_eq!(
            snapshot.selection_retained(),
            self.model.selection_retained,
            "selection snapshot after {context}"
        );
        assert_eq!(
            snapshot.stream_active(),
            self.model.stream_active,
            "active snapshot after {context}"
        );
        assert_eq!(
            snapshot.fault_reported(),
            self.model.fault_reported,
            "fault snapshot after {context}"
        );
        assert_eq!(
            snapshot.health(),
            self.model.health,
            "health snapshot after {context}"
        );
        assert_eq!(
            self.controller.output().start_calls,
            self.model.start_calls,
            "platform start calls after {context}"
        );
        assert_eq!(
            self.controller.output().stop_calls,
            self.model.stop_calls,
            "platform stop calls after {context}"
        );
        assert_eq!(
            self.created.load(Ordering::Relaxed),
            self.model.created,
            "factory create calls after {context}"
        );
        assert_eq!(
            self.starts.load(Ordering::Relaxed),
            self.model.starts,
            "factory net starts after {context}"
        );
        assert!(
            !self.model.fault_reported || !self.model.stream_active,
            "a reported fault must retire the stream after {context}"
        );
        assert!(
            !self.model.stream_active || self.model.selection_retained,
            "an active stream must retain a selection after {context}"
        );
        assert_eq!(
            self.model.starts,
            usize::from(self.model.stream_active),
            "net factory starts must equal the active flag after {context}"
        );
    }

    fn step_start(&mut self, force_invalid: bool) {
        let valid = !force_invalid && self.rng.next_u32() % 100 < 70;
        let chosen = selection(valid);
        let real = self.controller.start(chosen.clone());
        let expected = self.model.start(&chosen);
        assert_eq!(&real, &expected, "start with valid={valid}");
        self.assert_state("start");
        match real {
            Ok(()) => self.counters.start_ok += 1,
            Err(OutputLifecycleError::StreamAlreadyActive) => self.counters.start_active += 1,
            Err(OutputLifecycleError::SourceFactory(RenderSourceFactoryError::InvalidConfig(
                _,
            ))) => self.counters.start_invalid_config += 1,
            Err(OutputLifecycleError::SourceFactory(RenderSourceFactoryError::Build(_))) => {
                self.counters.start_create_failed += 1;
            }
            Err(OutputLifecycleError::Output(_)) => self.counters.start_backend_failed += 1,
            Err(OutputLifecycleError::MissingSelection) => {
                unreachable!("start always receives a selection");
            }
        }
    }

    fn step_monitor(&mut self) {
        let real = self.controller.monitor();
        let expected = self.model.monitor();
        assert_eq!(real, expected, "monitor");
        self.assert_state("monitor");
        match real {
            Some(OutputHealth::CallbackCapacityExceeded) => self.counters.monitor_capacity += 1,
            Some(OutputHealth::StreamError) => self.counters.monitor_stream_error += 1,
            Some(OutputHealth::Healthy) => unreachable!("monitor never reports healthy"),
            None if self.model.stream_active => self.counters.monitor_active_healthy += 1,
            None => self.counters.monitor_not_active += 1,
        }
    }

    fn step_recover(&mut self) {
        let real = self.controller.recover();
        let expected = self.model.recover();
        assert_eq!(&real, &expected, "recover");
        self.assert_state("recover");
        match real {
            Ok(()) => self.counters.recover_ok += 1,
            Err(OutputLifecycleError::StreamAlreadyActive) => self.counters.recover_active += 1,
            Err(OutputLifecycleError::MissingSelection) => {
                self.counters.recover_missing_selection += 1;
            }
            Err(OutputLifecycleError::SourceFactory(RenderSourceFactoryError::Build(_))) => {
                self.counters.recover_create_failed += 1;
            }
            Err(OutputLifecycleError::Output(_)) => self.counters.recover_backend_failed += 1,
            Err(OutputLifecycleError::SourceFactory(RenderSourceFactoryError::InvalidConfig(
                _,
            ))) => unreachable!("recovery selection was validated when stored"),
        }
    }

    fn step_stop(&mut self) {
        self.controller.stop();
        self.model.stop();
        self.assert_state("stop");
        self.counters.stop_count += 1;
    }

    fn step_set_health(&mut self) {
        let health = match self.rng.next_u32() % 3 {
            0 => OutputHealth::Healthy,
            1 => OutputHealth::CallbackCapacityExceeded,
            _ => OutputHealth::StreamError,
        };
        self.controller.output_mut().health = health;
        self.model.health = health;
        self.counters.set_health += 1;
    }

    fn step_set_fail_start(&mut self) {
        let value = self.rng.next_u32() % 2 == 0;
        self.controller.output_mut().fail_start = value;
        self.model.fail_start = value;
        self.counters.set_fail_start += 1;
    }

    fn step_set_fail_create(&mut self) {
        let value = self.rng.next_u32() % 2 == 0;
        self.fail_create.store(value, Ordering::Relaxed);
        self.model.fail_create = value;
        self.counters.set_fail_create += 1;
    }
}

fn run_walk(seed: u64, iterations: usize) -> WalkCounters {
    let created = Arc::new(AtomicUsize::new(0));
    let fail_create = Arc::new(AtomicBool::new(false));
    let starts = Arc::new(AtomicUsize::new(0));
    let mut counters = WalkCounters::default();
    let mut walk = Walk {
        controller: OutputController::new(
            TestOutput {
                health: OutputHealth::Healthy,
                start_calls: 0,
                stop_calls: 0,
                fail_start: false,
            },
            TestFactory {
                created: Arc::clone(&created),
                fail_create: Arc::clone(&fail_create),
                starts: Arc::clone(&starts),
            },
        ),
        model: OutputModel::new(),
        rng: XorShift64(seed),
        counters: &mut counters,
        created,
        fail_create,
        starts,
    };

    // Scripted prologue on the fresh stopped controller: recovery has no
    // selection, monitoring reports no fault, stop is unconditional, and an
    // invalid config is rejected before the platform is touched.
    walk.step_recover();
    walk.step_monitor();
    walk.step_stop();
    walk.step_start(true);

    for _ in 0..iterations {
        let operation = walk.rng.next_u32() % 100;
        if operation < 8 {
            walk.step_set_health();
        } else if operation < 12 {
            walk.step_set_fail_start();
        } else if operation < 16 {
            walk.step_set_fail_create();
        } else if operation < 42 {
            walk.step_start(false);
        } else if operation < 60 {
            walk.step_monitor();
        } else if operation < 80 {
            walk.step_recover();
        } else {
            walk.step_stop();
        }
    }
    counters
}

#[allow(clippy::similar_names)]
#[test]
fn seeded_output_recovery_walk_matches_a_spec_model() {
    let counters = run_walk(0xC0FF_EE00_2026_0021, 250_000);

    assert!(counters.start_ok > 0, "start_ok was never exercised");
    assert!(
        counters.start_active > 0,
        "start_active was never exercised"
    );
    assert!(
        counters.start_invalid_config > 0,
        "start_invalid_config was never exercised"
    );
    assert!(
        counters.start_create_failed > 0,
        "start_create_failed was never exercised"
    );
    assert!(
        counters.start_backend_failed > 0,
        "start_backend_failed was never exercised"
    );
    assert!(
        counters.monitor_active_healthy > 0,
        "monitor_active_healthy was never exercised"
    );
    assert!(
        counters.monitor_not_active > 0,
        "monitor_not_active was never exercised"
    );
    assert!(
        counters.monitor_capacity > 0,
        "monitor_capacity was never exercised"
    );
    assert!(
        counters.monitor_stream_error > 0,
        "monitor_stream_error was never exercised"
    );
    assert!(counters.recover_ok > 0, "recover_ok was never exercised");
    assert!(
        counters.recover_active > 0,
        "recover_active was never exercised"
    );
    assert!(
        counters.recover_missing_selection > 0,
        "recover_missing_selection was never exercised"
    );
    assert!(
        counters.recover_create_failed > 0,
        "recover_create_failed was never exercised"
    );
    assert!(
        counters.recover_backend_failed > 0,
        "recover_backend_failed was never exercised"
    );
    assert!(counters.stop_count > 0, "stop was never exercised");
}
