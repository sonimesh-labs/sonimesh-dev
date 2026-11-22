//! Low-rate desktop command owner independent of Tauri or a web view.
//!
//! This composition layer keeps the UI-facing lifecycle operations together:
//! snapshot polling, pairing lifecycle, local stream controls, and diagnostics.
//! Stream admission and realtime rendering remain owned by
//! [`ControlToMixerBridge`]; this owner only delegates those bounded control
//! operations and never touches the output callback.

use std::net::SocketAddr;

use sonimesh_platform::AudioOutput;

use crate::{
    ControlToMixerBridge, ControlToMixerError, DesktopControlSnapshot, DiagnosticExportAction,
    DiagnosticExportActionError, ExportRetentionPolicy, IssuedDiagnosticExport,
    MediaStreamIdentity, PairingAttemptOutcome, PairingId, PairingInvitationController,
    PairingInvitationError, PairingProofVerification, PairingSource,
};

/// Failure returned by a low-rate desktop command owner.
#[derive(Debug)]
pub enum DesktopControlOwnerError {
    /// Pairing lifecycle or snapshot clock failure.
    Pairing(PairingInvitationError),
    /// Diagnostic export serialization or retention failure.
    Diagnostics(DiagnosticExportActionError),
    /// Stream control failed because the bridge or mixer rejected it.
    Mixer(ControlToMixerError),
}

impl core::fmt::Display for DesktopControlOwnerError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Pairing(error) => write!(formatter, "desktop control pairing failure: {error}"),
            Self::Diagnostics(error) => {
                write!(formatter, "desktop control diagnostics failure: {error}")
            }
            Self::Mixer(error) => write!(formatter, "desktop control mixer failure: {error}"),
        }
    }
}

impl std::error::Error for DesktopControlOwnerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Pairing(error) => Some(error),
            Self::Diagnostics(error) => Some(error),
            Self::Mixer(error) => Some(error),
        }
    }
}

/// Low-rate control owner for one desktop receiver bridge.
pub struct DesktopControlOwner<
    O,
    const SLOTS: usize,
    const OUTPUT_SAMPLES_PER_BLOCK: usize,
    const MAX_SOURCES: usize,
> where
    O: AudioOutput,
{
    bridge: ControlToMixerBridge<O, SLOTS, OUTPUT_SAMPLES_PER_BLOCK>,
    pairing: PairingInvitationController<MAX_SOURCES>,
    diagnostics: DiagnosticExportAction,
}

impl<O, const SLOTS: usize, const OUTPUT_SAMPLES_PER_BLOCK: usize, const MAX_SOURCES: usize>
    DesktopControlOwner<O, SLOTS, OUTPUT_SAMPLES_PER_BLOCK, MAX_SOURCES>
where
    O: AudioOutput,
{
    /// Creates an owner with the default bounded diagnostic retention policy.
    ///
    /// # Errors
    ///
    /// The default policy is valid; this result keeps construction behavior
    /// consistent with [`Self::with_policy`] for shell owners that configure
    /// retention at startup.
    pub fn new(
        bridge: ControlToMixerBridge<O, SLOTS, OUTPUT_SAMPLES_PER_BLOCK>,
    ) -> Result<Self, DesktopControlOwnerError> {
        Self::with_policy(bridge, ExportRetentionPolicy::default())
    }

    /// Creates an owner with an explicit diagnostic retention policy.
    ///
    /// # Errors
    ///
    /// Returns a diagnostics error when the policy has invalid or zero bounds.
    pub fn with_policy(
        bridge: ControlToMixerBridge<O, SLOTS, OUTPUT_SAMPLES_PER_BLOCK>,
        policy: ExportRetentionPolicy,
    ) -> Result<Self, DesktopControlOwnerError> {
        let diagnostics =
            DiagnosticExportAction::new(policy).map_err(DesktopControlOwnerError::Diagnostics)?;
        Ok(Self {
            bridge,
            pairing: PairingInvitationController::new(),
            diagnostics,
        })
    }

    /// Polls synchronized low-rate lifecycle state for the desktop UI.
    ///
    /// # Errors
    ///
    /// Returns a pairing error when the monotonic clock observation fails.
    pub fn snapshot_at(
        &mut self,
        now_ms: u64,
    ) -> Result<DesktopControlSnapshot, DesktopControlOwnerError> {
        self.bridge
            .desktop_snapshot_at(&mut self.pairing, now_ms)
            .map_err(DesktopControlOwnerError::Pairing)
    }

    /// Starts a bounded pairing invitation through the shared lifecycle owner.
    ///
    /// # Errors
    ///
    /// Returns a pairing error when the lifecycle, invitation bounds, or
    /// current session rejects the request.
    pub fn begin_pairing(
        &mut self,
        id: PairingId,
        now_ms: u64,
        lifetime_ms: u64,
        max_attempts_per_source: u8,
    ) -> Result<(), DesktopControlOwnerError> {
        self.bridge
            .coordinator_mut()
            .begin_pairing(
                &mut self.pairing,
                id,
                now_ms,
                lifetime_ms,
                max_attempts_per_source,
            )
            .map_err(DesktopControlOwnerError::Pairing)
    }

    /// Verifies one already-authenticated pairing proof result.
    ///
    /// # Errors
    ///
    /// Returns a pairing error when the invitation or session rejects the
    /// attempt.
    pub fn verify_pairing_proof(
        &mut self,
        now_ms: u64,
        source: PairingSource,
        verify: impl FnOnce() -> PairingProofVerification,
    ) -> Result<PairingAttemptOutcome, DesktopControlOwnerError> {
        self.bridge
            .coordinator_mut()
            .verify_pairing_proof(&mut self.pairing, now_ms, source, verify)
            .map_err(DesktopControlOwnerError::Pairing)
    }

    /// Expires the active pairing invitation.
    ///
    /// # Errors
    ///
    /// Returns a pairing error when no active invitation is retained.
    pub fn expire_pairing(&mut self) -> Result<(), DesktopControlOwnerError> {
        self.bridge
            .coordinator_mut()
            .expire_pairing(&mut self.pairing)
            .map(|_| ())
            .map_err(DesktopControlOwnerError::Pairing)
    }

    /// Revokes the retained invitation or trusted in-memory session.
    ///
    /// # Errors
    ///
    /// Returns a pairing error when no invitation is retained or lifecycle
    /// synchronization fails.
    pub fn revoke_pairing(&mut self) -> Result<(), DesktopControlOwnerError> {
        self.bridge
            .coordinator_mut()
            .revoke_pairing(&mut self.pairing)
            .map(|_| ())
            .map_err(DesktopControlOwnerError::Pairing)
    }

    /// Publishes a stream-local gain target.
    ///
    /// # Errors
    ///
    /// Returns a mixer error when the stream is unknown or the gain is invalid.
    pub fn set_stream_gain(
        &self,
        identity: MediaStreamIdentity,
        gain: f32,
    ) -> Result<(), DesktopControlOwnerError> {
        self.bridge
            .set_stream_gain(identity, gain)
            .map_err(DesktopControlOwnerError::Mixer)
    }

    /// Publishes a stream-local mute flag.
    ///
    /// # Errors
    ///
    /// Returns a mixer error when the stream is unknown.
    pub fn set_stream_muted(
        &self,
        identity: MediaStreamIdentity,
        muted: bool,
    ) -> Result<(), DesktopControlOwnerError> {
        self.bridge
            .set_stream_muted(identity, muted)
            .map_err(DesktopControlOwnerError::Mixer)
    }

    /// Issues one bounded diagnostic export after synchronizing lifecycle clocks.
    ///
    /// # Errors
    ///
    /// Returns a pairing error when clock observation fails, or a diagnostics
    /// error when serialization or retention rejects the export.
    pub fn export_diagnostics(
        &mut self,
        now_ms: u64,
        software_version: &str,
        endpoint: Option<SocketAddr>,
        buffer_policy: Option<&str>,
    ) -> Result<IssuedDiagnosticExport, DesktopControlOwnerError> {
        self.snapshot_at(now_ms)?;
        self.diagnostics
            .export(&self.bridge, software_version, endpoint, buffer_policy)
            .map_err(DesktopControlOwnerError::Diagnostics)
    }

    /// Borrows the bridge for stream lifecycle execution and platform pumping.
    #[must_use]
    pub const fn bridge(&self) -> &ControlToMixerBridge<O, SLOTS, OUTPUT_SAMPLES_PER_BLOCK> {
        &self.bridge
    }

    /// Mutably borrows the bridge for lifecycle execution and recovery.
    pub fn bridge_mut(&mut self) -> &mut ControlToMixerBridge<O, SLOTS, OUTPUT_SAMPLES_PER_BLOCK> {
        &mut self.bridge
    }

    /// Returns the current number of retained diagnostic exports.
    #[must_use]
    pub fn retained_diagnostic_exports(&self) -> usize {
        self.diagnostics.retained_count()
    }
}

#[cfg(test)]
mod tests {
    use sonimesh_platform::{
        AudioOutput, OutputConfig, OutputDeviceId, OutputDeviceInfo, OutputError, OutputHealth,
        RenderSource,
    };
    use sonimesh_protocol::Capability;

    use super::*;
    use crate::{OutputSelection, SessionPhase, SessionState};

    const SLOTS: usize = 2;
    const OUTPUT_SAMPLES: usize = 256;

    struct TestOutput;

    impl AudioOutput for TestOutput {
        fn devices(&self) -> Result<Vec<OutputDeviceInfo>, OutputError> {
            Ok(Vec::new())
        }

        fn start(
            &mut self,
            _device: Option<&OutputDeviceId>,
            _config: OutputConfig,
            _source: Box<dyn RenderSource>,
        ) -> Result<(), OutputError> {
            Ok(())
        }

        fn stop(&mut self) {}

        fn health(&self) -> OutputHealth {
            OutputHealth::Healthy
        }
    }

    fn owner() -> DesktopControlOwner<TestOutput, SLOTS, OUTPUT_SAMPLES, 1> {
        let bridge = ControlToMixerBridge::new(
            SessionState::IDLE,
            &[Capability::FrameDuration10Ms],
            TestOutput,
            OutputSelection {
                device: None,
                config: OutputConfig {
                    sample_rate_hz: 48_000,
                    channels: 2,
                    sample_format: sonimesh_platform::SampleFormat::Float32,
                    requested_callback_frames: Some(480),
                    maximum_callback_frames: 1_024,
                },
            },
            8,
            8,
        )
        .expect("test bridge is valid");
        DesktopControlOwner::new(bridge).expect("default policy is valid")
    }

    #[test]
    fn owner_projects_pairing_lifecycle_through_one_snapshot_entry_point() {
        let mut owner = owner();
        assert_eq!(
            owner.snapshot_at(100).unwrap().session(),
            SessionPhase::Idle
        );

        owner
            .begin_pairing(PairingId::new([7; 16]), 100, 1_000, 1)
            .unwrap();
        let active = owner.snapshot_at(101).unwrap();
        assert_eq!(active.session(), SessionPhase::Pairing);
        assert_eq!(active.pairing(), crate::PairingSnapshotState::Active);

        owner.expire_pairing().unwrap();
        let expired = owner.snapshot_at(102).unwrap();
        assert_eq!(expired.session(), SessionPhase::Idle);
        assert_eq!(expired.pairing(), crate::PairingSnapshotState::Expired);
    }

    #[test]
    fn owner_export_synchronizes_snapshot_before_issuing_a_bundle() {
        let mut owner = owner();
        let issued = owner
            .export_diagnostics(100, "test", None, Some("bounded"))
            .unwrap();

        assert_eq!(issued.receipt.sequence, 0);
        assert_eq!(issued.receipt.bytes, issued.bundle.len());
        assert!(issued.bundle.contains("\"software_version\":\"test\""));
        assert_eq!(owner.retained_diagnostic_exports(), 1);
    }

    #[test]
    fn owner_rejects_invalid_custom_diagnostic_policy() {
        let bridge = owner().bridge;
        let result = DesktopControlOwner::<TestOutput, SLOTS, OUTPUT_SAMPLES, 1>::with_policy(
            bridge,
            ExportRetentionPolicy::bounded(0, 1),
        );
        assert!(matches!(
            result,
            Err(DesktopControlOwnerError::Diagnostics(
                DiagnosticExportActionError::InvalidRetentionPolicy
            ))
        ));
    }
}
