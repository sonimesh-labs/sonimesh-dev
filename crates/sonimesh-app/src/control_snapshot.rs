use crate::{PairingRecordState, SessionPhase, SessionState};
use sonimesh_platform::OutputHealth;

/// Low-rate desktop lifecycle data safe to project into a control-surface adapter.
///
/// This value deliberately excludes pairing identifiers, proofs, addresses,
/// output-device handles, session material, audio frames, and diagnostics. The
/// pairing owner remains responsible for observing its monotonic clock before
/// building a snapshot, so this type never advances lifecycle state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DesktopControlSnapshot {
    session: SessionPhase,
    pairing: PairingSnapshotState,
    output: OutputSnapshot,
}

/// Redacted output lifecycle suitable for low-rate UI state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputSnapshot {
    selection_retained: bool,
    stream_active: bool,
    fault_reported: bool,
    health: OutputHealth,
}

/// Redacted invitation lifecycle suitable for low-rate UI state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PairingSnapshotState {
    /// No invitation record is currently retained.
    #[default]
    Inactive,
    /// Invitation may accept one bounded, verified pairing proof.
    Active,
    /// Invitation was consumed by an accepted transcript-bound proof.
    Consumed,
    /// Invitation ended without another accepted proof.
    Expired,
    /// Invitation or its trusted session was explicitly revoked.
    Revoked,
}

impl DesktopControlSnapshot {
    /// Projects already-synchronized control state into the UI-safe view.
    #[must_use]
    pub(crate) const fn from_pairing_state(
        session: SessionState,
        pairing: Option<PairingRecordState>,
        output: OutputSnapshot,
    ) -> Self {
        Self {
            session: session.phase(),
            pairing: PairingSnapshotState::from_record(pairing),
            output,
        }
    }

    /// Returns the current read-only receiver lifecycle phase.
    #[must_use]
    pub const fn session(&self) -> SessionPhase {
        self.session
    }

    /// Returns the redacted retained pairing invitation lifecycle.
    #[must_use]
    pub const fn pairing(&self) -> PairingSnapshotState {
        self.pairing
    }

    /// Returns the redacted retained output lifecycle snapshot.
    #[must_use]
    pub const fn output(&self) -> OutputSnapshot {
        self.output
    }
}

impl OutputSnapshot {
    /// Projects the output lifecycle owner's current low-rate state.
    #[must_use]
    pub(crate) const fn from_health(
        selection_retained: bool,
        stream_active: bool,
        fault_reported: bool,
        health: OutputHealth,
    ) -> Self {
        Self {
            selection_retained,
            stream_active,
            fault_reported,
            health,
        }
    }

    /// Returns whether a recovery selection is retained.
    #[must_use]
    pub const fn selection_retained(&self) -> bool {
        self.selection_retained
    }

    /// Returns whether the controller currently owns an active stream.
    #[must_use]
    pub const fn stream_active(&self) -> bool {
        self.stream_active
    }

    /// Returns whether a non-healthy output fault has already been reported.
    #[must_use]
    pub const fn fault_reported(&self) -> bool {
        self.fault_reported
    }

    /// Returns the latest low-rate output health state.
    #[must_use]
    pub const fn health(&self) -> OutputHealth {
        self.health
    }
}

impl PairingSnapshotState {
    const fn from_record(state: Option<PairingRecordState>) -> Self {
        match state {
            None => Self::Inactive,
            Some(PairingRecordState::Active) => Self::Active,
            Some(PairingRecordState::Consumed) => Self::Consumed,
            Some(PairingRecordState::Expired) => Self::Expired,
            Some(PairingRecordState::Revoked) => Self::Revoked,
        }
    }
}

/// Projects a redacted low-rate output snapshot for a control-surface adapter.
///
/// Implementors own the real output lifecycle, so a bridge cannot fabricate
/// output state: the snapshot is read directly from the owner inside the
/// control-domain entry point.
pub trait OutputSnapshotSource {
    /// Returns the owner's current redacted output lifecycle snapshot.
    #[must_use]
    fn snapshot(&self) -> OutputSnapshot;
}

#[cfg(test)]
mod tests {
    use super::{DesktopControlSnapshot, OutputSnapshot, PairingSnapshotState};
    use crate::{PairingRecordState, SessionEvent, SessionState};

    fn output(
        selection_retained: bool,
        stream_active: bool,
        fault_reported: bool,
    ) -> OutputSnapshot {
        OutputSnapshot::from_health(
            selection_retained,
            stream_active,
            fault_reported,
            sonimesh_platform::OutputHealth::Healthy,
        )
    }

    #[test]
    fn output_snapshot_accessors_round_trip() {
        let snapshot = output(true, true, false);
        assert!(snapshot.selection_retained());
        assert!(snapshot.stream_active());
        assert!(!snapshot.fault_reported());
        assert_eq!(snapshot.health(), sonimesh_platform::OutputHealth::Healthy);
    }

    #[test]
    fn pairing_snapshot_defaults_to_inactive() {
        assert_eq!(
            PairingSnapshotState::default(),
            PairingSnapshotState::Inactive
        );
    }

    #[test]
    fn from_pairing_state_projects_every_record_state() {
        let cases = [
            (None, PairingSnapshotState::Inactive),
            (
                Some(PairingRecordState::Active),
                PairingSnapshotState::Active,
            ),
            (
                Some(PairingRecordState::Consumed),
                PairingSnapshotState::Consumed,
            ),
            (
                Some(PairingRecordState::Expired),
                PairingSnapshotState::Expired,
            ),
            (
                Some(PairingRecordState::Revoked),
                PairingSnapshotState::Revoked,
            ),
        ];
        for (record, expected) in cases {
            let snapshot = DesktopControlSnapshot::from_pairing_state(
                SessionState::IDLE,
                record,
                output(false, false, false),
            );
            assert_eq!(snapshot.pairing(), expected);
        }
    }

    #[test]
    fn from_pairing_state_projects_the_session_phase() {
        let mut session = SessionState::IDLE;
        session.apply(SessionEvent::StartPairing).unwrap();

        let snapshot = DesktopControlSnapshot::from_pairing_state(
            session,
            Some(PairingRecordState::Active),
            output(false, false, false),
        );
        assert_eq!(snapshot.session(), crate::SessionPhase::Pairing);
    }
}
