//! Bounded app-side diagnostic export action and retention journal (ADR 0011).
//!
//! The desktop control domain owns the export action: it fills the allowlisted
//! snapshot from live control and mixer state, serializes exactly one bounded
//! bundle through [`build_export_bundle`], and records a receipt under a fixed
//! retention policy. This module never writes files; the bounded on-disk store
//! lives in [`crate::export_store`], and the UI/Tauri command that triggers an
//! export remains open. Nothing here may be called from the realtime callback.

use std::collections::VecDeque;
use std::fmt;
use std::net::SocketAddr;

use sonimesh_diagnostics::{
    ExportError, MAX_EXPORT_BYTES, StreamMetricsSnapshot, build_export_bundle,
};
use sonimesh_platform::AudioOutput;

use crate::ControlToMixerBridge;

/// Default hard bound on retained export records.
pub const DEFAULT_MAX_RETAINED_EXPORTS: usize = 8;
/// Default hard bound on total retained export bytes (eight maximum bundles).
pub const DEFAULT_MAX_RETAINED_EXPORT_BYTES: usize = 8 * MAX_EXPORT_BYTES;

/// Fixed retention bounds for issued diagnostic exports (ADR 0011).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExportRetentionPolicy {
    /// Hard bound on retained export records.
    pub max_retained_exports: usize,
    /// Hard bound on total retained export bytes.
    pub max_retained_bytes: usize,
}

impl ExportRetentionPolicy {
    /// Builds a bounded retention policy.
    #[must_use]
    pub const fn bounded(max_retained_exports: usize, max_retained_bytes: usize) -> Self {
        Self {
            max_retained_exports,
            max_retained_bytes,
        }
    }

    /// Validates that both bounds are positive.
    ///
    /// # Errors
    ///
    /// Returns [`DiagnosticExportActionError::InvalidRetentionPolicy`] when
    /// either bound is zero.
    pub fn validate(self) -> Result<(), DiagnosticExportActionError> {
        if self.max_retained_exports == 0 || self.max_retained_bytes == 0 {
            return Err(DiagnosticExportActionError::InvalidRetentionPolicy);
        }
        Ok(())
    }
}

impl Default for ExportRetentionPolicy {
    fn default() -> Self {
        Self::bounded(
            DEFAULT_MAX_RETAINED_EXPORTS,
            DEFAULT_MAX_RETAINED_EXPORT_BYTES,
        )
    }
}

/// One export receipt retained by the journal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExportReceipt {
    /// Monotonic export sequence, starting at zero.
    pub sequence: u64,
    /// Serialized bundle byte size.
    pub bytes: usize,
}

/// One issued export bundle and its retention receipt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssuedDiagnosticExport {
    /// Retention receipt (sequence and byte size).
    pub receipt: ExportReceipt,
    /// Serialized versioned bundle the desktop owner may write.
    pub bundle: String,
}

/// Bounded in-memory ledger of issued exports.
///
/// The desktop owner applies the same policy to its on-disk retention. The
/// journal keeps the newest exports by evicting the oldest record until the
/// count and byte bounds hold, and rejects a single bundle that cannot fit
/// the byte bound.
pub struct ExportRetentionJournal {
    policy: ExportRetentionPolicy,
    records: VecDeque<ExportReceipt>,
    retained_bytes: usize,
    next_sequence: u64,
}

impl ExportRetentionJournal {
    /// Creates an empty journal under a validated policy.
    ///
    /// # Errors
    ///
    /// Returns [`DiagnosticExportActionError::InvalidRetentionPolicy`] when
    /// the policy bound is not positive.
    pub fn new(policy: ExportRetentionPolicy) -> Result<Self, DiagnosticExportActionError> {
        policy.validate()?;
        Ok(Self {
            policy,
            records: VecDeque::new(),
            retained_bytes: 0,
            next_sequence: 0,
        })
    }

    /// Records one issued export under the retention policy.
    ///
    /// # Errors
    ///
    /// Returns [`DiagnosticExportActionError::SequenceExhausted`] when the
    /// export sequence space is exhausted, or
    /// [`DiagnosticExportActionError::ExportBytesExceedRetentionLimit`] when a
    /// single bundle cannot fit the byte bound.
    pub fn record(&mut self, bytes: usize) -> Result<ExportReceipt, DiagnosticExportActionError> {
        if self.next_sequence == u64::MAX {
            return Err(DiagnosticExportActionError::SequenceExhausted);
        }
        if bytes > self.policy.max_retained_bytes {
            return Err(
                DiagnosticExportActionError::ExportBytesExceedRetentionLimit {
                    bytes,
                    max_retained_bytes: self.policy.max_retained_bytes,
                },
            );
        }
        let receipt = ExportReceipt {
            sequence: self.next_sequence,
            bytes,
        };
        self.next_sequence += 1;
        self.records.push_back(receipt);
        self.retained_bytes += bytes;
        while self.records.len() > self.policy.max_retained_exports
            || self.retained_bytes > self.policy.max_retained_bytes
        {
            let Some(evicted) = self.records.pop_front() else {
                break;
            };
            self.retained_bytes -= evicted.bytes;
        }
        Ok(receipt)
    }

    /// Returns the number of retained export records.
    #[must_use]
    pub fn retained_count(&self) -> usize {
        self.records.len()
    }

    /// Returns the total retained export bytes.
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    /// Returns the journal's fixed retention policy.
    #[must_use]
    pub const fn policy(&self) -> ExportRetentionPolicy {
        self.policy
    }

    /// Returns the next export sequence that will be issued.
    #[must_use]
    pub const fn next_sequence(&self) -> u64 {
        self.next_sequence
    }
}

/// App-side diagnostic export action (ADR 0011).
///
/// Owns the reusable metrics buffer and a bounded retention journal so the
/// desktop control domain can issue one export per command from live bridge
/// state without re-allocating the snapshot buffer on every export.
pub struct DiagnosticExportAction {
    metrics: Vec<StreamMetricsSnapshot>,
    journal: ExportRetentionJournal,
}

impl DiagnosticExportAction {
    /// Creates an empty action under a validated retention policy.
    ///
    /// # Errors
    ///
    /// Returns [`DiagnosticExportActionError::InvalidRetentionPolicy`] when
    /// the policy bound is not positive.
    pub fn new(policy: ExportRetentionPolicy) -> Result<Self, DiagnosticExportActionError> {
        policy.validate()?;
        Ok(Self {
            metrics: Vec::new(),
            journal: ExportRetentionJournal::new(policy)?,
        })
    }

    /// Issues one diagnostic export from live bridge state.
    ///
    /// The action fills the allowlisted snapshot through
    /// [`ControlToMixerBridge::export_snapshot`], serializes exactly one
    /// bounded bundle through [`build_export_bundle`], and records the receipt
    /// under the retention policy.
    ///
    /// # Errors
    ///
    /// Returns [`DiagnosticExportActionError`] when the bundle cannot be
    /// serialized or retained.
    pub fn export<O, const SLOTS: usize, const OUTPUT_SAMPLES_PER_BLOCK: usize>(
        &mut self,
        bridge: &ControlToMixerBridge<O, SLOTS, OUTPUT_SAMPLES_PER_BLOCK>,
        software_version: &str,
        endpoint: Option<SocketAddr>,
        buffer_policy: Option<&str>,
    ) -> Result<IssuedDiagnosticExport, DiagnosticExportActionError>
    where
        O: AudioOutput,
    {
        let snapshot =
            bridge.export_snapshot(software_version, endpoint, buffer_policy, &mut self.metrics);
        let bundle = build_export_bundle(&snapshot).map_err(DiagnosticExportActionError::Bundle)?;
        let receipt = self.journal.record(bundle.len())?;
        Ok(IssuedDiagnosticExport { receipt, bundle })
    }

    /// Returns the action's fixed retention policy.
    #[must_use]
    pub const fn policy(&self) -> ExportRetentionPolicy {
        self.journal.policy()
    }

    /// Returns the number of retained export records.
    #[must_use]
    pub fn retained_count(&self) -> usize {
        self.journal.retained_count()
    }

    /// Returns the total retained export bytes.
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        self.journal.retained_bytes()
    }
}

/// Failure while issuing or retaining a diagnostic export.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticExportActionError {
    /// A retention policy bound must be positive.
    InvalidRetentionPolicy,
    /// The bundle could not be serialized.
    Bundle(ExportError),
    /// One bundle exceeds the retention byte bound and cannot be retained.
    ExportBytesExceedRetentionLimit {
        /// Serialized bundle byte size.
        bytes: usize,
        /// Fixed retention byte bound.
        max_retained_bytes: usize,
    },
    /// The export sequence space is exhausted.
    SequenceExhausted,
}

impl fmt::Display for DiagnosticExportActionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRetentionPolicy => {
                formatter.write_str("diagnostic export retention bounds must be positive")
            }
            Self::Bundle(error) => write!(formatter, "diagnostic export bundle failed: {error}"),
            Self::ExportBytesExceedRetentionLimit {
                bytes,
                max_retained_bytes,
            } => write!(
                formatter,
                "diagnostic export bundle of {bytes} bytes exceeds the {max_retained_bytes}-byte retention bound"
            ),
            Self::SequenceExhausted => formatter.write_str("diagnostic export sequence exhausted"),
        }
    }
}

impl std::error::Error for DiagnosticExportActionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Bundle(error) => Some(error),
            Self::InvalidRetentionPolicy
            | Self::ExportBytesExceedRetentionLimit { .. }
            | Self::SequenceExhausted => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    #[test]
    fn policy_rejects_zero_bounds() {
        assert_eq!(
            ExportRetentionPolicy::bounded(0, 100).validate(),
            Err(DiagnosticExportActionError::InvalidRetentionPolicy)
        );
        assert_eq!(
            ExportRetentionPolicy::bounded(4, 0).validate(),
            Err(DiagnosticExportActionError::InvalidRetentionPolicy)
        );
        assert!(ExportRetentionPolicy::bounded(4, 100).validate().is_ok());
    }

    #[test]
    fn journal_evicts_the_oldest_record_beyond_the_count_bound() {
        let mut journal =
            ExportRetentionJournal::new(ExportRetentionPolicy::bounded(3, 1_024)).unwrap();

        let first = journal.record(100).unwrap();
        journal.record(100).unwrap();
        journal.record(100).unwrap();
        let fourth = journal.record(100).unwrap();

        assert_eq!(journal.retained_count(), 3);
        assert_eq!(journal.retained_bytes(), 300);
        assert_eq!(journal.next_sequence(), 4);
        assert_eq!(first.sequence, 0);
        assert_eq!(fourth.sequence, 3);
    }

    #[test]
    fn journal_evicts_the_oldest_record_beyond_the_byte_bound() {
        let mut journal =
            ExportRetentionJournal::new(ExportRetentionPolicy::bounded(10, 250)).unwrap();

        journal.record(100).unwrap();
        journal.record(100).unwrap();
        journal.record(100).unwrap();

        assert_eq!(journal.retained_count(), 2);
        assert_eq!(journal.retained_bytes(), 200);
    }

    #[test]
    fn a_single_bundle_larger_than_the_byte_bound_is_rejected() {
        let mut journal =
            ExportRetentionJournal::new(ExportRetentionPolicy::bounded(4, 100)).unwrap();

        assert_eq!(
            journal.record(150),
            Err(
                DiagnosticExportActionError::ExportBytesExceedRetentionLimit {
                    bytes: 150,
                    max_retained_bytes: 100,
                }
            )
        );
        assert_eq!(journal.retained_count(), 0);
        assert_eq!(journal.retained_bytes(), 0);
    }

    #[test]
    fn sequence_space_exhaustion_fails_closed() {
        let mut journal = ExportRetentionJournal {
            policy: ExportRetentionPolicy::bounded(4, 1_024),
            records: VecDeque::new(),
            retained_bytes: 0,
            next_sequence: u64::MAX,
        };

        assert_eq!(
            journal.record(10),
            Err(DiagnosticExportActionError::SequenceExhausted)
        );
        assert_eq!(journal.retained_count(), 0);
    }

    #[test]
    fn action_holds_a_bounded_policy_and_accessors() {
        let action = DiagnosticExportAction::new(ExportRetentionPolicy::bounded(4, 1_024)).unwrap();

        assert_eq!(action.policy(), ExportRetentionPolicy::bounded(4, 1_024));
        assert_eq!(action.retained_count(), 0);
        assert_eq!(action.retained_bytes(), 0);
    }

    #[test]
    fn action_rejects_an_invalid_policy() {
        assert!(matches!(
            DiagnosticExportAction::new(ExportRetentionPolicy::bounded(0, 100)),
            Err(DiagnosticExportActionError::InvalidRetentionPolicy)
        ));
    }

    #[test]
    fn preserved_error_variants_expose_their_causes() {
        let bundle = DiagnosticExportActionError::Bundle(ExportError::EmptySoftwareVersion);
        assert!(
            Error::source(&bundle)
                .and_then(|source| source.downcast_ref::<ExportError>())
                .is_some()
        );

        assert!(Error::source(&DiagnosticExportActionError::InvalidRetentionPolicy).is_none());
        assert!(Error::source(&DiagnosticExportActionError::SequenceExhausted).is_none());
        assert!(
            Error::source(
                &DiagnosticExportActionError::ExportBytesExceedRetentionLimit {
                    bytes: 1,
                    max_retained_bytes: 1,
                }
            )
            .is_none()
        );
    }
}
