//! Mixer stream session gain and mute survival across output recovery.
//!
//! A user's per-stream volume choice is published through the stream's
//! `GainControlHandle`. `MixerStreamSession::recover_stream` re-admits a
//! persisted stream against a fresh mixer slot using the retained handle, so
//! the published gain and mute state must survive output-device recreation
//! and continue to apply to exactly the owning stream. These tests drive two
//! admitted streams with distinct gain and mute settings through a real
//! decode-and-adapt cycle, fault and recover the output, and assert that the
//! recovered mixed bus still reflects each stream's setting at steady state.

use core::convert::Infallible;
use std::sync::Arc;

use sonimesh_app::{MediaStreamIdentity, MixerStreamSession, StreamStartRequest};
use sonimesh_audio::{FrameDecoder, realtime_pcm_queue};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceInfo, OutputError, OutputHealth, RenderSource,
    SampleFormat,
};
use sonimesh_protocol::{MediaDirection, MediaKeyContext, MediaSealer, SessionSecret};

/// Two streams share the mixer; each occupies one slot.
const SLOTS: usize = 2;
/// One output block is 1 ms at 48 kHz; each source block is 1 ms at 8 kHz.
const BLOCK: usize = 48;
/// The decode pipeline compiles for the 8-frame source block.
const PIPELINE_BLOCK: usize = 8;
/// Source blocks decoded, adapter pumps, and rendered blocks per feed cycle.
const STEADY_STATE_BLOCKS: u32 = 5;

const IDENTITY_A: MediaStreamIdentity = MediaStreamIdentity {
    session_id: 1,
    stream_id: 1,
    key_epoch: 0,
};
const IDENTITY_B: MediaStreamIdentity = MediaStreamIdentity {
    session_id: 2,
    stream_id: 1,
    key_epoch: 0,
};

/// Decoder that fills each whole PCM block with a constant sample.
struct ConstantDecoder(f32);

impl FrameDecoder for ConstantDecoder {
    type Error = Infallible;

    fn decode(&mut self, _encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
        output.fill(self.0);
        Ok(output.len())
    }

    fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
        output.fill(0.0);
        Ok(output.len())
    }
}

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

fn selection() -> sonimesh_app::OutputSelection {
    sonimesh_app::OutputSelection {
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

fn context(session_id: u64) -> MediaKeyContext {
    MediaKeyContext::new(session_id, 1, 0, MediaDirection::AndroidToDesktop).unwrap()
}

fn request(identity: MediaStreamIdentity) -> StreamStartRequest {
    StreamStartRequest {
        identity,
        // 8_000 Hz x 1 ms = 8 frames per block, matching PIPELINE_BLOCK.
        sample_rate_hz: 8_000,
        channels: 1,
        frame_duration_ms: 1,
    }
}

/// One admitted stream under test: its pipeline and sealer.
struct StreamHarness {
    pipeline: sonimesh_app::ReceiverStreamPipeline<ConstantDecoder, 4, 8, PIPELINE_BLOCK>,
    sealer: MediaSealer,
}

fn harness(session_id: u64, sample: f32) -> StreamHarness {
    let secret = SessionSecret::new([0xA5; 32]);
    let (producer, _consumer) =
        realtime_pcm_queue::<PIPELINE_BLOCK>(4).expect("bounded test queue is valid");
    let counters = Arc::new(StreamCounters::default());
    let pipeline = sonimesh_app::ReceiverStreamPipeline::new(
        &secret,
        context(session_id),
        ConstantDecoder(sample),
        producer,
        counters,
        0,
    )
    .expect("test pipeline is valid");
    StreamHarness {
        pipeline,
        sealer: MediaSealer::new(&secret, context(session_id)).expect("test sealer is valid"),
    }
}

/// Seals, receives, decodes, and pumps `STEADY_STATE_BLOCKS` source blocks for
/// both harnesses, then renders enough output blocks for the per-slot gain
/// ramps (128 samples) to fully settle. Returns the last rendered block.
fn feed_cycle(
    session: &mut MixerStreamSession<TestOutput, SLOTS, BLOCK>,
    a: &mut StreamHarness,
    b: &mut StreamHarness,
    first_sequence_a: u32,
    first_sequence_b: u32,
) -> [f32; BLOCK] {
    for block in 0..STEADY_STATE_BLOCKS {
        feed_block(&mut a.pipeline, &mut a.sealer, first_sequence_a + block);
        feed_block(&mut b.pipeline, &mut b.sealer, first_sequence_b + block);
        session.pump_all().unwrap();
    }
    // Render the pushed blocks: the per-slot 128-sample gain ramps settle
    // during the first renders, so the returned block's tail is steady state.
    let mut rendered = [0.0_f32; BLOCK];
    for _ in 0..3 {
        session.output_mut().render(&mut rendered);
    }
    rendered
}

fn feed_block(
    pipeline: &mut sonimesh_app::ReceiverStreamPipeline<ConstantDecoder, 4, 8, PIPELINE_BLOCK>,
    sealer: &mut MediaSealer,
    sequence: u32,
) {
    let payload = [0x2A; 8];
    let packet = sealer.seal(0, u64::from(sequence) * 8, &payload).unwrap();
    let mut plaintext = [0_u8; 8];
    pipeline.receive(&packet.datagram, &mut plaintext).unwrap();
    pipeline.decode_next().unwrap();
}

fn session() -> MixerStreamSession<TestOutput, SLOTS, BLOCK> {
    MixerStreamSession::new(
        TestOutput {
            health: OutputHealth::Healthy,
            start_calls: 0,
            stop_calls: 0,
            fail_start: false,
            source: None,
        },
        16,
    )
    .unwrap()
}

#[test]
fn per_stream_gain_and_mute_survive_output_recovery() {
    let mut session = session();
    let mut a = harness(1, 0.25);
    let mut b = harness(2, 0.5);

    session.start_output(selection()).unwrap();
    session
        .start_stream::<_, PIPELINE_BLOCK>(request(IDENTITY_A), 16, None, &mut a.pipeline)
        .unwrap();
    session
        .start_stream::<_, PIPELINE_BLOCK>(request(IDENTITY_B), 16, None, &mut b.pipeline)
        .unwrap();

    // Distinct per-stream settings: A is muted, B is at half gain.
    let gain_a = session.gain(IDENTITY_A).unwrap();
    let gain_b = session.gain(IDENTITY_B).unwrap();
    gain_a.set_muted(true);
    gain_b.set_gain(0.5).unwrap();
    assert!(gain_a.is_muted());
    assert!((gain_b.gain() - 0.5).abs() < 1e-6);

    // Settle the gain ramps, then recover the output.
    feed_cycle(&mut session, &mut a, &mut b, 0, 0);
    session.output_mut().health = OutputHealth::StreamError;
    assert_eq!(session.monitor(), Some(OutputHealth::StreamError));
    let identities = session.recover_output().unwrap();
    assert_eq!(identities, vec![IDENTITY_A, IDENTITY_B]);
    session
        .recover_stream::<_, PIPELINE_BLOCK>(IDENTITY_A, &mut a.pipeline)
        .unwrap();
    session
        .recover_stream::<_, PIPELINE_BLOCK>(IDENTITY_B, &mut b.pipeline)
        .unwrap();

    // The retained handles still expose the user's choices.
    let recovered_a = session.gain(IDENTITY_A).unwrap();
    let recovered_b = session.gain(IDENTITY_B).unwrap();
    assert!(
        recovered_a.is_muted(),
        "stream A mute must survive recovery"
    );
    assert!(
        (recovered_b.gain() - 0.5).abs() < 1e-6,
        "stream B gain must survive recovery"
    );

    // Steady-state mixed output after recovery: only stream B contributes.
    let rendered = feed_cycle(&mut session, &mut a, &mut b, 10, 10);
    let expected = 0.5 * 0.5;
    assert!(
        rendered[32..]
            .iter()
            .all(|sample| (*sample - expected).abs() <= 1e-4),
        "recovered mixed output tail {rendered:?} must equal {expected}"
    );
}

#[test]
fn gain_handle_published_after_recovery_affects_only_its_stream() {
    let mut session = session();
    let mut a = harness(1, 0.25);
    let mut b = harness(2, 0.5);

    session.start_output(selection()).unwrap();
    session
        .start_stream::<_, PIPELINE_BLOCK>(request(IDENTITY_A), 16, None, &mut a.pipeline)
        .unwrap();
    session
        .start_stream::<_, PIPELINE_BLOCK>(request(IDENTITY_B), 16, None, &mut b.pipeline)
        .unwrap();

    // Recover once with a default-unity A and half-gain B.
    session.output_mut().health = OutputHealth::StreamError;
    assert_eq!(session.monitor(), Some(OutputHealth::StreamError));
    let identities = session.recover_output().unwrap();
    assert_eq!(identities, vec![IDENTITY_A, IDENTITY_B]);
    session
        .recover_stream::<_, PIPELINE_BLOCK>(IDENTITY_A, &mut a.pipeline)
        .unwrap();
    session
        .recover_stream::<_, PIPELINE_BLOCK>(IDENTITY_B, &mut b.pipeline)
        .unwrap();

    // Publish a fresh gain to stream A only after recovery.
    session.gain(IDENTITY_A).unwrap().set_gain(0.5).unwrap();
    let rendered = feed_cycle(&mut session, &mut a, &mut b, 10, 10);
    let expected = 0.25 * 0.5 + 0.5 * 1.0;
    assert!(
        rendered[32..]
            .iter()
            .all(|sample| (*sample - expected).abs() <= 1e-4),
        "post-recovery gain must scale only stream A: {rendered:?}"
    );
}
