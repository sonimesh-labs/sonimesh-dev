//! Deterministic property coverage for the receiver output recovery seam.
//!
//! [`ReceiverOutputController`] recreates a receiver's paired PCM endpoint: the
//! platform callback source and the decoder-side producer are built as one
//! endpoint, the producer is installed into the receiver pipeline only after
//! the platform accepts its matching source, and the published output format
//! survives only while the accepted callback source is active. This test walks
//! a real controller against an independent state model with a counting
//! pipeline installer and shared stream diagnostics, and compares the result,
//! the platform call counters, the pipeline replacement count, and the
//! published nominal sample-rate pair after every step.

use std::sync::Arc;

use sonimesh_app::{
    OutputSelection, ReceiverOutputController, ReceiverOutputLifecycleError,
    ReceiverPcmEndpointError, ReceiverPcmEndpointFactory, ReceiverPcmProducerInstaller,
    SourcePcmFormat,
};
use sonimesh_audio::RealtimePcmProducer;
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceInfo, OutputError, OutputHealth, RenderSource,
    RenderSourceFactoryError, SampleFormat,
};

const SAMPLES_PER_BLOCK: usize = 4;
const SOURCE_SAMPLE_RATE_HZ: u32 = 48_000;
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

/// Pipeline double that counts producer replacements without taking ownership
/// of ingress or socket state.
struct CountingPipeline {
    replacements: usize,
}

impl ReceiverPcmProducerInstaller<SAMPLES_PER_BLOCK> for CountingPipeline {
    fn replace_pcm_producer(&mut self, _producer: RealtimePcmProducer<SAMPLES_PER_BLOCK>) {
        self.replacements += 1;
    }
}

fn selection(valid: bool) -> OutputSelection {
    OutputSelection {
        device: None,
        config: OutputConfig {
            sample_rate_hz: if valid { SOURCE_SAMPLE_RATE_HZ } else { 0 },
            channels: 2,
            sample_format: SampleFormat::Float32,
            requested_callback_frames: Some(480),
            maximum_callback_frames: 1_024,
        },
    }
}

/// Comparable model-side error. `EndpointOther` is unreachable in this walk:
/// with a fixed valid queue capacity and an equal-rate source, the only
/// endpoint failure the walk can produce is an invalid output config.
#[derive(Clone, Debug, Eq, PartialEq)]
enum ModelError {
    MissingSelection,
    StreamAlreadyActive,
    EndpointInvalidConfig,
    EndpointOther,
    Output(OutputError),
}

impl ModelError {
    fn from_real(error: ReceiverOutputLifecycleError) -> Self {
        match error {
            ReceiverOutputLifecycleError::MissingSelection => Self::MissingSelection,
            ReceiverOutputLifecycleError::StreamAlreadyActive => Self::StreamAlreadyActive,
            ReceiverOutputLifecycleError::Endpoint(ReceiverPcmEndpointError::Source(
                RenderSourceFactoryError::InvalidConfig(_),
            )) => Self::EndpointInvalidConfig,
            ReceiverOutputLifecycleError::Endpoint(_) => Self::EndpointOther,
            ReceiverOutputLifecycleError::Output(output) => Self::Output(output),
        }
    }
}

/// Independent spec model of the receiver output recovery state machine. It
/// derives each rule from the documented contract: the paired endpoint is
/// created before the platform is touched, the producer is installed and the
/// output format published only after the platform accepts the source, a fault
/// is reported at most once and clears the published format, and recovery
/// reuses the last successful selection.
#[allow(clippy::struct_excessive_bools)]
struct ReceiverOutputModel {
    selection_retained: bool,
    stream_active: bool,
    fault_reported: bool,
    health: OutputHealth,
    fail_start: bool,
    start_calls: usize,
    stop_calls: usize,
    replacements: usize,
    published: bool,
}

impl ReceiverOutputModel {
    fn new() -> Self {
        Self {
            selection_retained: false,
            stream_active: false,
            fault_reported: false,
            health: OutputHealth::Healthy,
            fail_start: false,
            start_calls: 0,
            stop_calls: 0,
            replacements: 0,
            published: false,
        }
    }

    fn install(&mut self, chosen: &OutputSelection) -> Result<(), ModelError> {
        if chosen.config.validate().is_err() {
            return Err(ModelError::EndpointInvalidConfig);
        }
        self.start_calls += 1;
        if self.fail_start {
            return Err(ModelError::Output(OutputError::Backend(
                FAULT_START_TEXT.to_owned(),
            )));
        }
        self.published = true;
        self.replacements += 1;
        Ok(())
    }

    fn start(&mut self, chosen: &OutputSelection) -> Result<(), ModelError> {
        if self.stream_active {
            return Err(ModelError::StreamAlreadyActive);
        }
        self.install(chosen)?;
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
        self.published = false;
        self.stream_active = false;
        self.fault_reported = true;
        Some(self.health)
    }

    fn recover(&mut self) -> Result<(), ModelError> {
        if self.stream_active {
            return Err(ModelError::StreamAlreadyActive);
        }
        if !self.selection_retained {
            return Err(ModelError::MissingSelection);
        }
        // The retained selection was validated by the successful start that
        // stored it, so recovery cannot hit an invalid-config endpoint error.
        self.install(&selection(true))?;
        self.stream_active = true;
        self.fault_reported = false;
        Ok(())
    }

    fn stop(&mut self) {
        self.stop_calls += 1;
        self.published = false;
        self.stream_active = false;
        self.fault_reported = false;
    }
}

/// Branch counters accumulated across the seeded walk.
#[derive(Default)]
struct WalkCounters {
    start_ok: usize,
    start_active: usize,
    start_endpoint_invalid: usize,
    start_backend_failed: usize,
    monitor_active_healthy: usize,
    monitor_not_active: usize,
    monitor_capacity: usize,
    monitor_stream_error: usize,
    recover_ok: usize,
    recover_active: usize,
    recover_missing_selection: usize,
    recover_backend_failed: usize,
    stop_count: usize,
    endpoint_other: usize,
}

#[allow(clippy::struct_field_names)]
struct Walk<'a> {
    controller: ReceiverOutputController<TestOutput, SAMPLES_PER_BLOCK>,
    pipeline: CountingPipeline,
    model: ReceiverOutputModel,
    counters: Arc<StreamCounters>,
    rng: XorShift64,
    walk_counters: &'a mut WalkCounters,
}

impl Walk<'_> {
    fn assert_state(&self, context: &str) {
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
            self.pipeline.replacements, self.model.replacements,
            "pipeline producer replacements after {context}"
        );
        let published = self.counters.snapshot();
        assert_eq!(
            published.nominal_input_sample_rate_hz,
            if self.model.published {
                SOURCE_SAMPLE_RATE_HZ
            } else {
                0
            },
            "published input rate after {context}"
        );
        assert_eq!(
            published.nominal_output_sample_rate_hz,
            if self.model.published {
                SOURCE_SAMPLE_RATE_HZ
            } else {
                0
            },
            "published output rate after {context}"
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
            self.model.published, self.model.stream_active,
            "a published format must match the active flag after {context}"
        );
    }

    fn step_start(&mut self, force_invalid: bool) {
        let valid = !force_invalid && self.rng.next_u32() % 100 < 75;
        let chosen = selection(valid);
        let real = self.controller.start(chosen.clone(), &mut self.pipeline);
        let expected = self.model.start(&chosen);
        let mapped = real.map_err(ModelError::from_real);
        assert_eq!(mapped, expected, "start with valid={valid}");
        self.assert_state("start");
        match mapped {
            Ok(()) => self.walk_counters.start_ok += 1,
            Err(ModelError::StreamAlreadyActive) => self.walk_counters.start_active += 1,
            Err(ModelError::EndpointInvalidConfig | ModelError::EndpointOther) => {
                self.walk_counters.start_endpoint_invalid += 1;
            }
            Err(ModelError::Output(_)) => self.walk_counters.start_backend_failed += 1,
            Err(ModelError::MissingSelection) => {
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
            Some(OutputHealth::CallbackCapacityExceeded) => {
                self.walk_counters.monitor_capacity += 1;
            }
            Some(OutputHealth::StreamError) => self.walk_counters.monitor_stream_error += 1,
            Some(OutputHealth::Healthy) => unreachable!("monitor never reports healthy"),
            None if self.model.stream_active => self.walk_counters.monitor_active_healthy += 1,
            None => self.walk_counters.monitor_not_active += 1,
        }
    }

    fn step_recover(&mut self) {
        let real = self.controller.recover(&mut self.pipeline);
        let expected = self.model.recover();
        let mapped = real.map_err(ModelError::from_real);
        assert_eq!(mapped, expected, "recover");
        self.assert_state("recover");
        match mapped {
            Ok(()) => self.walk_counters.recover_ok += 1,
            Err(ModelError::StreamAlreadyActive) => self.walk_counters.recover_active += 1,
            Err(ModelError::MissingSelection) => self.walk_counters.recover_missing_selection += 1,
            Err(ModelError::Output(_)) => self.walk_counters.recover_backend_failed += 1,
            Err(ModelError::EndpointInvalidConfig | ModelError::EndpointOther) => {
                unreachable!("recovery selection was validated when stored");
            }
        }
    }

    fn step_stop(&mut self) {
        self.controller.stop();
        self.model.stop();
        self.assert_state("stop");
        self.walk_counters.stop_count += 1;
    }

    fn step_set_health(&mut self) {
        let health = match self.rng.next_u32() % 3 {
            0 => OutputHealth::Healthy,
            1 => OutputHealth::CallbackCapacityExceeded,
            _ => OutputHealth::StreamError,
        };
        self.controller.output_mut().health = health;
        self.model.health = health;
    }

    fn step_set_fail_start(&mut self) {
        let value = self.rng.next_u32() % 2 == 0;
        self.controller.output_mut().fail_start = value;
        self.model.fail_start = value;
    }
}

fn run_walk(seed: u64, iterations: usize) -> WalkCounters {
    let counters = Arc::new(StreamCounters::default());
    let mut walk_counters = WalkCounters::default();
    let mut walk = Walk {
        controller: ReceiverOutputController::new(
            TestOutput {
                health: OutputHealth::Healthy,
                start_calls: 0,
                stop_calls: 0,
                fail_start: false,
            },
            ReceiverPcmEndpointFactory::new(
                4,
                SourcePcmFormat::new(SOURCE_SAMPLE_RATE_HZ, 1).expect("valid source format"),
                Arc::clone(&counters),
            ),
        ),
        pipeline: CountingPipeline { replacements: 0 },
        model: ReceiverOutputModel::new(),
        counters,
        rng: XorShift64(seed),
        walk_counters: &mut walk_counters,
    };

    // Scripted prologue on the fresh stopped controller: recovery has no
    // selection, monitoring reports no fault, stop is unconditional, and an
    // invalid output config fails the endpoint before the platform is touched.
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
        } else if operation < 40 {
            walk.step_start(false);
        } else if operation < 58 {
            walk.step_monitor();
        } else if operation < 82 {
            walk.step_recover();
        } else {
            walk.step_stop();
        }
    }
    walk_counters
}

#[allow(clippy::similar_names)]
#[test]
fn seeded_receiver_output_walk_matches_a_spec_model() {
    let counters = run_walk(0xC0FF_EE00_2026_0031, 250_000);

    assert!(counters.start_ok > 0, "start_ok was never exercised");
    assert!(
        counters.start_active > 0,
        "start_active was never exercised"
    );
    assert!(
        counters.start_endpoint_invalid > 0,
        "start_endpoint_invalid was never exercised"
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
        counters.recover_backend_failed > 0,
        "recover_backend_failed was never exercised"
    );
    assert!(counters.stop_count > 0, "stop was never exercised");
    assert_eq!(
        counters.endpoint_other, 0,
        "walk produced an unexpected endpoint error"
    );
}
