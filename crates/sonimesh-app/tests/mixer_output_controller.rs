//! Mixer output-lifetime controller tests: live membership handoff, recovery,
//! and fail-closed lifecycle behavior.

use sonimesh_app::{
    GainControlHandle, MixerMembershipController, MixerOutputController, MixerOutputLifecycleError,
    OutputSelection,
};
use sonimesh_audio::{RealtimePcmPushOutcome, realtime_pcm_queue};
use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceInfo, OutputError, OutputHealth, RenderSource,
    SampleFormat,
};

const SLOTS: usize = 4;
const BLOCK: usize = 8;

struct TestOutput {
    health: OutputHealth,
    start_calls: usize,
    stop_calls: usize,
    fail_start: bool,
    source: Option<Box<dyn RenderSource>>,
}

impl TestOutput {
    fn render(&mut self, output: &mut [f32]) {
        self.source
            .as_mut()
            .expect("test output never captured a source")
            .render(output);
    }
}

impl AudioOutput for TestOutput {
    fn devices(&self) -> Result<Vec<OutputDeviceInfo>, OutputError> {
        Ok(Vec::new())
    }

    fn start(
        &mut self,
        _device: Option<&sonimesh_platform::OutputDeviceId>,
        _config: OutputConfig,
        source: Box<dyn RenderSource>,
    ) -> Result<(), OutputError> {
        self.start_calls += 1;
        if self.fail_start {
            return Err(OutputError::Backend("test start failure".to_owned()));
        }
        self.source = Some(source);
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
            channels: 1,
            sample_format: SampleFormat::Float32,
            requested_callback_frames: Some(480),
            maximum_callback_frames: 480,
        },
    }
}

fn controller() -> MixerOutputController<TestOutput, SLOTS, BLOCK> {
    MixerOutputController::new(TestOutput {
        health: OutputHealth::Healthy,
        start_calls: 0,
        stop_calls: 0,
        fail_start: false,
        source: None,
    })
}

fn push_constant(
    producer: &mut sonimesh_audio::RealtimePcmProducer<BLOCK>,
    value: f32,
    blocks: usize,
) {
    for _ in 0..blocks {
        let block = [value; BLOCK];
        assert!(matches!(
            producer.push(&block),
            RealtimePcmPushOutcome::Inserted | RealtimePcmPushOutcome::ReplacedOldest
        ));
    }
}

#[test]
fn start_installs_a_live_mixer_membership_handle() {
    let mut output = controller();

    output.start(selection()).unwrap();

    let format = output.output_format().expect("started output format");
    assert_eq!(format.sample_rate_hz, 48_000);
    assert_eq!(format.channels, 1);
    let membership = output.membership().expect("started membership handle");
    let mut membership =
        MixerMembershipController::<SLOTS, BLOCK>::new(membership, 4, format).unwrap();

    let mut admitted = membership
        .admit_stream(format, GainControlHandle::new(), None)
        .unwrap();
    push_constant(&mut admitted.producer, 0.25, 2);

    let mut rendered = [0.0_f32; BLOCK];
    output.output_mut().render(&mut rendered);
    assert!(
        rendered.iter().any(|sample| *sample > 0.0),
        "a live membership handle must reach the platform-owned mixer"
    );
    assert!(output.snapshot().stream_active());
    assert!(output.snapshot().selection_retained());
}

#[test]
fn recovery_recreates_the_source_and_retires_the_old_membership() {
    let mut output = controller();
    output.start(selection()).unwrap();
    let stale_membership = output.membership().unwrap();
    let (_, stale_consumer) = realtime_pcm_queue::<BLOCK>(2).unwrap();
    stale_membership
        .insert(stale_consumer, GainControlHandle::new(), None)
        .unwrap();

    output.output_mut().health = OutputHealth::StreamError;
    assert_eq!(output.monitor(), Some(OutputHealth::StreamError));
    assert_eq!(output.monitor(), None);
    assert!(
        output.membership().is_none(),
        "fault must retire the handle"
    );

    output.recover().unwrap();
    assert_eq!(output.output().start_calls, 2);
    assert_eq!(output.output().stop_calls, 1);
    let format = output.output_format().expect("recovered output format");
    let fresh_membership = output.membership().expect("recovered membership handle");
    let mut fresh =
        MixerMembershipController::<SLOTS, BLOCK>::new(fresh_membership, 4, format).unwrap();

    // Commands published to the retired mixer must not reach the new source.
    let mut rendered = [0.0_f32; BLOCK];
    output.output_mut().render(&mut rendered);
    assert!(rendered.iter().all(|sample| *sample == 0.0));
    assert_eq!(fresh.pending_commands(), 0);

    let mut admitted = fresh
        .admit_stream(format, GainControlHandle::new(), None)
        .unwrap();
    push_constant(&mut admitted.producer, 0.5, 2);
    output.output_mut().render(&mut rendered);
    assert!(
        rendered.iter().any(|sample| *sample > 0.0),
        "a recovered source must accept fresh admissions"
    );
    assert!(!output.snapshot().fault_reported());
}

#[test]
fn failed_start_retains_no_membership_and_allows_a_later_start() {
    let mut output = controller();
    output.output_mut().fail_start = true;

    assert!(matches!(
        output.start(selection()),
        Err(MixerOutputLifecycleError::Output(OutputError::Backend(_)))
    ));
    assert!(output.membership().is_none());
    assert!(output.output_format().is_none());
    assert!(!output.snapshot().selection_retained());
    assert!(!output.snapshot().stream_active());

    output.output_mut().fail_start = false;
    output.start(selection()).unwrap();
    assert!(output.membership().is_some());
}

#[test]
fn invalid_selection_fails_closed_before_the_platform() {
    let mut output = controller();
    let mut invalid = selection();
    invalid.config.channels = 0;

    assert!(matches!(
        output.start(invalid),
        Err(MixerOutputLifecycleError::Source(_))
    ));
    assert!(output.output().start_calls == 0);
    assert!(output.membership().is_none());
}

#[test]
fn duplicate_start_and_recovery_preserve_the_active_stream() {
    let mut output = controller();
    output.start(selection()).unwrap();
    let handle = output.membership().unwrap();

    assert!(matches!(
        output.start(selection()),
        Err(MixerOutputLifecycleError::StreamAlreadyActive)
    ));
    assert!(matches!(
        output.recover(),
        Err(MixerOutputLifecycleError::StreamAlreadyActive)
    ));
    assert_eq!(output.output().start_calls, 1);
    assert!(output.membership().is_some());
    // The membership projection is unchanged: no command was published.
    let _ = handle;
    assert_eq!(
        MixerMembershipController::<SLOTS, BLOCK>::new(
            output.membership().unwrap(),
            4,
            output.output_format().unwrap()
        )
        .unwrap()
        .active(),
        0
    );
}

#[test]
fn stop_retires_membership_and_allows_recovery() {
    let mut output = controller();
    output.start(selection()).unwrap();

    output.stop();
    assert!(output.membership().is_none());
    assert_eq!(output.output().stop_calls, 1);

    output.output_mut().health = OutputHealth::Healthy;
    output.recover().unwrap();
    assert_eq!(output.output().start_calls, 2);
    assert!(output.membership().is_some());
}

#[test]
fn output_snapshot_tracks_idle_streaming_and_reported_faults() {
    let mut output = controller();

    let idle = output.snapshot();
    assert!(!idle.selection_retained());
    assert!(!idle.stream_active());
    assert!(!idle.fault_reported());
    assert_eq!(idle.health(), OutputHealth::Healthy);

    output.start(selection()).unwrap();
    let streaming = output.snapshot();
    assert!(streaming.selection_retained());
    assert!(streaming.stream_active());
    assert!(!streaming.fault_reported());

    output.output_mut().health = OutputHealth::StreamError;
    assert_eq!(output.monitor(), Some(OutputHealth::StreamError));
    let failed = output.snapshot();
    assert!(failed.selection_retained());
    assert!(!failed.stream_active());
    assert!(failed.fault_reported());
    assert_eq!(failed.health(), OutputHealth::StreamError);
}
