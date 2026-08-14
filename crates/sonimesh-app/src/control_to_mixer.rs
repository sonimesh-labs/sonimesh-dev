//! Control-to-mixer bridge composing the control coordinator and the mixer
//! stream session.
//!
//! This is the production-facing composition layer for the Phase 2 mixer path
//! (ADR 0015). One [`ControlToMixerBridge`] owns the [`ControlStreamCoordinator`]
//! and the [`MixerStreamSession`], so the app can drive both from one object:
//! the bridge prepares platform output, admits and removes per-stream pipelines,
//! commits tokenized completions, and rolls back partial work when a start,
//! rotation, or stop fails. It is not part of the realtime scheduling path; the
//! caller still advances adapter pumps and renders through the owned mixer
//! session.

use std::net::SocketAddr;
use std::sync::Arc;

use sonimesh_audio::{MAX_PCM_QUEUE_BLOCKS, RealtimeQueueConfigError};
use sonimesh_diagnostics::{
    ExportPlatformFormat, ExportSessionPhase, ExportSessionSnapshot, StreamCounters,
    StreamMetricsSnapshot,
};
use sonimesh_platform::{AudioOutput, OutputHealth};
use sonimesh_protocol::{Capability, Envelope};

use crate::{
    ControlOperationToken, ControlSessionToken, ControlStreamAction, ControlStreamCoordinator,
    ControlStreamError, DesktopControlSnapshot, MediaStreamIdentity, MixerAdmissionError,
    MixerStreamSession, MixerStreamSessionError, OutputSelection, OutputSnapshot,
    OutputSnapshotSource, PairingInvitationController, PairingInvitationError,
    ReceiverPcmProducerInstaller, ReconnectGraceObservation, SessionEvent, SessionPhase,
    SessionState, StartedMixerStream, StreamStartRequest,
};

/// Control or mixer failure surfaced by the bridge.
#[derive(Debug)]
pub enum ControlToMixerError {
    /// The control coordinator rejected the operation.
    Control(ControlStreamError),
    /// The mixer session rejected the operation.
    Mixer(MixerStreamSessionError),
}

impl core::fmt::Display for ControlToMixerError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Control(error) => write!(formatter, "control-to-mixer control failure: {error}"),
            Self::Mixer(error) => write!(formatter, "control-to-mixer mixer failure: {error}"),
        }
    }
}

impl std::error::Error for ControlToMixerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Control(error) => Some(error),
            Self::Mixer(error) => Some(error),
        }
    }
}

/// Redacted low-rate bridge state safe to project into a control-surface
/// adapter.
///
/// This value deliberately excludes pairing identifiers, stream identities,
/// keys, addresses, audio frames, and diagnostics counters; it carries only
/// the lifecycle phase, the output lifecycle, and membership counts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BridgeSnapshot {
    session: SessionPhase,
    output: OutputSnapshot,
    output_active: bool,
    active_streams: usize,
    pending_mixer_commands: usize,
}

impl BridgeSnapshot {
    /// Returns the current control-domain lifecycle phase.
    #[must_use]
    pub const fn session(&self) -> SessionPhase {
        self.session
    }

    /// Returns the redacted platform output lifecycle.
    #[must_use]
    pub const fn output(&self) -> OutputSnapshot {
        self.output
    }

    /// Returns whether the platform output is currently started.
    #[must_use]
    pub const fn output_active(&self) -> bool {
        self.output_active
    }

    /// Returns the number of admitted mixer streams.
    #[must_use]
    pub const fn active_streams(&self) -> usize {
        self.active_streams
    }

    /// Returns membership commands waiting for the next callback boundary.
    #[must_use]
    pub const fn pending_mixer_commands(&self) -> usize {
        self.pending_mixer_commands
    }
}

/// One object owning the control coordinator and the mixer stream session.
///
/// Stream lifecycle must flow through [`Self::execute_start`],
/// [`Self::execute_rotation`], and [`Self::execute_stop`] so the coordinator's
/// tokenized completions stay in sync with mixer admission. The platform output
/// starts with the first stream and stops with the last; a fault retires the
/// membership boundary and [`Self::recover_output`] recreates the source so the
/// caller can re-admit every persisted stream through
/// [`Self::recover_stream`].
pub struct ControlToMixerBridge<O, const SLOTS: usize, const OUTPUT_SAMPLES_PER_BLOCK: usize> {
    coordinator: ControlStreamCoordinator,
    mixer: MixerStreamSession<O, SLOTS, OUTPUT_SAMPLES_PER_BLOCK>,
    selection: OutputSelection,
    source_queue_capacity: usize,
    output_active: bool,
}

impl<O, const SLOTS: usize, const OUTPUT_SAMPLES_PER_BLOCK: usize>
    ControlToMixerBridge<O, SLOTS, OUTPUT_SAMPLES_PER_BLOCK>
where
    O: AudioOutput,
{
    /// Creates a stopped bridge with a fixed output selection and bounded queues.
    ///
    /// # Errors
    ///
    /// Returns a control error when the local capability policy exceeds the
    /// protocol bound, or a mixer error when the mixer queue capacity or the
    /// per-stream source queue capacity is outside the bounded PCM range.
    pub fn new(
        state: SessionState,
        capabilities: &[Capability],
        output: O,
        selection: OutputSelection,
        queue_capacity: usize,
        source_queue_capacity: usize,
    ) -> Result<Self, ControlToMixerError> {
        let coordinator = ControlStreamCoordinator::new(state, capabilities)
            .map_err(ControlToMixerError::Control)?;
        let mixer =
            MixerStreamSession::new(output, queue_capacity).map_err(ControlToMixerError::Mixer)?;
        if source_queue_capacity == 0 {
            return Err(ControlToMixerError::Mixer(
                MixerStreamSessionError::Membership(MixerAdmissionError::Queue(
                    RealtimeQueueConfigError::ZeroCapacity,
                )),
            ));
        }
        if source_queue_capacity > MAX_PCM_QUEUE_BLOCKS {
            return Err(ControlToMixerError::Mixer(
                MixerStreamSessionError::Membership(MixerAdmissionError::Queue(
                    RealtimeQueueConfigError::CapacityTooLarge,
                )),
            ));
        }
        Ok(Self {
            coordinator,
            mixer,
            selection,
            source_queue_capacity,
            output_active: false,
        })
    }

    /// Returns the current control-domain lifecycle state.
    #[must_use]
    pub const fn state(&self) -> SessionState {
        self.coordinator.state()
    }

    /// Returns whether the platform output is currently started.
    #[must_use]
    pub const fn output_active(&self) -> bool {
        self.output_active
    }

    /// Returns a redacted low-rate snapshot for a control-surface adapter.
    #[must_use]
    pub fn snapshot(&self) -> BridgeSnapshot {
        BridgeSnapshot {
            session: self.state().phase(),
            output: self.mixer.snapshot(),
            output_active: self.output_active,
            active_streams: self.mixer.stream_count(),
            pending_mixer_commands: self.mixer.pending_commands(),
        }
    }

    /// Projects the redacted desktop control snapshot from this bridge.
    ///
    /// This is the control-surface entry point for the composition path: the
    /// pairing owner applies its monotonic clock before projecting, and the
    /// output lifecycle is read from this bridge's own mixer session, so a
    /// caller cannot present an active invitation or fabricated output state.
    /// The returned value excludes pairing identifiers, proofs, addresses,
    /// session material, and audio frames.
    ///
    /// # Errors
    ///
    /// Returns [`PairingInvitationError`] when the pairing clock observation is
    /// invalid. An owner with no retained invitation still returns a snapshot.
    pub fn desktop_snapshot_at<const MAX_SOURCES: usize>(
        &mut self,
        pairing: &mut PairingInvitationController<MAX_SOURCES>,
        now_ms: u64,
    ) -> Result<DesktopControlSnapshot, PairingInvitationError> {
        let output = self.mixer.snapshot();
        self.coordinator
            .snapshot_at(pairing, now_ms, &OwnedSnapshot(output))
    }

    /// Borrows the control coordinator for read-only access.
    #[must_use]
    pub const fn coordinator(&self) -> &ControlStreamCoordinator {
        &self.coordinator
    }

    /// Mutably borrows the control coordinator for pairing and lifecycle owner
    /// operations that are not stream lifecycle (start/rotate/stop).
    pub fn coordinator_mut(&mut self) -> &mut ControlStreamCoordinator {
        &mut self.coordinator
    }

    /// Borrows the mixer session for read-only control-domain access.
    #[must_use]
    pub const fn mixer(&self) -> &MixerStreamSession<O, SLOTS, OUTPUT_SAMPLES_PER_BLOCK> {
        &self.mixer
    }

    /// Mutably borrows the mixer session for pump and drift control.
    pub fn mixer_mut(&mut self) -> &mut MixerStreamSession<O, SLOTS, OUTPUT_SAMPLES_PER_BLOCK> {
        &mut self.mixer
    }

    /// Installs one freshly authenticated control transport.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStreamError`] when the lifecycle is not ready or a
    /// transport is already installed.
    pub fn install_authenticated_control_session(
        &mut self,
    ) -> Result<ControlSessionToken, ControlStreamError> {
        self.coordinator.install_authenticated_control_session()
    }

    /// Validates one authenticated control envelope against the coordinator.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStreamError`] when the envelope is invalid or the
    /// lifecycle rejects the command.
    pub fn accept(
        &mut self,
        token: ControlSessionToken,
        envelope: &Envelope,
    ) -> Result<ControlStreamAction, ControlStreamError> {
        self.coordinator.accept(token, envelope)
    }

    /// Applies a platform acknowledgement that is not a stream lifecycle commit.
    ///
    /// Reconnect and output events that invalidate stream state also retire the
    /// mixer: reconnection carries fresh session material, and a directly
    /// acknowledged output restoration without [`Self::recover_output`] must not
    /// leave stale producers behind.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStreamError`] when the current state rejects the event.
    pub fn acknowledge_platform_event(
        &mut self,
        event: SessionEvent,
    ) -> Result<(), ControlStreamError> {
        self.coordinator.acknowledge_platform_event(event)?;
        match event {
            SessionEvent::OutputLost => {
                self.output_active = false;
            }
            SessionEvent::OutputRestored | SessionEvent::Reconnected => {
                self.mixer.stop();
                self.output_active = false;
            }
            SessionEvent::ConnectionLost
            | SessionEvent::StartPairing
            | SessionEvent::StartStream
            | SessionEvent::StopStream
            | SessionEvent::SessionExpired => {}
        }
        Ok(())
    }

    /// Expires the authenticated control session and stops the mixer output.
    pub fn expire_control_session(&mut self) {
        self.coordinator.expire_control_session();
        self.mixer.stop();
        self.output_active = false;
    }

    /// Arms the bounded reconnect window after a connection loss.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStreamError`] when the lifecycle is not reconnecting or
    /// the window cannot be armed.
    pub fn begin_reconnect_grace(
        &mut self,
        now_ms: u64,
        duration_ms: u64,
    ) -> Result<(), ControlStreamError> {
        self.coordinator.begin_reconnect_grace(now_ms, duration_ms)
    }

    /// Observes the reconnect window and stops the mixer if it expires.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStreamError`] when the clock observation is invalid.
    pub fn observe_reconnect_grace(
        &mut self,
        now_ms: u64,
    ) -> Result<ReconnectGraceObservation, ControlStreamError> {
        let observation = self.coordinator.observe_reconnect_grace(now_ms)?;
        if matches!(observation, ReconnectGraceObservation::Expired) {
            self.mixer.stop();
            self.output_active = false;
        }
        Ok(observation)
    }

    /// Prepares the output, admits one stream, and commits the start.
    ///
    /// The platform output starts when this is the first stream. On admission
    /// or completion failure the bridge aborts the pending operation and rolls
    /// back any partial work, including stopping an output this call started.
    ///
    /// # Errors
    ///
    /// Returns a control or mixer error without changing active media when the
    /// output cannot start, admission fails closed, or the coordinator rejects
    /// the completion.
    pub fn execute_start<P, const S: usize>(
        &mut self,
        token: ControlOperationToken,
        request: StreamStartRequest,
        counters: Option<Arc<StreamCounters>>,
        pipeline: &mut P,
    ) -> Result<(), ControlToMixerError>
    where
        P: ReceiverPcmProducerInstaller<S>,
    {
        let mut started_output = false;
        if !self.output_active {
            if let Err(error) = self.mixer.start_output(self.selection.clone()) {
                let _ = self.coordinator.abort_pending(token);
                return Err(ControlToMixerError::Mixer(error));
            }
            self.output_active = true;
            started_output = true;
        }
        if let Err(error) =
            self.mixer
                .start_stream(request, self.source_queue_capacity, counters, pipeline)
        {
            if started_output {
                self.mixer.stop();
                self.output_active = false;
            }
            let _ = self.coordinator.abort_pending(token);
            return Err(ControlToMixerError::Mixer(error));
        }
        if let Err(error) = self.coordinator.complete_start(token, request) {
            let _ = self.mixer.remove_stream(request.identity);
            if started_output {
                self.mixer.stop();
                self.output_active = false;
            }
            // A rejected completion is a dead operation: never leave a dangling
            // pending command that could block a later accept after recovery.
            let _ = self.coordinator.abort_pending(token);
            return Err(ControlToMixerError::Control(error));
        }
        Ok(())
    }

    /// Rotates one stream to a newer key epoch and commits the rotation.
    ///
    /// The mixer keeps the stream's slot, gain, and counters; the caller is
    /// responsible for rotating the matching decode pipeline. On mixer failure
    /// the pending operation is aborted without changing active media. When the
    /// coordinator rejects the completion the rotated identity is rolled back
    /// and the pending operation is aborted, so control and mixer stay in
    /// agreement.
    ///
    /// # Errors
    ///
    /// Returns a control or mixer error without changing active media when the
    /// mixer rejects the rotation or the coordinator rejects the completion.
    pub fn execute_rotation(
        &mut self,
        token: ControlOperationToken,
        previous: MediaStreamIdentity,
        next: StreamStartRequest,
    ) -> Result<(), ControlToMixerError> {
        if let Err(error) = self.mixer.rotate_stream(previous, next) {
            let _ = self.coordinator.abort_pending(token);
            return Err(ControlToMixerError::Mixer(error));
        }
        if let Err(error) = self.coordinator.complete_rotation(token, previous, next) {
            // The coordinator rejected the completion, for example because the
            // output faulted or the session retired between accept and execute.
            // Rotate the mixer back to the previous identity and clear the dead
            // pending operation so the control and mixer domains keep the same
            // stream and recovery can accept fresh commands.
            let _ = self.mixer.rotate_stream(
                next.identity,
                StreamStartRequest {
                    identity: previous,
                    ..next
                },
            );
            let _ = self.coordinator.abort_pending(token);
            return Err(ControlToMixerError::Control(error));
        }
        Ok(())
    }

    /// Removes one stream and commits the stop.
    ///
    /// The control domain commits the stop before the mixer removes the
    /// stream. A removal cannot be rolled back at the mixer without the
    /// stream's decode pipeline, so a rejected completion (for example an
    /// output fault or connection loss between accept and execute) must fail
    /// closed with both domains unchanged. When the last active stream stops,
    /// the platform output is stopped and the lifecycle returns to `Ready`.
    ///
    /// # Errors
    ///
    /// Returns a control error without changing active media when the
    /// coordinator rejects the completion, or a mixer error when the removal
    /// fails for a reason other than the identity being already absent.
    pub fn execute_stop(
        &mut self,
        token: ControlOperationToken,
        identity: MediaStreamIdentity,
    ) -> Result<(), ControlToMixerError> {
        self.coordinator
            .complete_stop(token, identity)
            .map_err(ControlToMixerError::Control)?;
        match self.mixer.remove_stream(identity) {
            Ok(()) | Err(MixerStreamSessionError::UnknownStream(_)) => {
                // The control domain committed the removal. An unknown identity
                // is already absent from the mixer, so the stop is already
                // effective in both domains.
            }
            Err(error) => return Err(ControlToMixerError::Mixer(error)),
        }
        if self.coordinator.stream_count() == 0 {
            self.mixer.stop();
            self.output_active = false;
        }
        Ok(())
    }

    /// Detects a platform fault and retires the membership boundary.
    ///
    /// On a fault the coordinator moves to `OutputUnavailable` so
    /// [`Self::recover_output`] can recreate the source.
    ///
    /// # Errors
    ///
    /// Returns a control error when the lifecycle cannot acknowledge the lost
    /// output from its current state.
    pub fn monitor(&mut self) -> Result<Option<OutputHealth>, ControlToMixerError> {
        let fault = self.mixer.monitor();
        if let Some(health) = fault {
            self.coordinator
                .acknowledge_platform_event(SessionEvent::OutputLost)
                .map_err(ControlToMixerError::Control)?;
            self.output_active = false;
            Ok(Some(health))
        } else {
            Ok(None)
        }
    }

    /// Recreates the output source and returns persisted streams to re-admit.
    ///
    /// The coordinator returns to `Streaming`; the caller must re-admit each
    /// returned identity through [`Self::recover_stream`].
    ///
    /// # Errors
    ///
    /// Returns a control or mixer error when recreation fails or the lifecycle
    /// cannot acknowledge the restored output.
    pub fn recover_output(&mut self) -> Result<Vec<MediaStreamIdentity>, ControlToMixerError> {
        let identities = self
            .mixer
            .recover_output()
            .map_err(ControlToMixerError::Mixer)?;
        self.coordinator
            .acknowledge_platform_event(SessionEvent::OutputRestored)
            .map_err(ControlToMixerError::Control)?;
        self.output_active = true;
        Ok(identities)
    }

    /// Re-admits one persisted stream against the recovered output.
    ///
    /// # Errors
    ///
    /// Returns a mixer error when the output is not active, the identity is not
    /// persisted, or admission fails closed.
    pub fn recover_stream<P, const S: usize>(
        &mut self,
        identity: MediaStreamIdentity,
        pipeline: &mut P,
    ) -> Result<StartedMixerStream, ControlToMixerError>
    where
        P: ReceiverPcmProducerInstaller<S>,
    {
        self.mixer
            .recover_stream(identity, pipeline)
            .map_err(ControlToMixerError::Mixer)
    }

    /// Fills an allowlisted diagnostic export snapshot from live bridge state.
    ///
    /// The lifecycle phase maps onto the closed export phase set
    /// (ADR 0011), the
    /// platform output format is included only while the output is active, and
    /// one redacted metrics snapshot per active mixer stream is written into
    /// the caller-provided buffer in slot order. The caller owns serialization
    /// and retention; `sonimesh_diagnostics::build_export_bundle` remains the
    /// single bundle writer.
    #[must_use]
    pub fn export_snapshot<'a>(
        &'a self,
        software_version: &'a str,
        endpoint: Option<SocketAddr>,
        buffer_policy: Option<&'a str>,
        metrics: &'a mut Vec<StreamMetricsSnapshot>,
    ) -> ExportSessionSnapshot<'a> {
        metrics.clear();
        metrics.extend(self.mixer.stream_metrics());
        ExportSessionSnapshot {
            software_version,
            endpoint,
            platform_format: self.output_active.then_some(ExportPlatformFormat {
                sample_rate_hz: self.selection.config.sample_rate_hz,
                channels: self.selection.config.channels,
            }),
            buffer_policy,
            session_phase: Some(export_session_phase(self.state().phase())),
            streams: metrics,
        }
    }
}

/// Maps the app lifecycle phase onto the closed export phase set.
///
/// The current phase sets match one-to-one; a future phase outside the export
/// schema must be omitted rather than guessed so older bundles never carry an
/// unknown token.
const fn export_session_phase(phase: SessionPhase) -> ExportSessionPhase {
    match phase {
        SessionPhase::Idle => ExportSessionPhase::Idle,
        SessionPhase::Pairing => ExportSessionPhase::Pairing,
        SessionPhase::Ready => ExportSessionPhase::Ready,
        SessionPhase::Streaming => ExportSessionPhase::Streaming,
        SessionPhase::Reconnecting => ExportSessionPhase::Reconnecting,
        SessionPhase::OutputUnavailable => ExportSessionPhase::OutputUnavailable,
    }
}

/// Presents an already-read output snapshot at the snapshot source boundary.
///
/// The bridge reads its mixer output lifecycle once and then feeds the pairing
/// owner's projection, so the desktop snapshot cannot re-enter the bridge while
/// the coordinator is being observed.
struct OwnedSnapshot(OutputSnapshot);

impl OutputSnapshotSource for OwnedSnapshot {
    fn snapshot(&self) -> OutputSnapshot {
        self.0
    }
}

impl<O, const SLOTS: usize, const OUTPUT_SAMPLES_PER_BLOCK: usize> OutputSnapshotSource
    for ControlToMixerBridge<O, SLOTS, OUTPUT_SAMPLES_PER_BLOCK>
where
    O: AudioOutput,
{
    fn snapshot(&self) -> OutputSnapshot {
        self.mixer.snapshot()
    }
}
