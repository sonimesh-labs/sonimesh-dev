#![no_main]

//! Fuzz the control-to-mixer bridge composition.
//!
//! The bridge (ADR 0015) composes the control coordinator and the mixer stream
//! session into one production-facing object. This target establishes one
//! trusted, negotiated control session and then drives arbitrary start,
//! rotation, stop, output-fault recovery, reconnect-grace, and session-expiry
//! scripts through the real pairing path and the tokenized completion
//! contract. After every step it asserts the cross-domain invariants: the
//! coordinator and the mixer must report the same bounded stream count, the
//! bridge output-active flag must match the mixer output snapshot (a fault or
//! grace expiry must never leave the snapshot claiming a live output), and a
//! fail-closed operation must leave both domains unchanged.

use libfuzzer_sys::fuzz_target;
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
            return Err(OutputError::Backend("fuzz start failure".to_owned()));
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

/// Pipeline installer that discards each paired producer; the fuzz script
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

/// Reads one byte, wrapping around the slice.
fn next_byte(data: &[u8], offset: &mut usize) -> u8 {
    let byte = data[*offset % data.len()];
    *offset += 1;
    byte
}

/// Reads one little-endian `u64`, wrapping around the slice.
fn next_u64(data: &[u8], offset: &mut usize) -> u64 {
    let mut bytes = [0_u8; 8];
    for byte in &mut bytes {
        *byte = next_byte(data, offset);
    }
    u64::from_le_bytes(bytes)
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
fn op_start(harness: &mut Harness, data: &[u8], offset: &mut usize, commit: bool) {
    let stream_id = 1 + u32::from(next_byte(data, offset) % SLOTS as u8);
    let epoch = u32::from(next_byte(data, offset) % 4);
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
fn op_stop(harness: &mut Harness, data: &[u8], offset: &mut usize, commit: bool) {
    let stream_id = 1 + u32::from(next_byte(data, offset) % SLOTS as u8);
    let epoch = u32::from(next_byte(data, offset) % 4);
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

/// Feeds one newer-epoch StartStream to rotate one active stream.
fn op_rotate(harness: &mut Harness, data: &[u8], offset: &mut usize) {
    let stream_id = 1 + u32::from(next_byte(data, offset) % SLOTS as u8);
    let epoch = 1 + u32::from(next_byte(data, offset) % 4);
    let envelope = start_for(SESSION_ID, stream_id, epoch);
    let Ok(action) = harness.bridge.accept(harness.token, &envelope) else {
        return;
    };
    if let ControlStreamAction::Rotate {
        token,
        previous,
        next,
    } = action
    {
        if next_byte(data, offset).is_multiple_of(2) {
            let _ = harness.bridge.execute_rotation(token, previous, next);
        } else {
            let _ = harness.bridge.coordinator_mut().abort_pending(token);
        }
    }
}

/// Faults the backend, retires the output, and attempts one recovery with a
/// randomly failing backend start.
fn op_fault_and_recover(harness: &mut Harness, data: &[u8], offset: &mut usize) {
    harness.bridge.mixer_mut().output_mut().health = OutputHealth::StreamError;
    match harness.bridge.monitor() {
        Ok(Some(_)) => {
            assert!(!harness.bridge.output_active());
            assert!(!harness.bridge.mixer().snapshot().stream_active());
            harness.bridge.mixer_mut().output_mut().fail_start =
                next_byte(data, offset).is_multiple_of(4);
            match harness.bridge.recover_output() {
                Ok(identities) => {
                    harness.bridge.mixer_mut().output_mut().fail_start = false;
                    for identity in identities {
                        let _ = harness
                            .bridge
                            .recover_stream::<FuzzPipeline, BLOCK>(identity, &mut harness.pipeline);
                    }
                    assert!(harness.bridge.output_active());
                    assert!(harness.bridge.mixer().snapshot().stream_active());
                }
                Err(_) => {
                    // Recovery crossed a retired lifecycle: both domains must
                    // agree the output is still down.
                    assert!(!harness.bridge.output_active());
                    assert!(!harness.bridge.mixer().snapshot().stream_active());
                }
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
fn op_reconnect(harness: &mut Harness, data: &[u8], offset: &mut usize) {
    if harness.bridge.state().phase() != SessionPhase::Reconnecting {
        let _ = harness
            .bridge
            .acknowledge_platform_event(SessionEvent::ConnectionLost);
    }
    if harness.bridge.state().phase() == SessionPhase::Reconnecting {
        let now_ms = next_u64(data, offset) % 1_000_000;
        let duration_ms = next_u64(data, offset) % 10_000 + 1;
        let _ = harness.bridge.begin_reconnect_grace(now_ms, duration_ms);
        let _ = harness
            .bridge
            .observe_reconnect_grace(now_ms + next_u64(data, offset) % 20_000);
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

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    let Some(mut harness) = Harness::establish() else {
        return;
    };
    let mut offset = 0;
    let steps = 1 + data.len() % 128;
    for _ in 0..steps {
        if harness.bridge.state().phase() == SessionPhase::Idle {
            harness.reestablish();
        }
        match next_byte(data, &mut offset) % 9 {
            0 => op_start(&mut harness, data, &mut offset, true),
            1 => op_start(&mut harness, data, &mut offset, false),
            2 => op_stop(&mut harness, data, &mut offset, true),
            3 => op_stop(&mut harness, data, &mut offset, false),
            4 => op_fault_and_recover(&mut harness, data, &mut offset),
            5 => op_reconnect(&mut harness, data, &mut offset),
            6 => op_reconnected(&mut harness),
            7 => op_expire(&mut harness),
            _ => op_rotate(&mut harness, data, &mut offset),
        }
        harness.drain();
        assert_invariants(&harness.bridge);
    }
});
