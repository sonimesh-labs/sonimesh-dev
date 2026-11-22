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

use sonimesh_audio::{MAX_MIXER_COMMAND_QUEUE, MAX_PCM_QUEUE_BLOCKS, RealtimeQueueConfigError};
use sonimesh_diagnostics::{
    ExportPlatformFormat, ExportSessionPhase, ExportSessionSnapshot, StreamCounters,
    StreamMetricsSnapshot,
};
use sonimesh_platform::{AudioOutput, OutputDeviceId, OutputDeviceInfo, OutputError, OutputHealth};
use sonimesh_protocol::{Capability, Envelope};

use crate::{
    ControlOperationToken, ControlSessionToken, ControlStreamAction, ControlStreamCoordinator,
    ControlStreamError, DesktopControlSnapshot, MediaStreamIdentity, MixerAdmissionError,
    MixerStreamSession, MixerStreamSessionError, OutputSelection, OutputSnapshot,
    OutputSnapshotSource, PairingInvitationController, PairingInvitationError,
    ReceiverPcmProducerInstaller, ReconnectGraceError, ReconnectGraceObservation, SessionEvent,
    SessionPhase, SessionState, StartedMixerStream, StreamStartRequest,
};

/// Control or mixer failure surfaced by the bridge.
#[derive(Debug)]
pub enum ControlToMixerError {
    /// The control coordinator rejected the operation.
    Control(ControlStreamError),
    /// The mixer session rejected the operation.
    Mixer(MixerStreamSessionError),
    /// The requested local gain was outside the bounded control range.
    Gain(crate::GainControlError),
    /// Output selection cannot change while the platform stream is active.
    OutputSelectionActive,
}

impl core::fmt::Display for ControlToMixerError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Control(error) => write!(formatter, "control-to-mixer control failure: {error}"),
            Self::Mixer(error) => write!(formatter, "control-to-mixer mixer failure: {error}"),
            Self::Gain(error) => write!(formatter, "control-to-mixer gain failure: {error}"),
            Self::OutputSelectionActive => {
                formatter.write_str("control-to-mixer output selection is active")
            }
        }
    }
}

impl std::error::Error for ControlToMixerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Control(error) => Some(error),
            Self::Mixer(error) => Some(error),
            Self::Gain(error) => Some(error),
            Self::OutputSelectionActive => None,
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
    /// bridge observes reconnect grace before reading its mixer state, so a
    /// snapshot-driven expiry or clock regression stops the output and retires
    /// stale stream records before projection. The pairing owner then applies
    /// its monotonic clock, and the output lifecycle is read from this bridge's
    /// own mixer session, so a caller cannot present an active invitation or
    /// fabricated output state. The returned value excludes pairing
    /// identifiers, proofs, addresses, session material, and audio frames.
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
        // The coordinator snapshot path also observes reconnect grace, but the
        // bridge owns the companion mixer lifetime. Observe through the bridge
        // first so an expiry or clock regression retires the output before its
        // low-rate state is read for this projection. If this observation
        // retires the session, the coordinator's later same-timestamp
        // observation is idle and cannot revive it.
        let _ = self.observe_reconnect_grace(now_ms);
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

    /// Publishes one stream's bounded local gain target.
    ///
    /// This is a control-domain operation. The returned handle is shared with
    /// the preallocated realtime render source, so the callback observes the
    /// new target without receiving a command or crossing the UI boundary.
    ///
    /// # Errors
    ///
    /// Returns a mixer error for an unknown stream and a gain error for an
    /// invalid target.
    pub fn set_stream_gain(
        &self,
        identity: MediaStreamIdentity,
        gain: f32,
    ) -> Result<(), ControlToMixerError> {
        self.mixer
            .gain(identity)
            .map_err(ControlToMixerError::Mixer)?
            .set_gain(gain)
            .map_err(ControlToMixerError::Gain)
    }

    /// Publishes one stream's local mute flag.
    ///
    /// # Errors
    ///
    /// Returns a mixer error when the stream identity is not admitted.
    pub fn set_stream_muted(
        &self,
        identity: MediaStreamIdentity,
        muted: bool,
    ) -> Result<(), ControlToMixerError> {
        self.mixer
            .gain(identity)
            .map_err(ControlToMixerError::Mixer)?
            .set_muted(muted);
        Ok(())
    }

    /// Returns the local gain/mute state when exactly one stream is admitted.
    #[must_use]
    pub fn single_stream_gain(&self) -> Option<crate::GainControlState> {
        self.mixer.single_stream_gain()
    }

    /// Enumerates output endpoints outside the realtime callback.
    ///
    /// # Errors
    ///
    /// Returns the platform enumeration error without changing bridge state.
    pub fn output_devices(&self) -> Result<Vec<OutputDeviceInfo>, OutputError> {
        self.mixer.output().devices()
    }

    /// Returns the currently retained output device, or `None` for the default role.
    #[must_use]
    pub fn selected_output_device(&self) -> Option<OutputDeviceId> {
        self.selection.device.clone()
    }

    /// Retains a new output device for the next output lifetime.
    ///
    /// The current output is never stopped or recreated by this setter. A
    /// caller must select the device while stopped, then start or recover the
    /// output through the normal lifecycle path.
    ///
    /// # Errors
    ///
    /// Returns [`ControlToMixerError::OutputSelectionActive`] when an output
    /// stream is currently active.
    pub fn set_output_device(
        &mut self,
        device: Option<OutputDeviceId>,
    ) -> Result<(), ControlToMixerError> {
        if self.output_active {
            return Err(ControlToMixerError::OutputSelectionActive);
        }
        self.selection.device = device;
        Ok(())
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
    /// Reconnect and output-loss events that invalidate stream state also retire
    /// the mixer. Output restoration must instead use [`Self::recover_output`],
    /// which recreates a fresh source and preserves the coordinator/mixer
    /// stream-membership transaction.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStreamError`] when the current state rejects the event
    /// or when a caller attempts to bypass [`Self::recover_output`] for output
    /// restoration.
    pub fn acknowledge_platform_event(
        &mut self,
        event: SessionEvent,
    ) -> Result<(), ControlStreamError> {
        if event == SessionEvent::OutputRestored {
            return Err(ControlStreamError::UnsupportedLifecycleEvent(event));
        }
        self.coordinator.acknowledge_platform_event(event)?;
        match event {
            SessionEvent::OutputLost => {
                // Retire the realtime membership boundary for direct platform
                // notifications as well as the monitor() path. The coordinator
                // now reports the output as unavailable, so leaving the mixer
                // active would expose stale producers and contradict the
                // control-domain lifecycle.
                self.mixer.retire_output_preserving_streams();
                self.output_active = false;
            }
            SessionEvent::Reconnected => {
                self.mixer.stop();
                self.output_active = false;
            }
            SessionEvent::OutputRestored => {
                return Err(ControlStreamError::UnsupportedLifecycleEvent(event));
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
        let observation = self.coordinator.observe_reconnect_grace(now_ms);
        // The coordinator force-expires the session on a clock regression as
        // well as on deadline expiry, so the mixer must stop in both cases:
        // an idle lifecycle with a live stream or output would let the
        // snapshot contradict the coordinator.
        let expired = matches!(
            observation,
            Ok(ReconnectGraceObservation::Expired)
                | Err(ControlStreamError::ReconnectGrace(
                    ReconnectGraceError::TimeRegressed,
                )),
        );
        if expired {
            self.mixer.stop();
            self.output_active = false;
        }
        observation
    }

    /// Preflights bounded callback-command capacity for an operation and its rollback.
    ///
    /// The control domain is the only normal membership publisher, while the
    /// realtime callback only drains commands. Reserving before committing an
    /// operation ensures that a failed coordinator completion can publish its
    /// compensating remove instead of leaving control and mixer membership out
    /// of sync.
    fn ensure_membership_command_capacity(
        &self,
        required_commands: usize,
    ) -> Result<(), ControlToMixerError> {
        if required_commands > MAX_MIXER_COMMAND_QUEUE
            || self
                .mixer
                .pending_commands()
                .saturating_add(required_commands)
                > MAX_MIXER_COMMAND_QUEUE
        {
            return Err(ControlToMixerError::Mixer(
                MixerStreamSessionError::Membership(MixerAdmissionError::CommandQueueFull),
            ));
        }
        Ok(())
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
        // Start publishes one insert. If control completion later rejects the
        // operation, rollback must publish a second remove, so preflight both
        // queue slots before changing either domain.
        if let Err(error) = self.ensure_membership_command_capacity(2) {
            let _ = self.coordinator.abort_pending(token);
            return Err(error);
        }

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
        // Stop publishes one remove when the output membership is active. Do
        // not commit the control stop unless that bounded command can be
        // published; otherwise the coordinator would retire a stream the
        // mixer was still rendering.
        if self.mixer.removal_requires_membership_command(identity) {
            if let Err(error) = self.ensure_membership_command_capacity(1) {
                let _ = self.coordinator.abort_pending(token);
                return Err(error);
            }
        }

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
    /// On a fault the mixer stops the output and the coordinator moves to
    /// `OutputUnavailable` so [`Self::recover_output`] can recreate the
    /// source.
    ///
    /// # Errors
    ///
    /// Returns a control error when the lifecycle cannot acknowledge the lost
    /// output from its current state (for example during reconnect grace,
    /// where `OutputLost` is invalid). The bridge output is still marked down
    /// in that case, because the mixer already stopped the output.
    pub fn monitor(&mut self) -> Result<Option<OutputHealth>, ControlToMixerError> {
        let fault = self.mixer.monitor();
        if let Some(health) = fault {
            // The mixer stopped the output and retired its membership. Mark
            // the bridge output down before acknowledging, so the snapshot
            // never claims an active output even when the lifecycle cannot
            // represent the fault.
            self.output_active = false;
            self.coordinator
                .acknowledge_platform_event(SessionEvent::OutputLost)
                .map_err(ControlToMixerError::Control)?;
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
    /// The coordinator acknowledges the restoration before the mixer recreates
    /// its backend, because recreation cannot be rolled back: a rejected
    /// restoration must fail closed with both domains unchanged. When the
    /// mixer cannot recreate the backend after the coordinator committed, the
    /// coordinator is rolled back to `OutputUnavailable` so both domains keep
    /// agreeing that the output is still down and recovery can be retried.
    ///
    /// # Errors
    ///
    /// Returns a control error without changing either domain when the
    /// lifecycle cannot acknowledge the restored output, or a mixer error with
    /// the coordinator rolled back to `OutputUnavailable` when recreation
    /// fails.
    pub fn recover_output(&mut self) -> Result<Vec<MediaStreamIdentity>, ControlToMixerError> {
        self.coordinator
            .acknowledge_platform_event(SessionEvent::OutputRestored)
            .map_err(ControlToMixerError::Control)?;
        let identities = match self
            .mixer
            .recover_output()
            .map_err(ControlToMixerError::Mixer)
        {
            Ok(identities) => identities,
            Err(error) => {
                // The coordinator already returned to Streaming. Roll it back
                // to OutputUnavailable (valid from Streaming) so both domains
                // agree the output is not restored yet.
                let _ = self
                    .coordinator
                    .acknowledge_platform_event(SessionEvent::OutputLost);
                return Err(error);
            }
        };
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

#[cfg(test)]
mod tests {
    use sonimesh_audio::{
        MAX_MIXER_COMMAND_QUEUE, MAX_PCM_QUEUE_BLOCKS, RealtimePcmProducer,
        RealtimeQueueConfigError,
    };
    use sonimesh_platform::{
        AudioOutput, OutputConfig, OutputDeviceId, OutputDeviceInfo, OutputError, OutputHealth,
        RenderSource, SampleFormat,
    };
    use sonimesh_protocol::{
        CONTROL_PROTOCOL_VERSION, Capability, Envelope, Hello, StartStream, StopStream, envelope,
    };

    use super::{ControlToMixerBridge, ControlToMixerError};
    use crate::{
        ControlOperationToken, ControlSessionToken, ControlStreamAction, ControlStreamError,
        MediaStreamIdentity, MixerAdmissionError, MixerOutputLifecycleError,
        MixerStreamSessionError, OutputSelection, ReceiverPcmProducerInstaller,
        ReconnectGraceObservation, SessionEvent, SessionPhase, SessionState, StreamStartRequest,
    };

    const SLOTS: usize = 2;
    const OUTPUT_SAMPLES: usize = 256;
    const SOURCE_SAMPLES: usize = 480;

    struct FakeOutput {
        started: bool,
        stopped: bool,
        health: OutputHealth,
        fail_start: bool,
    }

    impl Default for FakeOutput {
        fn default() -> Self {
            Self {
                started: false,
                stopped: false,
                health: OutputHealth::Healthy,
                fail_start: false,
            }
        }
    }

    impl AudioOutput for FakeOutput {
        fn devices(&self) -> Result<Vec<OutputDeviceInfo>, OutputError> {
            Ok(Vec::new())
        }

        fn start(
            &mut self,
            _device: Option<&OutputDeviceId>,
            _config: OutputConfig,
            _source: Box<dyn RenderSource>,
        ) -> Result<(), OutputError> {
            if self.fail_start {
                return Err(OutputError::DeviceUnavailable);
            }
            self.started = true;
            self.stopped = false;
            Ok(())
        }

        fn stop(&mut self) {
            self.started = false;
            self.stopped = true;
        }

        fn health(&self) -> OutputHealth {
            self.health
        }
    }

    struct FakePipeline<const S: usize> {
        replaced: usize,
        producer: Option<RealtimePcmProducer<S>>,
    }

    impl<const S: usize> Default for FakePipeline<S> {
        fn default() -> Self {
            Self {
                replaced: 0,
                producer: None,
            }
        }
    }

    impl<const S: usize> ReceiverPcmProducerInstaller<S> for FakePipeline<S> {
        fn replace_pcm_producer(&mut self, producer: RealtimePcmProducer<S>) {
            self.replaced += 1;
            self.producer = Some(producer);
        }
    }

    fn selection(device: Option<OutputDeviceId>) -> OutputSelection {
        OutputSelection {
            device,
            config: OutputConfig {
                sample_rate_hz: 48_000,
                channels: 2,
                sample_format: SampleFormat::Float32,
                requested_callback_frames: Some(480),
                maximum_callback_frames: 1_024,
            },
        }
    }

    fn identity(stream_id: u32) -> MediaStreamIdentity {
        MediaStreamIdentity {
            session_id: 0x0102_0304,
            stream_id,
            key_epoch: 0,
        }
    }

    fn request(identity: MediaStreamIdentity) -> StreamStartRequest {
        StreamStartRequest {
            identity,
            sample_rate_hz: 48_000,
            channels: 1,
            frame_duration_ms: 10,
        }
    }

    fn bridge(output: FakeOutput) -> ControlToMixerBridge<FakeOutput, SLOTS, OUTPUT_SAMPLES> {
        ControlToMixerBridge::new(
            SessionState::READY,
            &[Capability::FrameDuration10Ms, Capability::FrameDuration20Ms],
            output,
            selection(None),
            8,
            8,
        )
        .expect("fixed test bridge is valid")
    }

    fn hello_envelope() -> Envelope {
        Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 7,
            payload: Some(envelope::Payload::Hello(Hello {
                supported_versions: vec![CONTROL_PROTOCOL_VERSION],
                capabilities: vec![
                    Capability::FrameDuration10Ms as i32,
                    Capability::FrameDuration20Ms as i32,
                ],
                display_name: "test sender".to_owned(),
            })),
        }
    }

    fn start_envelope(request: StreamStartRequest) -> Envelope {
        Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 11,
            payload: Some(envelope::Payload::StartStream(StartStream {
                media_session_id: request.identity.session_id,
                stream_id: request.identity.stream_id,
                sample_rate_hz: request.sample_rate_hz,
                channels: request.channels,
                frame_duration_ms: request.frame_duration_ms,
                media_key_epoch: u32::from(request.identity.key_epoch),
            })),
        }
    }

    fn stop_envelope(identity: MediaStreamIdentity) -> Envelope {
        Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 12,
            payload: Some(envelope::Payload::StopStream(StopStream {
                media_session_id: identity.session_id,
                stream_id: identity.stream_id,
                media_key_epoch: u32::from(identity.key_epoch),
            })),
        }
    }

    fn start_one(
        bridge: &mut ControlToMixerBridge<FakeOutput, SLOTS, OUTPUT_SAMPLES>,
        session: ControlSessionToken,
        identity: MediaStreamIdentity,
        pipeline: &mut FakePipeline<SOURCE_SAMPLES>,
    ) {
        let request = request(identity);
        match bridge.accept(session, &start_envelope(request)) {
            Ok(ControlStreamAction::Start { token, request }) => {
                bridge
                    .execute_start(token, request, None, pipeline)
                    .unwrap();
            }
            other => panic!("expected a start action, got {other:?}"),
        }
    }

    fn stop_one(
        bridge: &mut ControlToMixerBridge<FakeOutput, SLOTS, OUTPUT_SAMPLES>,
        session: ControlSessionToken,
        identity: MediaStreamIdentity,
    ) {
        match bridge.accept(session, &stop_envelope(identity)) {
            Ok(ControlStreamAction::Stop { token, identity }) => {
                bridge.execute_stop(token, identity).unwrap();
            }
            other => panic!("expected a stop action, got {other:?}"),
        }
    }

    fn prepare_start(
        bridge: &mut ControlToMixerBridge<FakeOutput, SLOTS, OUTPUT_SAMPLES>,
        session: ControlSessionToken,
        identity: MediaStreamIdentity,
    ) -> (ControlOperationToken, StreamStartRequest) {
        let request = request(identity);
        match bridge.accept(session, &start_envelope(request)) {
            Ok(ControlStreamAction::Start { token, request }) => (token, request),
            other => panic!("expected a start action, got {other:?}"),
        }
    }

    #[test]
    fn new_rejects_zero_or_oversized_source_queue_capacity() {
        let capabilities = [Capability::FrameDuration10Ms, Capability::FrameDuration20Ms];
        assert!(matches!(
            ControlToMixerBridge::<FakeOutput, SLOTS, OUTPUT_SAMPLES>::new(
                SessionState::READY,
                &capabilities,
                FakeOutput::default(),
                selection(None),
                8,
                0,
            ),
            Err(ControlToMixerError::Mixer(
                MixerStreamSessionError::Membership(MixerAdmissionError::Queue(
                    RealtimeQueueConfigError::ZeroCapacity
                ))
            ))
        ));
        assert!(matches!(
            ControlToMixerBridge::<FakeOutput, SLOTS, OUTPUT_SAMPLES>::new(
                SessionState::READY,
                &capabilities,
                FakeOutput::default(),
                selection(None),
                8,
                MAX_PCM_QUEUE_BLOCKS + 1,
            ),
            Err(ControlToMixerError::Mixer(
                MixerStreamSessionError::Membership(MixerAdmissionError::Queue(
                    RealtimeQueueConfigError::CapacityTooLarge
                ))
            ))
        ));
    }

    #[test]
    fn new_rejects_zero_or_oversized_mixer_queue_capacity() {
        let capabilities = [Capability::FrameDuration10Ms, Capability::FrameDuration20Ms];
        assert!(matches!(
            ControlToMixerBridge::<FakeOutput, SLOTS, OUTPUT_SAMPLES>::new(
                SessionState::READY,
                &capabilities,
                FakeOutput::default(),
                selection(None),
                0,
                8,
            ),
            Err(ControlToMixerError::Mixer(
                MixerStreamSessionError::Membership(MixerAdmissionError::Queue(
                    RealtimeQueueConfigError::ZeroCapacity
                ))
            ))
        ));
        assert!(matches!(
            ControlToMixerBridge::<FakeOutput, SLOTS, OUTPUT_SAMPLES>::new(
                SessionState::READY,
                &capabilities,
                FakeOutput::default(),
                selection(None),
                MAX_PCM_QUEUE_BLOCKS + 1,
                8,
            ),
            Err(ControlToMixerError::Mixer(
                MixerStreamSessionError::Membership(MixerAdmissionError::Queue(
                    RealtimeQueueConfigError::CapacityTooLarge
                ))
            ))
        ));
    }

    #[test]
    fn new_rejects_an_oversized_local_capability_policy() {
        let capabilities = [Capability::Unspecified; 17];
        assert!(matches!(
            ControlToMixerBridge::<FakeOutput, SLOTS, OUTPUT_SAMPLES>::new(
                SessionState::READY,
                &capabilities,
                FakeOutput::default(),
                selection(None),
                8,
                8,
            ),
            Err(ControlToMixerError::Control(
                ControlStreamError::LocalCapabilityPolicyTooLarge
            ))
        ));
    }

    #[test]
    fn snapshot_reflects_idle_and_streaming_bridge_state() {
        let mut bridge = bridge(FakeOutput::default());
        let idle = bridge.snapshot();
        assert_eq!(idle.session(), SessionPhase::Ready);
        assert!(!idle.output_active());
        assert_eq!(idle.active_streams(), 0);
        assert_eq!(idle.pending_mixer_commands(), 0);

        let session = bridge.install_authenticated_control_session().unwrap();
        bridge.accept(session, &hello_envelope()).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        start_one(&mut bridge, session, identity(1), &mut pipeline);

        let streaming = bridge.snapshot();
        assert_eq!(streaming.session(), SessionPhase::Streaming);
        assert!(streaming.output_active());
        assert_eq!(streaming.active_streams(), 1);
        assert!(streaming.pending_mixer_commands() >= 1);
        assert!(streaming.output().stream_active());
    }

    #[test]
    fn stop_keeps_control_and_mixer_streams_when_membership_queue_is_full() {
        let mut bridge = bridge(FakeOutput::default());
        let session = bridge.install_authenticated_control_session().unwrap();
        bridge.accept(session, &hello_envelope()).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        start_one(&mut bridge, session, identity(1), &mut pipeline);

        // Fill the callback queue through the same mixer membership boundary.
        // The active coordinator stream remains in slot 0 while the spare slot
        // is repeatedly inserted and removed; the final insert makes the queue
        // exactly full without changing coordinator-owned stream membership.
        for stream_id in 2..=8 {
            let stream = identity(stream_id);
            bridge
                .mixer_mut()
                .start_stream(request(stream), 8, None, &mut pipeline)
                .unwrap();
            bridge.mixer_mut().remove_stream(stream).unwrap();
        }
        bridge
            .mixer_mut()
            .start_stream(request(identity(9)), 8, None, &mut pipeline)
            .unwrap();
        assert_eq!(bridge.mixer().pending_commands(), MAX_MIXER_COMMAND_QUEUE);

        let token = match bridge.accept(session, &stop_envelope(identity(1))) {
            Ok(ControlStreamAction::Stop { token, .. }) => token,
            other => panic!("expected a stop action, got {other:?}"),
        };
        assert!(matches!(
            bridge.execute_stop(token, identity(1)),
            Err(ControlToMixerError::Mixer(
                MixerStreamSessionError::Membership(MixerAdmissionError::CommandQueueFull)
            ))
        ));

        assert_eq!(bridge.state().phase(), SessionPhase::Streaming);
        assert_eq!(bridge.coordinator().stream_count(), 1);
        assert_eq!(bridge.mixer().stream_count(), 2);
        assert!(bridge.output_active());
        assert_eq!(bridge.mixer().pending_commands(), MAX_MIXER_COMMAND_QUEUE);
    }

    #[test]
    fn start_aborts_pending_operation_when_membership_queue_cannot_cover_rollback() {
        let mut bridge = bridge(FakeOutput::default());
        let session = bridge.install_authenticated_control_session().unwrap();
        bridge.accept(session, &hello_envelope()).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        start_one(&mut bridge, session, identity(1), &mut pipeline);

        // Leave the coordinator's first stream active while filling the
        // bounded callback queue through the mixer-only membership boundary.
        for stream_id in 2..=8 {
            let stream = identity(stream_id);
            bridge
                .mixer_mut()
                .start_stream(request(stream), 8, None, &mut pipeline)
                .unwrap();
            bridge.mixer_mut().remove_stream(stream).unwrap();
        }
        bridge
            .mixer_mut()
            .start_stream(request(identity(9)), 8, None, &mut pipeline)
            .unwrap();
        assert_eq!(bridge.mixer().pending_commands(), MAX_MIXER_COMMAND_QUEUE);

        let (token, request) = prepare_start(&mut bridge, session, identity(10));
        assert!(matches!(
            bridge.execute_start(token, request, None, &mut pipeline),
            Err(ControlToMixerError::Mixer(
                MixerStreamSessionError::Membership(MixerAdmissionError::CommandQueueFull)
            ))
        ));

        // The preflight must abort the control operation before touching the
        // output or mixer, leaving both domains unchanged and retryable.
        assert_eq!(bridge.state().phase(), SessionPhase::Streaming);
        assert_eq!(bridge.coordinator().stream_count(), 1);
        assert_eq!(bridge.mixer().stream_count(), 2);
        assert!(bridge.output_active());
        assert_eq!(bridge.mixer().pending_commands(), MAX_MIXER_COMMAND_QUEUE);
    }

    #[test]
    fn lifecycle_starts_output_with_first_stream_and_stops_with_last() {
        let mut bridge = bridge(FakeOutput::default());
        let session = bridge.install_authenticated_control_session().unwrap();
        assert!(matches!(
            bridge.accept(session, &hello_envelope()),
            Ok(ControlStreamAction::Negotiated(_))
        ));
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();

        start_one(&mut bridge, session, identity(1), &mut pipeline);
        assert_eq!(bridge.state().phase(), SessionPhase::Streaming);
        assert!(bridge.output_active());
        assert_eq!(bridge.mixer().stream_count(), 1);
        assert_eq!(pipeline.replaced, 1);
        assert!(bridge.mixer().output().started);

        stop_one(&mut bridge, session, identity(1));
        assert_eq!(bridge.state().phase(), SessionPhase::Ready);
        assert!(!bridge.output_active());
        assert_eq!(bridge.mixer().stream_count(), 0);
        assert!(bridge.mixer().output().stopped);
    }

    #[test]
    fn first_stream_output_failure_rolls_back_without_dangling_pending() {
        let output = FakeOutput {
            fail_start: true,
            ..Default::default()
        };
        let mut bridge = bridge(output);
        let session = bridge.install_authenticated_control_session().unwrap();
        bridge.accept(session, &hello_envelope()).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        let (token, request) = prepare_start(&mut bridge, session, identity(1));

        assert!(matches!(
            bridge.execute_start(token, request, None, &mut pipeline),
            Err(ControlToMixerError::Mixer(MixerStreamSessionError::Output(
                MixerOutputLifecycleError::Output(OutputError::DeviceUnavailable)
            )))
        ));
        assert!(!bridge.output_active());
        assert_eq!(bridge.state().phase(), SessionPhase::Ready);
        assert_eq!(bridge.mixer().stream_count(), 0);
        assert!(!bridge.mixer().output().started);

        // The aborted pending operation must not block a later admission.
        bridge.mixer_mut().output_mut().fail_start = false;
        start_one(&mut bridge, session, identity(1), &mut pipeline);
        assert_eq!(bridge.state().phase(), SessionPhase::Streaming);
        assert!(bridge.output_active());
        assert_eq!(bridge.mixer().stream_count(), 1);
    }

    #[test]
    fn output_stays_active_while_any_stream_runs() {
        let mut bridge = bridge(FakeOutput::default());
        let session = bridge.install_authenticated_control_session().unwrap();
        bridge.accept(session, &hello_envelope()).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();

        start_one(&mut bridge, session, identity(1), &mut pipeline);
        start_one(&mut bridge, session, identity(2), &mut pipeline);
        assert_eq!(bridge.mixer().stream_count(), 2);
        assert!(bridge.output_active());

        stop_one(&mut bridge, session, identity(1));
        assert_eq!(bridge.mixer().stream_count(), 1);
        assert!(bridge.output_active());
        assert!(!bridge.mixer().output().stopped);

        stop_one(&mut bridge, session, identity(2));
        assert_eq!(bridge.mixer().stream_count(), 0);
        assert!(!bridge.output_active());
        assert!(bridge.mixer().output().stopped);
        assert_eq!(bridge.state().phase(), SessionPhase::Ready);
    }

    #[test]
    fn monitor_fault_retires_membership_and_recovery_readmits_streams() {
        let output = FakeOutput {
            health: OutputHealth::StreamError,
            ..Default::default()
        };
        let mut bridge = bridge(output);
        let session = bridge.install_authenticated_control_session().unwrap();
        bridge.accept(session, &hello_envelope()).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        start_one(&mut bridge, session, identity(1), &mut pipeline);
        start_one(&mut bridge, session, identity(2), &mut pipeline);

        assert!(matches!(
            bridge.monitor(),
            Ok(Some(OutputHealth::StreamError))
        ));
        assert!(matches!(bridge.monitor(), Ok(None)));
        assert_eq!(bridge.state().phase(), SessionPhase::OutputUnavailable);
        assert!(!bridge.output_active());
        assert_eq!(bridge.mixer().stream_count(), 2);
        assert!(bridge.mixer().output().stopped);
        assert!(bridge.snapshot().output().fault_reported());

        let identities = bridge.recover_output().unwrap();
        assert_eq!(identities, vec![identity(1), identity(2)]);
        assert_eq!(bridge.state().phase(), SessionPhase::Streaming);
        assert!(bridge.output_active());

        bridge.recover_stream(identity(1), &mut pipeline).unwrap();
        bridge.recover_stream(identity(2), &mut pipeline).unwrap();
        assert_eq!(pipeline.replaced, 4);
        assert_eq!(bridge.mixer().stream_count(), 2);
        assert!(bridge.mixer().output().started);
    }

    #[test]
    fn recovery_failure_rolls_the_coordinator_back_to_output_unavailable() {
        let output = FakeOutput {
            health: OutputHealth::CallbackCapacityExceeded,
            ..Default::default()
        };
        let mut bridge = bridge(output);
        let session = bridge.install_authenticated_control_session().unwrap();
        bridge.accept(session, &hello_envelope()).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        start_one(&mut bridge, session, identity(1), &mut pipeline);
        assert!(matches!(
            bridge.monitor(),
            Ok(Some(OutputHealth::CallbackCapacityExceeded))
        ));
        assert_eq!(bridge.state().phase(), SessionPhase::OutputUnavailable);

        bridge.mixer_mut().output_mut().fail_start = true;
        assert!(matches!(
            bridge.recover_output(),
            Err(ControlToMixerError::Mixer(MixerStreamSessionError::Output(
                MixerOutputLifecycleError::Output(OutputError::DeviceUnavailable)
            )))
        ));
        // Both domains must agree the output is still down so recovery can retry.
        assert_eq!(bridge.state().phase(), SessionPhase::OutputUnavailable);
        assert!(!bridge.output_active());
        assert_eq!(bridge.mixer().stream_count(), 1);
    }

    #[test]
    fn reconnect_grace_expiry_stops_the_mixer_output() {
        let mut bridge = bridge(FakeOutput::default());
        let session = bridge.install_authenticated_control_session().unwrap();
        bridge.accept(session, &hello_envelope()).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        start_one(&mut bridge, session, identity(1), &mut pipeline);

        bridge
            .acknowledge_platform_event(SessionEvent::ConnectionLost)
            .unwrap();
        assert_eq!(bridge.state().phase(), SessionPhase::Reconnecting);
        bridge.begin_reconnect_grace(1_000, 500).unwrap();
        assert!(matches!(
            bridge.observe_reconnect_grace(1_000),
            Ok(ReconnectGraceObservation::InGrace { remaining_ms })
                if remaining_ms == 500
        ));
        assert!(bridge.output_active());

        assert!(matches!(
            bridge.observe_reconnect_grace(1_501),
            Ok(ReconnectGraceObservation::Expired)
        ));
        assert_eq!(bridge.state().phase(), SessionPhase::Idle);
        assert!(!bridge.output_active());
        assert_eq!(bridge.mixer().stream_count(), 0);
        assert!(bridge.mixer().output().stopped);
    }

    #[test]
    fn reconnected_stops_mixer_and_retires_the_control_session() {
        let mut bridge = bridge(FakeOutput::default());
        let session = bridge.install_authenticated_control_session().unwrap();
        bridge.accept(session, &hello_envelope()).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        start_one(&mut bridge, session, identity(1), &mut pipeline);

        bridge
            .acknowledge_platform_event(SessionEvent::ConnectionLost)
            .unwrap();
        bridge
            .acknowledge_platform_event(SessionEvent::Reconnected)
            .unwrap();
        assert_eq!(bridge.state().phase(), SessionPhase::Ready);
        assert!(!bridge.output_active());
        assert_eq!(bridge.mixer().stream_count(), 0);
        assert!(bridge.mixer().output().stopped);
        assert!(bridge.coordinator().negotiation().is_none());
    }

    #[test]
    fn direct_output_restoration_is_rejected_until_bridge_recovery_runs() {
        let mut bridge = bridge(FakeOutput::default());
        let session = bridge.install_authenticated_control_session().unwrap();
        bridge.accept(session, &hello_envelope()).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        start_one(&mut bridge, session, identity(1), &mut pipeline);

        bridge
            .acknowledge_platform_event(SessionEvent::OutputLost)
            .unwrap();
        assert_eq!(bridge.state().phase(), SessionPhase::OutputUnavailable);

        assert!(matches!(
            bridge.acknowledge_platform_event(SessionEvent::OutputRestored),
            Err(ControlStreamError::UnsupportedLifecycleEvent(
                SessionEvent::OutputRestored
            ))
        ));
        assert_eq!(bridge.state().phase(), SessionPhase::OutputUnavailable);
        assert!(!bridge.output_active());
        assert_eq!(bridge.mixer().stream_count(), 1);

        assert_eq!(bridge.recover_output().unwrap(), vec![identity(1)]);
        assert_eq!(bridge.state().phase(), SessionPhase::Streaming);
        assert!(bridge.output_active());
    }

    #[test]
    fn expire_control_session_stops_mixer_and_output() {
        let mut bridge = bridge(FakeOutput::default());
        let session = bridge.install_authenticated_control_session().unwrap();
        bridge.accept(session, &hello_envelope()).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        start_one(&mut bridge, session, identity(1), &mut pipeline);

        bridge.expire_control_session();
        assert_eq!(bridge.state().phase(), SessionPhase::Idle);
        assert!(!bridge.output_active());
        assert_eq!(bridge.mixer().stream_count(), 0);
        assert!(bridge.mixer().output().stopped);
    }

    #[test]
    fn stale_start_token_fails_closed_without_touching_active_media() {
        let mut bridge = bridge(FakeOutput::default());
        let session = bridge.install_authenticated_control_session().unwrap();
        bridge.accept(session, &hello_envelope()).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        start_one(&mut bridge, session, identity(1), &mut pipeline);

        // Prepare a second start, then lose the connection so the coordinator
        // retires the pending operation before the platform executes it.
        let (token, request) = prepare_start(&mut bridge, session, identity(2));
        bridge
            .acknowledge_platform_event(SessionEvent::ConnectionLost)
            .unwrap();

        assert!(matches!(
            bridge.execute_start(token, request, None, &mut pipeline),
            Err(ControlToMixerError::Control(
                ControlStreamError::CompletionMismatch
            ))
        ));
        assert_eq!(bridge.state().phase(), SessionPhase::Reconnecting);
        assert_eq!(bridge.mixer().stream_count(), 1);
        assert!(bridge.output_active());
    }

    #[test]
    fn stale_rotation_token_rotates_the_mixer_back_to_the_previous_identity() {
        let mut bridge = bridge(FakeOutput::default());
        let session = bridge.install_authenticated_control_session().unwrap();
        bridge.accept(session, &hello_envelope()).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        start_one(&mut bridge, session, identity(1), &mut pipeline);

        // Peer rotates stream 1 to a newer key epoch.
        let mut rotated = request(identity(1));
        rotated.identity.key_epoch = 1;
        let rotated = match bridge.accept(session, &start_envelope(rotated)) {
            Ok(ControlStreamAction::Rotate {
                token,
                previous,
                next,
            }) => (token, previous, next),
            other => panic!("expected a rotate action, got {other:?}"),
        };
        let (token, previous, next) = rotated;
        assert_eq!(previous, identity(1));
        assert_eq!(next.identity.key_epoch, 1);

        // The output faults between accept and execute, retiring the pending
        // operation; the platform must not commit the rotation.
        bridge
            .acknowledge_platform_event(SessionEvent::OutputLost)
            .unwrap();
        assert!(matches!(
            bridge.execute_rotation(token, previous, next),
            Err(ControlToMixerError::Control(
                ControlStreamError::CompletionMismatch
            ))
        ));

        assert_eq!(bridge.state().phase(), SessionPhase::OutputUnavailable);
        assert_eq!(bridge.mixer().stream_count(), 1);
        assert!(bridge.mixer().gain(identity(1)).is_ok());
        assert!(matches!(
            bridge.mixer().gain(MediaStreamIdentity {
                key_epoch: 1,
                ..identity(1)
            }),
            Err(MixerStreamSessionError::UnknownStream(_))
        ));
    }

    #[test]
    fn monitor_fault_during_reconnect_still_marks_output_down() {
        let mut bridge = bridge(FakeOutput::default());
        let session = bridge.install_authenticated_control_session().unwrap();
        bridge.accept(session, &hello_envelope()).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        start_one(&mut bridge, session, identity(1), &mut pipeline);
        bridge
            .acknowledge_platform_event(SessionEvent::ConnectionLost)
            .unwrap();

        bridge.mixer_mut().output_mut().health = OutputHealth::StreamError;
        assert!(matches!(
            bridge.monitor(),
            Err(ControlToMixerError::Control(
                ControlStreamError::Transition(_)
            ))
        ));
        // The bridge still marks the output down even when the lifecycle cannot
        // represent the fault during reconnect.
        assert!(!bridge.output_active());
        assert!(bridge.mixer().output().stopped);
        assert_eq!(bridge.state().phase(), SessionPhase::Reconnecting);
    }

    #[test]
    fn error_display_is_actionable() {
        assert_eq!(
            ControlToMixerError::Control(ControlStreamError::CompletionMismatch).to_string(),
            "control-to-mixer control failure: stream lifecycle completion did not match pending state"
        );
        assert_eq!(
            ControlToMixerError::Mixer(MixerStreamSessionError::OutputNotActive).to_string(),
            "control-to-mixer mixer failure: mixer session output is not active"
        );
    }
}
