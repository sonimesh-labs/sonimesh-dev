//! Deterministic property coverage for the control-to-mixer bridge composition.
//!
//! The bridge (ADR 0015) composes the control coordinator and the mixer stream
//! session into one production-facing object. Seeded random walks drive
//! authenticated start, rotation, stop, output-fault recovery, reconnect grace,
//! and session expiry through one bridge, then compare the control coordinator
//! and the mixer session after every step. The two domains must always report
//! the same bounded stream count, the bridge output-active flag must match the
//! mixer output snapshot (a fault must never leave the snapshot claiming an
//! active output), and a fail-closed operation must leave both domains
//! unchanged.

use sonimesh_app::{
    ControlSessionToken, ControlStreamAction, ControlToMixerBridge, PairingId,
    PairingInvitationController, PairingProofVerification, PairingSource,
    ReceiverPcmProducerInstaller, SessionEvent, SessionPhase, SessionState,
};
use sonimesh_audio::RealtimePcmProducer;
use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceInfo, OutputError, OutputHealth, RenderSource,
    SampleFormat,
};
use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, Envelope, Hello, StartStream, StopStream, envelope,
};

const SLOTS: usize = 4;
const BLOCK: usize = 480;
const SESSION_ID: u64 = 1;

type Bridge = ControlToMixerBridge<FuzzOutput, SLOTS, BLOCK>;

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

    fn next_u8(&mut self) -> u8 {
        self.next_u32().to_le_bytes()[0]
    }

    fn next_u64(&mut self) -> u64 {
        let mut bytes = [0_u8; 8];
        for byte in &mut bytes {
            *byte = self.next_u8();
        }
        u64::from_le_bytes(bytes)
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        self.next_u32() as usize % bound
    }

    /// Random stream identifier in `1..=SLOTS`.
    fn next_stream_id(&mut self) -> u32 {
        // The slot bound is a small compile-time constant that always fits u32.
        let bound = u32::try_from(SLOTS).expect("slot count fits u32");
        1 + self.next_u32() % bound
    }
}

/// Audio output stub that records backend health and renders through the
/// captured callback source so membership commands are applied.
struct FuzzOutput {
    health: OutputHealth,
    fail_start: bool,
    source: Option<Box<dyn RenderSource>>,
}

impl FuzzOutput {
    fn render(&mut self, output: &mut [f32]) {
        if let Some(source) = self.source.as_mut() {
            source.render(output);
        }
    }
}

impl AudioOutput for FuzzOutput {
    fn devices(&self) -> Result<Vec<OutputDeviceInfo>, OutputError> {
        Ok(Vec::new())
    }

    fn start(
        &mut self,
        _device: Option<&sonimesh_platform::OutputDeviceId>,
        _config: OutputConfig,
        source: Box<dyn RenderSource>,
    ) -> Result<(), OutputError> {
        if self.fail_start {
            return Err(OutputError::Backend(
                "property-test start failure".to_owned(),
            ));
        }
        self.source = Some(source);
        Ok(())
    }

    fn stop(&mut self) {
        self.source = None;
    }

    fn health(&self) -> OutputHealth {
        self.health
    }
}

/// Pipeline installer that discards each paired producer; the property walk
/// never feeds media, so only the control-domain boundary is exercised.
struct FuzzPipeline;

impl ReceiverPcmProducerInstaller<BLOCK> for FuzzPipeline {
    fn replace_pcm_producer(&mut self, _producer: RealtimePcmProducer<BLOCK>) {}
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

fn capabilities() -> &'static [Capability] {
    &[Capability::FrameDuration10Ms]
}

fn hello() -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 1,
        payload: Some(envelope::Payload::Hello(Hello {
            supported_versions: vec![CONTROL_PROTOCOL_VERSION],
            capabilities: capabilities()
                .iter()
                .map(|capability| *capability as i32)
                .collect(),
            display_name: "Lab sender".to_owned(),
        })),
    }
}

fn start_for(session_id: u64, stream_id: u32, epoch: u32) -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: u64::from(epoch) * 100 + u64::from(stream_id) + 1,
        payload: Some(envelope::Payload::StartStream(StartStream {
            media_session_id: session_id,
            stream_id,
            sample_rate_hz: 48_000,
            channels: 1,
            frame_duration_ms: 10,
            media_key_epoch: epoch,
        })),
    }
}

fn stop_for(session_id: u64, stream_id: u32, epoch: u32) -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: u64::from(epoch) * 100 + u64::from(stream_id) + 100,
        payload: Some(envelope::Payload::StopStream(StopStream {
            media_session_id: session_id,
            stream_id,
            media_key_epoch: epoch,
        })),
    }
}

/// One trusted, negotiated bridge with a live authenticated control session.
struct Harness {
    bridge: Bridge,
    token: ControlSessionToken,
    pipeline: FuzzPipeline,
}

impl Harness {
    fn establish() -> Option<Self> {
        let mut pairing = PairingInvitationController::<1>::new();
        let mut session = SessionState::IDLE;
        pairing
            .begin(&mut session, PairingId::new([1; 16]), 100, 50, 1)
            .ok()?;
        pairing
            .verify_proof(&mut session, 101, PairingSource::new([2; 16]), || {
                PairingProofVerification::Verified
            })
            .ok()?;
        let mut bridge = ControlToMixerBridge::new(
            session,
            capabilities(),
            FuzzOutput {
                health: OutputHealth::Healthy,
                fail_start: false,
                source: None,
            },
            selection(),
            16,
            16,
        )
        .ok()?;
        let token = bridge.install_authenticated_control_session().ok()?;
        if !matches!(
            bridge.accept(token, &hello()),
            Ok(ControlStreamAction::Negotiated(_))
        ) {
            return None;
        }
        Some(Self {
            bridge,
            token,
            pipeline: FuzzPipeline,
        })
    }

    /// Applies one pending render so membership commands reach the callback
    /// mixer, keeping the bounded command queue drained between operations.
    fn drain(&mut self) {
        let mut block = [0.0_f32; BLOCK];
        self.bridge.mixer_mut().output_mut().render(&mut block);
    }

    /// Rebuilds the bridge after a session expiry retires every token.
    fn reestablish(&mut self) {
        if let Some(rebuilt) = Self::establish() {
            *self = rebuilt;
        }
    }
}

/// Asserts the cross-domain invariants after every bridge operation.
fn assert_invariants(bridge: &Bridge) {
    let snapshot = bridge.snapshot();
    let mixer_streams = bridge.mixer().stream_count();
    let coordinator_streams = bridge.coordinator().stream_count();
    assert_eq!(
        snapshot.active_streams(),
        mixer_streams,
        "bridge snapshot stream count diverged from the mixer session"
    );
    assert_eq!(
        snapshot.active_streams(),
        coordinator_streams,
        "bridge snapshot stream count diverged from the control coordinator"
    );
    assert!(
        snapshot.active_streams() <= SLOTS,
        "mixer admitted past the fixed slot bound"
    );
    assert!(
        snapshot.active_streams() <= sonimesh_app::MAX_ACTIVE_STREAMS,
        "coordinator admitted past the active stream bound"
    );
    assert_eq!(
        snapshot.output_active(),
        bridge.output_active(),
        "bridge output-active flag diverged from its own snapshot"
    );
    assert_eq!(
        snapshot.output_active(),
        bridge.mixer().snapshot().stream_active(),
        "bridge output-active flag diverged from the mixer output snapshot"
    );
}

/// Feeds one StartStream envelope and commits or aborts the produced action.
fn op_start(harness: &mut Harness, rng: &mut XorShift64, commit: bool) {
    let stream_id = rng.next_stream_id();
    let epoch = u32::from(rng.next_u8() % 4);
    let envelope = start_for(SESSION_ID, stream_id, epoch);
    let Ok(action) = harness.bridge.accept(harness.token, &envelope) else {
        return;
    };
    match action {
        ControlStreamAction::Start { token, request } => {
            if commit {
                let _ = harness.bridge.execute_start::<FuzzPipeline, BLOCK>(
                    token,
                    request,
                    None,
                    &mut harness.pipeline,
                );
            } else {
                let _ = harness.bridge.coordinator_mut().abort_pending(token);
            }
        }
        ControlStreamAction::Rotate {
            token,
            previous,
            next,
        } => {
            if commit {
                let _ = harness.bridge.execute_rotation(token, previous, next);
            } else {
                let _ = harness.bridge.coordinator_mut().abort_pending(token);
            }
        }
        ControlStreamAction::Stop { .. }
        | ControlStreamAction::Negotiated(_)
        | ControlStreamAction::Noop
        | ControlStreamAction::Ignored => {}
    }
}

/// Feeds one StopStream envelope and commits or aborts the produced action.
fn op_stop(harness: &mut Harness, rng: &mut XorShift64, commit: bool) {
    let stream_id = rng.next_stream_id();
    let epoch = u32::from(rng.next_u8() % 4);
    let envelope = stop_for(SESSION_ID, stream_id, epoch);
    let Ok(action) = harness.bridge.accept(harness.token, &envelope) else {
        return;
    };
    if let ControlStreamAction::Stop { token, identity } = action {
        if commit {
            let _ = harness.bridge.execute_stop(token, identity);
        } else {
            let _ = harness.bridge.coordinator_mut().abort_pending(token);
        }
    }
}

/// Faults the backend, retires the output, and attempts one recovery with a
/// randomly failing backend start.
fn op_fault_and_recover(harness: &mut Harness, rng: &mut XorShift64) {
    harness.bridge.mixer_mut().output_mut().health = OutputHealth::StreamError;
    match harness.bridge.monitor() {
        Ok(Some(_)) => {
            assert!(!harness.bridge.output_active());
            assert!(!harness.bridge.mixer().snapshot().stream_active());
            harness.bridge.mixer_mut().output_mut().fail_start = rng.next_u8() % 4 == 0;
            if let Ok(identities) = harness.bridge.recover_output() {
                harness.bridge.mixer_mut().output_mut().fail_start = false;
                for identity in identities {
                    let _ = harness
                        .bridge
                        .recover_stream::<FuzzPipeline, BLOCK>(identity, &mut harness.pipeline);
                }
                assert!(harness.bridge.output_active());
                assert!(harness.bridge.mixer().snapshot().stream_active());
            } else {
                // Recovery crossed a retired lifecycle: both domains must
                // agree the output is still down.
                assert!(!harness.bridge.output_active());
                assert!(!harness.bridge.mixer().snapshot().stream_active());
            }
        }
        Ok(None) => {}
        Err(_) => {
            // A fault during a lifecycle that cannot represent OutputLost (for
            // example reconnect grace) must still leave the output honestly
            // down, because the mixer already stopped it.
            assert!(!harness.bridge.output_active());
            assert!(!harness.bridge.mixer().snapshot().stream_active());
        }
    }
}

/// Loses the control connection, arms the reconnect window, and observes it
/// with an adversarial clock.
fn op_reconnect(harness: &mut Harness, rng: &mut XorShift64) {
    if harness.bridge.state().phase() != SessionPhase::Reconnecting {
        let _ = harness
            .bridge
            .acknowledge_platform_event(SessionEvent::ConnectionLost);
    }
    if harness.bridge.state().phase() == SessionPhase::Reconnecting {
        let now_ms = rng.next_u64() % 1_000_000;
        let duration_ms = rng.next_u64() % 10_000 + 1;
        let _ = harness.bridge.begin_reconnect_grace(now_ms, duration_ms);
        let _ = harness
            .bridge
            .observe_reconnect_grace(now_ms + rng.next_u64() % 20_000);
        if harness.bridge.state().phase() == SessionPhase::Idle {
            assert_eq!(harness.bridge.mixer().stream_count(), 0);
            assert!(!harness.bridge.output_active());
        }
    }
}

/// Reconnects with fresh session material and reinstalls a negotiated session.
fn op_reconnected(harness: &mut Harness) {
    if harness.bridge.state().phase() != SessionPhase::Reconnecting {
        return;
    }
    if harness
        .bridge
        .acknowledge_platform_event(SessionEvent::Reconnected)
        .is_err()
    {
        return;
    }
    assert_eq!(harness.bridge.coordinator().stream_count(), 0);
    assert_eq!(harness.bridge.mixer().stream_count(), 0);
    assert!(!harness.bridge.output_active());
    if let Ok(token) = harness.bridge.install_authenticated_control_session() {
        harness.token = token;
        let _ = harness.bridge.accept(token, &hello());
    }
}

fn op_expire(harness: &mut Harness) {
    harness.bridge.expire_control_session();
    assert_eq!(harness.bridge.coordinator().stream_count(), 0);
    assert_eq!(harness.bridge.mixer().stream_count(), 0);
    assert!(!harness.bridge.output_active());
}

fn run_walk(seed: u64) {
    let mut rng = XorShift64(seed);
    let mut harness = Harness::establish().expect("trusted bridge must establish");
    for _ in 0..1_500 {
        if harness.bridge.state().phase() == SessionPhase::Idle {
            harness.reestablish();
        }
        match rng.next_usize(9) {
            0 => op_start(&mut harness, &mut rng, true),
            1 => op_start(&mut harness, &mut rng, false),
            2 => op_stop(&mut harness, &mut rng, true),
            3 => op_stop(&mut harness, &mut rng, false),
            4 => op_fault_and_recover(&mut harness, &mut rng),
            5 => op_reconnect(&mut harness, &mut rng),
            6 => op_reconnected(&mut harness),
            7 => op_expire(&mut harness),
            _ => {
                // Randomly rotate one active stream to a newer key epoch.
                let stream_id = rng.next_stream_id();
                let epoch = 1 + u32::from(rng.next_u8() % 4);
                let envelope = start_for(SESSION_ID, stream_id, epoch);
                let Ok(action) = harness.bridge.accept(harness.token, &envelope) else {
                    continue;
                };
                if let ControlStreamAction::Rotate {
                    token,
                    previous,
                    next,
                } = action
                {
                    if rng.next_u8() % 2 == 0 {
                        let _ = harness.bridge.execute_rotation(token, previous, next);
                    } else {
                        let _ = harness.bridge.coordinator_mut().abort_pending(token);
                    }
                }
            }
        }
        harness.drain();
        assert_invariants(&harness.bridge);
    }
}

#[test]
fn seeded_walks_keep_control_and_mixer_in_agreement() {
    for seed in [1, 2, 3, 4, 5, 6] {
        run_walk(seed);
    }
}
