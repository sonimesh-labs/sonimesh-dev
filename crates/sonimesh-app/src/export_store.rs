//! Bounded on-disk diagnostic export retention store (ADR 0011).
//!
//! The desktop control domain owns export retention. This store persists
//! issued bundles to one directory under the same [`ExportRetentionPolicy`]
//! the in-memory journal applies: at most N retained files and at most M
//! retained bytes, evicting the oldest file first, and rejecting a single
//! bundle that cannot fit the byte bound before any file is written. Writes
//! are atomic (write to a pending file, then rename), filenames derive only
//! from the export sequence, and opening the store recovers retained files
//! from the directory and removes stale pending files left by a crash.
//! Nothing here may be called from the realtime callback.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::{ExportReceipt, ExportRetentionPolicy};

/// Filename prefix for retained export bundles.
const EXPORT_FILE_PREFIX: &str = "export-";
/// Filename suffix for retained export bundles.
const EXPORT_FILE_SUFFIX: &str = ".json";
/// Suffix appended while a bundle write is in progress.
const PENDING_FILE_SUFFIX: &str = ".pending";
/// Width of the zero-padded export sequence in a filename.
const SEQUENCE_FIELD_WIDTH: usize = 16;

/// Bounded on-disk diagnostic export store (ADR 0011).
///
/// Owns one directory and applies a fixed retention policy to the files in
/// it. The store is not part of the realtime path and must never be used
/// from the output callback.
pub struct ExportStore {
    directory: PathBuf,
    policy: ExportRetentionPolicy,
    retained: BTreeMap<u64, usize>,
    retained_bytes: usize,
    next_sequence: u64,
}

impl ExportStore {
    /// Opens or creates the store directory and recovers its retained files.
    ///
    /// The directory is created when missing, stale pending files are
    /// removed, retained files are restored from their names, and the fixed
    /// policy is enforced (evicting the oldest files first when the existing
    /// state exceeds the count or byte bounds).
    ///
    /// # Errors
    ///
    /// Returns [`ExportStoreError`] when the policy is not positive, the
    /// target path exists but is not a directory, or a file operation fails.
    pub fn open(
        directory: impl AsRef<Path>,
        policy: ExportRetentionPolicy,
    ) -> Result<Self, ExportStoreError> {
        policy
            .validate()
            .map_err(|_| ExportStoreError::InvalidRetentionPolicy)?;
        let directory = directory.as_ref();
        if directory.exists() && !directory.is_dir() {
            return Err(ExportStoreError::NotADirectory {
                path: directory.to_path_buf(),
            });
        }
        fs::create_dir_all(directory).map_err(ExportStoreError::Io)?;
        let mut retained = BTreeMap::new();
        let mut next_sequence = 0_u64;
        for entry in fs::read_dir(directory).map_err(ExportStoreError::Io)? {
            let entry = entry.map_err(ExportStoreError::Io)?;
            if !entry.file_type().map_err(ExportStoreError::Io)?.is_file() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if parse_pending_file_name(&name).is_some() {
                fs::remove_file(entry.path()).map_err(ExportStoreError::Io)?;
                continue;
            }
            if let Some(sequence) = parse_export_file_name(&name) {
                let bytes = usize::try_from(entry.metadata().map_err(ExportStoreError::Io)?.len())
                    .unwrap_or(usize::MAX);
                retained.insert(sequence, bytes);
                next_sequence = next_sequence.max(sequence.saturating_add(1));
            }
        }
        let retained_bytes = retained
            .values()
            .try_fold(0_usize, |total, &bytes| total.checked_add(bytes))
            .unwrap_or(usize::MAX);
        let mut store = Self {
            directory: directory.to_path_buf(),
            policy,
            retained,
            retained_bytes,
            next_sequence,
        };
        store.evict_to_policy()?;
        Ok(store)
    }

    /// Persists one bundle under the retention policy and returns its receipt.
    ///
    /// The bundle is written atomically to the store directory. When the
    /// count or byte bounds are exceeded by the new export, the oldest
    /// retained files are evicted until the bounds hold. A bundle larger than
    /// the byte bound is rejected before any file is written.
    ///
    /// # Errors
    ///
    /// Returns [`ExportStoreError::SequenceExhausted`] when the export
    /// sequence space is exhausted,
    /// [`ExportStoreError::ExportBytesExceedRetentionLimit`] when one bundle
    /// cannot fit the byte bound, or [`ExportStoreError::Io`] when a file
    /// operation fails. On an eviction failure the new export may remain on
    /// disk and is reconciled on the next [`Self::open`].
    pub fn store(&mut self, bundle: &str) -> Result<ExportReceipt, ExportStoreError> {
        if self.next_sequence == u64::MAX {
            return Err(ExportStoreError::SequenceExhausted);
        }
        let bytes = bundle.len();
        if bytes > self.policy.max_retained_bytes {
            return Err(ExportStoreError::ExportBytesExceedRetentionLimit {
                bytes,
                max_retained_bytes: self.policy.max_retained_bytes,
            });
        }
        let receipt = ExportReceipt {
            sequence: self.next_sequence,
            bytes,
        };
        write_bundle_atomically(&self.directory, receipt.sequence, bundle)
            .map_err(ExportStoreError::Io)?;
        self.retained.insert(receipt.sequence, bytes);
        self.retained_bytes = self.retained_bytes.saturating_add(bytes);
        self.next_sequence += 1;
        self.evict_to_policy()?;
        Ok(receipt)
    }

    /// Reads one retained bundle back from the store directory.
    ///
    /// # Errors
    ///
    /// Returns [`ExportStoreError::NotRetained`] when the sequence is not
    /// currently retained, or [`ExportStoreError::Io`] when the file cannot
    /// be read.
    pub fn read(&self, sequence: u64) -> Result<String, ExportStoreError> {
        if !self.retained.contains_key(&sequence) {
            return Err(ExportStoreError::NotRetained { sequence });
        }
        fs::read_to_string(self.directory.join(export_file_name(sequence)))
            .map_err(ExportStoreError::Io)
    }

    /// Returns the retained receipts in ascending sequence order.
    #[must_use]
    pub fn receipts(&self) -> Vec<ExportReceipt> {
        self.retained
            .iter()
            .map(|(&sequence, &bytes)| ExportReceipt { sequence, bytes })
            .collect()
    }

    /// Returns the store's fixed retention policy.
    #[must_use]
    pub const fn policy(&self) -> ExportRetentionPolicy {
        self.policy
    }

    /// Returns the store's directory.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Returns the number of currently retained files.
    #[must_use]
    pub fn retained_count(&self) -> usize {
        self.retained.len()
    }

    /// Returns the total retained on-disk bytes.
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    /// Returns the next export sequence that will be persisted.
    #[must_use]
    pub const fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    fn evict_to_policy(&mut self) -> Result<(), ExportStoreError> {
        while self.retained.len() > self.policy.max_retained_exports
            || self.retained_bytes > self.policy.max_retained_bytes
        {
            let Some(oldest) = self.retained.keys().next().copied() else {
                break;
            };
            fs::remove_file(self.directory.join(export_file_name(oldest)))
                .map_err(ExportStoreError::Io)?;
            if let Some(bytes) = self.retained.remove(&oldest) {
                self.retained_bytes = self.retained_bytes.saturating_sub(bytes);
            }
        }
        Ok(())
    }
}

fn export_file_name(sequence: u64) -> String {
    format!("{EXPORT_FILE_PREFIX}{sequence:0SEQUENCE_FIELD_WIDTH$}{EXPORT_FILE_SUFFIX}")
}

fn pending_file_name(sequence: u64) -> String {
    format!("{}{PENDING_FILE_SUFFIX}", export_file_name(sequence))
}

fn parse_export_file_name(name: &str) -> Option<u64> {
    let sequence = name
        .strip_prefix(EXPORT_FILE_PREFIX)?
        .strip_suffix(EXPORT_FILE_SUFFIX)?
        .parse()
        .ok()?;
    (export_file_name(sequence) == name).then_some(sequence)
}

fn parse_pending_file_name(name: &str) -> Option<u64> {
    let name = name.strip_suffix(PENDING_FILE_SUFFIX)?;
    parse_export_file_name(name)
}

fn write_bundle_atomically(directory: &Path, sequence: u64, bundle: &str) -> io::Result<()> {
    let pending_path = directory.join(pending_file_name(sequence));
    let final_path = directory.join(export_file_name(sequence));
    let mut pending = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&pending_path)?;
    // Once this call has created the pending file, clean it up on any later
    // write, flush, or rename failure. Crash recovery still handles a process
    // that dies before cleanup, while a live store must not poison its next
    // sequence with a stale pending path after a transient I/O error.
    let result = pending
        .write_all(bundle.as_bytes())
        .and_then(|()| pending.sync_all());
    drop(pending);
    let result = result.and_then(|()| fs::rename(&pending_path, &final_path));
    if result.is_err() {
        let _ = fs::remove_file(&pending_path);
    }
    result
}

/// Failure while opening or retaining an on-disk export store.
pub enum ExportStoreError {
    /// A retention policy bound must be positive.
    InvalidRetentionPolicy,
    /// The target path exists but is not a directory.
    NotADirectory {
        /// The offending path.
        path: PathBuf,
    },
    /// A store file operation failed.
    Io(io::Error),
    /// The export sequence space is exhausted.
    SequenceExhausted,
    /// One bundle exceeds the retention byte bound and cannot be retained.
    ExportBytesExceedRetentionLimit {
        /// Serialized bundle byte size.
        bytes: usize,
        /// Fixed retention byte bound.
        max_retained_bytes: usize,
    },
    /// The requested export sequence is not currently retained.
    NotRetained {
        /// The missing sequence.
        sequence: u64,
    },
}

impl fmt::Debug for ExportStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRetentionPolicy => formatter.write_str("InvalidRetentionPolicy"),
            Self::NotADirectory { .. } => formatter.write_str("NotADirectory { path: [REDACTED] }"),
            Self::Io(error) => formatter
                .debug_struct("Io")
                .field("kind", &error.kind())
                .finish(),
            Self::SequenceExhausted => formatter.write_str("SequenceExhausted"),
            Self::ExportBytesExceedRetentionLimit {
                bytes,
                max_retained_bytes,
            } => formatter
                .debug_struct("ExportBytesExceedRetentionLimit")
                .field("bytes", bytes)
                .field("max_retained_bytes", max_retained_bytes)
                .finish(),
            Self::NotRetained { sequence } => formatter
                .debug_struct("NotRetained")
                .field("sequence", sequence)
                .finish(),
        }
    }
}

impl fmt::Display for ExportStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRetentionPolicy => {
                formatter.write_str("export store retention bounds must be positive")
            }
            Self::NotADirectory { path } => write!(
                formatter,
                "export store path is not a directory: {}",
                path.display()
            ),
            Self::Io(error) => write!(formatter, "export store file operation failed: {error}"),
            Self::SequenceExhausted => formatter.write_str("export store sequence exhausted"),
            Self::ExportBytesExceedRetentionLimit {
                bytes,
                max_retained_bytes,
            } => write!(
                formatter,
                "export store bundle of {bytes} bytes exceeds the {max_retained_bytes}-byte retention bound"
            ),
            Self::NotRetained { sequence } => {
                write!(
                    formatter,
                    "export store does not retain sequence {sequence}"
                )
            }
        }
    }
}

impl std::error::Error for ExportStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidRetentionPolicy
            | Self::NotADirectory { .. }
            | Self::SequenceExhausted
            | Self::ExportBytesExceedRetentionLimit { .. }
            | Self::NotRetained { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let base = std::env::temp_dir().join("sonimesh-export-store-unit-tests");
            let id = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
            let path = base.join(format!("{}-{id}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temporary directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn export_file_names_round_trip_through_the_parser() {
        for sequence in [0, 1, 9, 10, 123_456, u64::MAX] {
            let name = export_file_name(sequence);
            assert_eq!(parse_export_file_name(&name), Some(sequence));
        }
        assert_eq!(export_file_name(0), "export-0000000000000000.json");
    }

    #[test]
    fn pending_file_names_round_trip_through_the_pending_parser() {
        let name = pending_file_name(7);
        assert_eq!(parse_pending_file_name(&name), Some(7));
        assert_eq!(parse_export_file_name(&name), None);
    }

    #[test]
    fn foreign_names_do_not_parse_as_exports() {
        assert_eq!(parse_export_file_name("export-abc.json"), None);
        assert_eq!(parse_export_file_name("export-1.json"), None);
        assert_eq!(
            parse_export_file_name("export-00000000000000001.json"),
            None
        );
        assert_eq!(parse_export_file_name("notes.txt"), None);
        assert_eq!(
            parse_export_file_name("export-0000000000000001.json.extra"),
            None
        );
        assert_eq!(parse_export_file_name(""), None);
        assert_eq!(parse_pending_file_name("export-notes.json.pending"), None);
        assert_eq!(parse_pending_file_name("export-1.json.pending"), None);
        assert_eq!(parse_pending_file_name("notes.txt.pending"), None);
    }

    #[test]
    fn error_debug_redacts_paths_and_io_details() {
        let path_error = ExportStoreError::NotADirectory {
            path: PathBuf::from("C:\\Users\\private\\exports"),
        };
        let io_error = ExportStoreError::Io(io::Error::other("private export path"));

        let path_debug = format!("{path_error:?}");
        let io_debug = format!("{io_error:?}");

        assert_eq!(path_debug, "NotADirectory { path: [REDACTED] }");
        assert!(io_debug.contains("kind: Other"));
        assert!(!io_debug.contains("private export path"));
    }

    #[test]
    fn failed_atomic_rename_does_not_poison_the_pending_path() {
        let directory = TempDir::new();
        fs::create_dir(directory.path().join(export_file_name(0)))
            .expect("create conflicting final directory");

        let error = write_bundle_atomically(directory.path(), 0, "bundle")
            .expect_err("rename over a directory must fail");

        assert_ne!(error.kind(), io::ErrorKind::NotFound);
        assert!(!directory.path().join(pending_file_name(0)).exists());
    }
}
