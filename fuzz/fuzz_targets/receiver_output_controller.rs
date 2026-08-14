#![no_main]

//! Fuzz the receiver output recovery lifecycle state machine.
//!
//! `ReceiverOutputController` recreates a receiver's paired PCM endpoint: the
//! platform callback source and the decoder-side producer are built as one
//! endpoint, the producer is installed into the receiver pipeline only after
//! the platform accepts its matching source, and the published output-format
//! diagnostics survive only while the accepted callback source is active.
//! This target drives arbitrary start, monitor, recover, stop, health, and
//! fail-start scripts and compares the real controller against an independent
//! state model after every step, proving the lifecycle cannot panic, cannot
//! publish format diagnostics or swap the producer while the stream is
//! retired, reports a fault at most once, and reuses the last successful
//! selection on recovery.
//!
//! The model mirrors the seeded property walk
//! (`crates/sonimesh-app/tests/receiver_output_controller_property.rs`) so the
//! deterministic and byte-driven paths assert the same contract.

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use sonimesh_app::{
    OutputSelection, ReceiverOutputController, ReceiverOutputLifecycleError,
    ReceiverPcmEndpointError, ReceiverPcmEndpointFactory, ReceiverPcmProducerInstaller,
    SourcePcmFormat,
};
use sonimesh_audio::RealtimePcmProducer;
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceId, OutputDeviceInfo, OutputError, OutputHealth,
    RenderSource, RenderSourceFactoryError, SampleFormat,
};

const SAMPLES_PER_BLOCK: usize = 4;
const SOURCE_SAMPLE_RATE_HZ: u32 = 48_000;
const FAULT_START_TEXT: &str = "fuzz start failure";

/// Reads one byte from `data`, wrapping around the slice.
fn next_byte(data: &[u8], offset: &mut usize) -> u8 {
    let byte = data[*offset % data.len()];
    *offset += 1;
    byte
}

/// Reads one little-endian `u64` from `data`, wrapping around the slice.
fn next_u64(data: &[u8], offset: &mut usize) -> u64 {
    let mut bytes = [0_u8; 8];
    for byte in &mut bytes {
        *byte = next_byte(data, offset);
    }
    u64::from_le_bytes(bytes)
}

/// Audio output stub that records backend health and platform call counts.
struct FuzzOutput {
    health: OutputHealth,
    start_calls: usize,
    stop_calls: usize,
    fail_start: bool,
}

impl AudioOutput for FuzzOutput {
    fn devices(&self) -> Result<Vec<OutputDeviceInfo>, OutputError> {
        Ok(Vec::new())
    }

    fn start(
        &mut self,
        _device: Option<&OutputDeviceId>,
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
/// output format published only after the platform accepts the source, a
/// fault is reported at most once and clears the published format, and
/// recovery reuses the last successful selection.
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
        self.fault_reported = true;
    }
}

/// Real controller plus its independent model, walked in lockstep.
struct ReceiverOutputHarness {
    controller: ReceiverOutputController<FuzzOutput, SAMPLES_PER_BLOCK>,
    pipeline: CountingPipeline,
    counters: Arc<StreamCounters>,
    model: ReceiverOutputModel,
}

impl ReceiverOutputHarness {
    fn new() -> Self {
        let counters = Arc::new(StreamCounters::default());
        Self {
            controller: ReceiverOutputController::new(
                FuzzOutput {
                    health: OutputHealth::Healthy,
                    start_calls: 0,
                    stop_calls: 0,
                    fail_start: false,
                },
                ReceiverPcmEndpointFactory::new(
                    4,
                    SourcePcmFormat::new(SOURCE_SAMPLE_RATE_HZ, 1)
                        .expect("fixed source format is valid"),
                    Arc::clone(&counters),
                ),
            ),
            pipeline: CountingPipeline { replacements: 0 },
            counters,
            model: ReceiverOutputModel::new(),
        }
    }

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
        let expected_rate = if self.model.published {
            SOURCE_SAMPLE_RATE_HZ
        } else {
            0
        };
        assert_eq!(
            published.nominal_input_sample_rate_hz, expected_rate,
            "published input rate after {context}"
        );
        assert_eq!(
            published.nominal_output_sample_rate_hz, expected_rate,
            "published output rate after {context}"
        );
        let snapshot = self.controller.snapshot();
        assert_eq!(
            snapshot.selection_retained(),
            self.model.selection_retained,
            "snapshot selection retention after {context}"
        );
        assert_eq!(
            snapshot.stream_active(),
            self.model.stream_active,
            "snapshot stream-active flag after {context}"
        );
        assert_eq!(
            snapshot.fault_reported(),
            self.model.fault_reported,
            "snapshot fault-reported flag after {context}"
        );
        assert_eq!(
            snapshot.health(),
            self.model.health,
            "snapshot health after {context}"
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

    fn start(&mut self, valid: bool) {
        let chosen = selection(valid);
        let real = self.controller.start(chosen.clone(), &mut self.pipeline);
        let expected = self.model.start(&chosen);
        let mapped = real.map_err(ModelError::from_real);
        assert_eq!(mapped, expected, "start with valid={valid}");
        self.assert_state("start");
    }

    fn monitor(&mut self) {
        let real = self.controller.monitor();
        let expected = self.model.monitor();
        assert_eq!(real, expected, "monitor");
        self.assert_state("monitor");
    }

    fn recover(&mut self) {
        let real = self.controller.recover(&mut self.pipeline);
        let expected = self.model.recover();
        let mapped = real.map_err(ModelError::from_real);
        assert_eq!(mapped, expected, "recover");
        self.assert_state("recover");
    }

    fn stop(&mut self) {
        self.controller.stop();
        self.model.stop();
        self.assert_state("stop");
    }

    fn set_health(&mut self, health: OutputHealth) {
        self.controller.output_mut().health = health;
        self.model.health = health;
    }

    fn set_fail_start(&mut self, value: bool) {
        self.controller.output_mut().fail_start = value;
        self.model.fail_start = value;
    }
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    let mut harness = ReceiverOutputHarness::new();

    // Deterministic edge coverage on every input: recovery with no saved
    // selection, monitoring a stopped and an active healthy stream, stop
    // unconditionally, an invalid config rejected before the backend starts,
    // a backend failure surfacing as an output error, a fault reported once,
    // and recovery reusing the last successful selection even after a
    // failing backend start.
    harness.recover();
    harness.monitor();
    harness.stop();
    harness.start(false);
    harness.start(true);
    harness.monitor();
    harness.set_health(OutputHealth::StreamError);
    harness.monitor();
    harness.monitor();
    harness.recover();
    harness.set_fail_start(true);
    harness.recover();
    harness.set_fail_start(false);
    harness.recover();
    harness.stop();

    // Fuzz-derived operations with adversarial health, fail-start, and
    // selection states.
    let mut offset = 0;
    let steps = 1 + data.len() % 64;
    for _ in 0..steps {
        match next_byte(data, &mut offset) % 100 {
            0..=7 => {
                let health = match next_u64(data, &mut offset) % 3 {
                    0 => OutputHealth::Healthy,
                    1 => OutputHealth::CallbackCapacityExceeded,
                    _ => OutputHealth::StreamError,
                };
                harness.set_health(health);
            }
            8..=11 => harness.set_fail_start(next_u64(data, &mut offset).is_multiple_of(2)),
            12..=39 => harness.start(next_u64(data, &mut offset) % 100 < 75),
            40..=57 => harness.monitor(),
            58..=81 => harness.recover(),
            _ => harness.stop(),
        }
    }
});
