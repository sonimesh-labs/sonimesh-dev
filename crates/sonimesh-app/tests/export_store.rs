//! Black-box coverage for the on-disk diagnostic export store (ADR 0011).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sonimesh_app::{ExportRetentionPolicy, ExportStore, ExportStoreError};

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

/// One unique temporary directory removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let base = std::env::temp_dir().join("sonimesh-export-store-tests");
        let id = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let path = base.join(format!("{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temporary directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn export_file_name(sequence: u64) -> String {
    format!("export-{sequence:016}.json")
}

fn pending_file_name(sequence: u64) -> String {
    format!("export-{sequence:016}.json.pending")
}

#[test]
fn store_persists_bundles_and_evicts_the_oldest_beyond_the_count_bound() {
    let dir = TempDir::new();
    let mut store = ExportStore::open(dir.path(), ExportRetentionPolicy::bounded(2, 4_096))
        .expect("open store");

    let first = store.store("bundle-a").expect("store bundle");
    let second = store.store("bundle-b").expect("store bundle");
    let third = store.store("bundle-c").expect("store bundle");

    assert_eq!(first.sequence, 0);
    assert_eq!(third.sequence, 2);
    assert_eq!(store.retained_count(), 2);
    assert_eq!(store.retained_bytes(), second.bytes + third.bytes);
    assert_eq!(store.receipts(), vec![second, third]);
    assert!(matches!(
        store.read(0),
        Err(ExportStoreError::NotRetained { sequence: 0 })
    ));
    assert_eq!(store.read(1).expect("read retained bundle"), "bundle-b");
    assert_eq!(store.read(2).expect("read retained bundle"), "bundle-c");
    assert_eq!(store.next_sequence(), 3);
    assert_eq!(
        std::fs::read_dir(dir.path())
            .expect("read store directory")
            .count(),
        2
    );
}

#[test]
fn store_evicts_the_oldest_beyond_the_byte_bound() {
    let dir = TempDir::new();
    let mut store =
        ExportStore::open(dir.path(), ExportRetentionPolicy::bounded(10, 250)).expect("open store");

    store.store(&"x".repeat(100)).expect("store bundle");
    store.store(&"x".repeat(100)).expect("store bundle");
    store.store(&"x".repeat(100)).expect("store bundle");

    assert_eq!(store.retained_count(), 2);
    assert_eq!(store.retained_bytes(), 200);
    assert_eq!(store.next_sequence(), 3);
    assert_eq!(store.receipts()[0].sequence, 1);
    assert_eq!(store.receipts()[1].sequence, 2);
}

#[test]
fn oversized_bundle_is_rejected_before_any_file_is_written() {
    let dir = TempDir::new();
    let mut store =
        ExportStore::open(dir.path(), ExportRetentionPolicy::bounded(4, 100)).expect("open store");

    let bundle = "x".repeat(150);
    assert!(matches!(
        store.store(&bundle),
        Err(ExportStoreError::ExportBytesExceedRetentionLimit {
            bytes: 150,
            max_retained_bytes: 100,
        })
    ));
    assert_eq!(store.retained_count(), 0);
    assert_eq!(store.retained_bytes(), 0);
    assert_eq!(
        std::fs::read_dir(dir.path())
            .expect("read store directory")
            .count(),
        0
    );
}

#[test]
fn opening_recovers_retained_exports_and_resumes_the_sequence() {
    let dir = TempDir::new();
    {
        let mut store = ExportStore::open(dir.path(), ExportRetentionPolicy::bounded(8, 4_096))
            .expect("open store");
        store.store("one").expect("store bundle");
        store.store("two").expect("store bundle");
        store.store("three").expect("store bundle");
    }

    let store = ExportStore::open(dir.path(), ExportRetentionPolicy::bounded(8, 4_096))
        .expect("reopen store");

    assert_eq!(store.retained_count(), 3);
    assert_eq!(store.next_sequence(), 3);
    assert_eq!(
        store
            .receipts()
            .iter()
            .map(|receipt| receipt.sequence)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(store.read(0).expect("read recovered bundle"), "one");
}

#[test]
fn opening_evicts_oldest_files_under_a_shrunk_policy() {
    let dir = TempDir::new();
    {
        let mut store = ExportStore::open(dir.path(), ExportRetentionPolicy::bounded(8, 4_096))
            .expect("open store");
        store.store("one").expect("store bundle");
        store.store("two").expect("store bundle");
        store.store("three").expect("store bundle");
    }

    let store = ExportStore::open(dir.path(), ExportRetentionPolicy::bounded(1, 4_096))
        .expect("reopen store");

    assert_eq!(store.retained_count(), 1);
    assert_eq!(store.receipts()[0].sequence, 2);
    assert_eq!(
        std::fs::read_dir(dir.path())
            .expect("read store directory")
            .count(),
        1
    );
    assert!(!dir.path().join(export_file_name(0)).exists());
    assert!(dir.path().join(export_file_name(2)).exists());
}

#[test]
fn opening_removes_stale_pending_files() {
    let dir = TempDir::new();
    std::fs::write(dir.path().join(pending_file_name(5)), "partial")
        .expect("write stale pending file");

    let store = ExportStore::open(dir.path(), ExportRetentionPolicy::bounded(4, 4_096))
        .expect("open store");

    assert_eq!(store.retained_count(), 0);
    assert_eq!(store.next_sequence(), 0);
    assert_eq!(
        std::fs::read_dir(dir.path())
            .expect("read store directory")
            .count(),
        0
    );
}

#[test]
fn opening_ignores_foreign_files_in_the_directory() {
    let dir = TempDir::new();
    std::fs::write(dir.path().join("notes.txt"), "hello").expect("write foreign file");
    std::fs::write(dir.path().join("export-hello.json"), "{}").expect("write foreign file");

    let store = ExportStore::open(dir.path(), ExportRetentionPolicy::bounded(4, 4_096))
        .expect("open store");

    assert_eq!(store.retained_count(), 0);
    assert!(dir.path().join("notes.txt").exists());
    assert!(dir.path().join("export-hello.json").exists());
}

#[test]
fn opening_rejects_a_non_directory_path() {
    let dir = TempDir::new();
    let file = dir.path().join("not-a-directory");
    std::fs::write(&file, "x").expect("write file");

    assert!(matches!(
        ExportStore::open(&file, ExportRetentionPolicy::bounded(4, 4_096)),
        Err(ExportStoreError::NotADirectory { .. })
    ));
}

#[test]
fn sequence_exhaustion_fails_closed_after_recovery() {
    let dir = TempDir::new();
    std::fs::write(dir.path().join(export_file_name(u64::MAX)), "x")
        .expect("write max sequence file");

    let mut store = ExportStore::open(dir.path(), ExportRetentionPolicy::bounded(8, 4_096))
        .expect("open store");

    assert_eq!(store.next_sequence(), u64::MAX);
    assert!(matches!(
        store.store("x"),
        Err(ExportStoreError::SequenceExhausted)
    ));
}

#[test]
fn atomic_write_leaves_no_pending_file() {
    let dir = TempDir::new();
    let mut store = ExportStore::open(dir.path(), ExportRetentionPolicy::bounded(4, 4_096))
        .expect("open store");

    store.store("bundle").expect("store bundle");

    assert!(!dir.path().join(pending_file_name(0)).exists());
    assert_eq!(store.read(0).expect("read bundle"), "bundle");
}
