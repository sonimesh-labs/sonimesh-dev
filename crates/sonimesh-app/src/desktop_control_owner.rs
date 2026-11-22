//! Low-rate desktop command owner independent of Tauri or a web view.
//!
//! This composition layer keeps the UI-facing lifecycle operations together:
//! snapshot polling, pairing lifecycle, local stream controls, and diagnostics.
//! Stream admission and realtime rendering remain owned by
//! [`ControlToMixerBridge`]; this owner only delegates those bounded control
//! operations and never touches the output callback.

use std::net::SocketAddr;

use sonimesh_platform::{AudioOutput, OutputDeviceId, OutputDeviceInfo, OutputError};

use crate::{
    ControlToMixerBridge, ControlToMixerError, DesktopControlSnapshot, DiagnosticExportAction,
    DiagnosticExportActionError, ExportReceipt, ExportRetentionPolicy, ExportStore,
    ExportStoreError, GainControlState, IssuedDiagnosticExport, MediaStreamIdentity,
    PairingAttemptOutcome, PairingId, PairingInvitationController, PairingInvitationError,
    PairingProofVerification, PairingSource,
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
    /// Output enumeration failed in the platform adapter.
    Output(OutputError),
    /// Diagnostic export persistence failed.
    ExportStore(ExportStoreError),
}

/// Shell-neutral low-rate desktop view assembled from one control owner.
///
/// The lifecycle snapshot remains redacted, while output inventory and the
/// retained selection are supplied by the platform adapter. `stream_control`
/// is present only for a single admitted stream; multi-stream owners must use
/// identity-addressed controls instead of guessing which stream the UI means.
#[derive(Debug)]
pub struct DesktopControlView {
    lifecycle: DesktopControlSnapshot,
    output_devices: Vec<OutputDeviceInfo>,
    selected_output: Option<OutputDeviceId>,
    stream_control: Option<GainControlState>,
}

impl DesktopControlView {
    /// Returns the synchronized lifecycle projection.
    #[must_use]
    pub const fn lifecycle(&self) -> DesktopControlSnapshot {
        self.lifecycle
    }

    /// Returns the latest platform output inventory.
    #[must_use]
    pub fn output_devices(&self) -> &[OutputDeviceInfo] {
        &self.output_devices
    }

    /// Returns the output retained for the next output lifetime.
    #[must_use]
    pub fn selected_output(&self) -> Option<&OutputDeviceId> {
        self.selected_output.as_ref()
    }

    /// Returns single-stream gain/mute state, when the owner can project it.
    #[must_use]
    pub const fn stream_control(&self) -> Option<GainControlState> {
        self.stream_control
    }
}

impl core::fmt::Display for DesktopControlOwnerError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Pairing(error) => write!(formatter, "desktop control pairing failure: {error}"),
            Self::Diagnostics(error) => {
                write!(formatter, "desktop control diagnostics failure: {error}")
            }
            Self::Mixer(error) => write!(formatter, "desktop control mixer failure: {error}"),
            Self::Output(error) => write!(formatter, "desktop control output failure: {error}"),
            Self::ExportStore(error) => {
                write!(formatter, "desktop control export store failure: {error}")
            }
        }
    }
}

impl std::error::Error for DesktopControlOwnerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Pairing(error) => Some(error),
            Self::Diagnostics(error) => Some(error),
            Self::Mixer(error) => Some(error),
            Self::Output(error) => Some(error),
            Self::ExportStore(error) => Some(error),
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

    /// Builds one low-rate view for a shell command or control surface.
    ///
    /// # Errors
    ///
    /// Returns a pairing error when lifecycle clock observation fails or an
    /// output error when the platform inventory cannot be read.
    pub fn view_at(&mut self, now_ms: u64) -> Result<DesktopControlView, DesktopControlOwnerError> {
        let lifecycle = self.snapshot_at(now_ms)?;
        let output_devices = self.output_devices()?;
        Ok(DesktopControlView {
            lifecycle,
            output_devices,
            selected_output: self.selected_output_device(),
            stream_control: self.bridge.single_stream_gain(),
        })
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

    /// Enumerates output endpoints for the desktop control surface.
    ///
    /// # Errors
    ///
    /// Returns an output error when the platform cannot enumerate endpoints.
    pub fn output_devices(&self) -> Result<Vec<OutputDeviceInfo>, DesktopControlOwnerError> {
        self.bridge
            .output_devices()
            .map_err(DesktopControlOwnerError::Output)
    }

    /// Retains an output device for the next output lifetime.
    ///
    /// Passing `None` selects the operating system's default output role. A
    /// concrete device is checked against the current platform inventory before
    /// it is retained, so a stale UI selection fails without mutating state.
    ///
    /// # Errors
    ///
    /// Returns an output error when the device is not present or enumeration
    /// fails, or a mixer error when the output is currently active.
    pub fn select_output_device(
        &mut self,
        device: Option<OutputDeviceId>,
    ) -> Result<(), DesktopControlOwnerError> {
        if let Some(device) = device.as_ref() {
            let available = self.output_devices()?;
            if !available.iter().any(|candidate| candidate.id == *device) {
                return Err(DesktopControlOwnerError::Output(
                    OutputError::DeviceUnavailable,
                ));
            }
        }
        self.bridge
            .set_output_device(device)
            .map_err(DesktopControlOwnerError::Mixer)
    }

    /// Returns the output device retained for the next output lifetime.
    #[must_use]
    pub fn selected_output_device(&self) -> Option<OutputDeviceId> {
        self.bridge.selected_output_device()
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

    /// Generates one bounded diagnostic bundle and persists it atomically.
    ///
    /// The in-memory action still records its bounded receipt, while the store
    /// owns the on-disk sequence and retention policy. The returned receipt is
    /// the persisted file's receipt and can be mapped directly to a UI export
    /// entry.
    ///
    /// # Errors
    ///
    /// Returns a pairing or serialization error before any file write, or an
    /// export-store error when the atomic write or retention step fails.
    pub fn export_diagnostics_to_store(
        &mut self,
        now_ms: u64,
        software_version: &str,
        endpoint: Option<SocketAddr>,
        buffer_policy: Option<&str>,
        store: &mut ExportStore,
    ) -> Result<ExportReceipt, DesktopControlOwnerError> {
        let issued = self.export_diagnostics(now_ms, software_version, endpoint, buffer_policy)?;
        store
            .store(&issued.bundle)
            .map_err(DesktopControlOwnerError::ExportStore)
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
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use sonimesh_platform::{
        AudioOutput, OutputConfig, OutputDeviceId, OutputDeviceInfo, OutputError, OutputHealth,
        RenderSource,
    };
    use sonimesh_protocol::Capability;

    use super::*;
    use crate::{OutputSelection, SessionPhase, SessionState};

    const SLOTS: usize = 2;
    const OUTPUT_SAMPLES: usize = 256;
    static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

    struct TestOutput;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let id = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join("sonimesh-desktop-control-owner-tests")
                .join(format!("{}-{id}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temporary directory");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

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
    fn owner_keeps_default_selection_and_rejects_stale_device_ids() {
        let mut owner = owner();
        assert!(owner.output_devices().unwrap().is_empty());
        owner.select_output_device(None).unwrap();
        assert!(owner.selected_output_device().is_none());

        let result = owner.select_output_device(Some(OutputDeviceId::new("missing")));
        assert!(matches!(
            result,
            Err(DesktopControlOwnerError::Output(
                OutputError::DeviceUnavailable
            ))
        ));
        assert!(owner.selected_output_device().is_none());
    }

    #[test]
    fn owner_view_combines_lifecycle_inventory_and_single_stream_projection() {
        let mut owner = owner();
        let view = owner.view_at(100).unwrap();

        assert_eq!(view.lifecycle().session(), SessionPhase::Idle);
        assert!(view.output_devices().is_empty());
        assert!(view.selected_output().is_none());
        assert!(view.stream_control().is_none());
    }

    #[test]
    fn owner_persists_diagnostics_through_the_bounded_store() {
        let temp_dir = TempDir::new();
        let mut owner = owner();
        let mut store = ExportStore::open(&temp_dir.0, ExportRetentionPolicy::default()).unwrap();

        let receipt = owner
            .export_diagnostics_to_store(100, "test", None, Some("bounded"), &mut store)
            .unwrap();

        assert_eq!(receipt.sequence, 0);
        assert_eq!(store.receipts(), vec![receipt]);
        assert!(
            store
                .read(receipt.sequence)
                .unwrap()
                .contains("\"software_version\":\"test\"")
        );
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
