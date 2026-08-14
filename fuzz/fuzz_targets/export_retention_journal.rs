#![no_main]

//! Fuzz the bounded export retention journal (ADR 0011).
//!
//! `ExportRetentionJournal` keeps the newest export receipts by evicting the
//! oldest record until the fixed count and byte bounds hold, and rejects a
//! single bundle that cannot fit the byte bound. This target drives a
//! deterministic edge prologue and then arbitrary record-size scripts through
//! a real journal against an independent spec model that computes the retained
//! set as the longest newest-first suffix satisfying both bounds (instead of
//! an eviction loop) and sums bytes from the retained list (instead of
//! tracking a running total), comparing the result, retained count, retained
//! bytes, and next sequence after every step so a bookkeeping or
//! eviction-order bug in the real implementation cannot be shared by the
//! model.
//!
//! The model mirrors the seeded property walk
//! (`crates/sonimesh-app/tests/diagnostic_export_journal_property.rs`) so the
//! deterministic and byte-driven paths assert the same contract.

use libfuzzer_sys::fuzz_target;
use sonimesh_app::{
    DiagnosticExportActionError, ExportReceipt, ExportRetentionJournal, ExportRetentionPolicy,
};

/// Reads one byte from `data`, wrapping around the slice.
fn next_byte(data: &[u8], offset: &mut usize) -> u8 {
    let byte = data[*offset % data.len()];
    *offset += 1;
    byte
}

/// Reads one little-endian `u64` from `data`, wrapping around the slice.
fn next_u64(data: &[u8], offset: &mut usize) -> u64 {
    let mut bytes = [0_u8; 8];
    for byte in &mut bytes {
        *byte = next_byte(data, offset);
    }
    u64::from_le_bytes(bytes)
}

/// Independent spec model of the retention journal.
///
/// The real journal appends and evicts from the front of a `VecDeque` while
/// tracking a running byte total. This model instead computes the retained set
/// as the longest newest-first suffix that satisfies both bounds and sums the
/// bytes from the retained list, so a bookkeeping or eviction-order bug in the
/// real implementation cannot be shared by the model.
struct JournalModel {
    records: Vec<ExportReceipt>,
    next_sequence: u64,
    max_retained_exports: usize,
    max_retained_bytes: usize,
}

impl JournalModel {
    fn new(policy: ExportRetentionPolicy) -> Self {
        Self {
            records: Vec::new(),
            next_sequence: 0,
            max_retained_exports: policy.max_retained_exports,
            max_retained_bytes: policy.max_retained_bytes,
        }
    }

    fn retained_bytes(&self) -> usize {
        self.records.iter().map(|receipt| receipt.bytes).sum()
    }

    fn record(&mut self, bytes: usize) -> Result<ExportReceipt, DiagnosticExportActionError> {
        if self.next_sequence == u64::MAX {
            return Err(DiagnosticExportActionError::SequenceExhausted);
        }
        if bytes > self.max_retained_bytes {
            return Err(
                DiagnosticExportActionError::ExportBytesExceedRetentionLimit {
                    bytes,
                    max_retained_bytes: self.max_retained_bytes,
                },
            );
        }
        let receipt = ExportReceipt {
            sequence: self.next_sequence,
            bytes,
        };
        self.next_sequence += 1;

        // Longest newest-first suffix of the old records that fits beside the
        // new record under both bounds.
        let mut newest_first = vec![receipt];
        let mut total = bytes;
        for older in self.records.iter().rev() {
            if newest_first.len() >= self.max_retained_exports {
                break;
            }
            if total + older.bytes > self.max_retained_bytes {
                break;
            }
            total += older.bytes;
            newest_first.push(*older);
        }
        newest_first.reverse();
        self.records = newest_first;
        Ok(receipt)
    }
}

/// Branch counters accumulated across the walk.
#[derive(Default)]
struct WalkCounters {
    accepted: usize,
    oversized: usize,
    evicted: usize,
}

struct JournalWalk {
    journal: ExportRetentionJournal,
    model: JournalModel,
    counters: WalkCounters,
}

impl JournalWalk {
    fn new(policy: ExportRetentionPolicy) -> Self {
        Self {
            journal: ExportRetentionJournal::new(policy).expect("valid policy"),
            model: JournalModel::new(policy),
            counters: WalkCounters::default(),
        }
    }

    fn assert_state(&self, context: &str) {
        assert_eq!(
            self.journal.retained_count(),
            self.model.records.len(),
            "retained count after {context}"
        );
        assert_eq!(
            self.journal.retained_bytes(),
            self.model.retained_bytes(),
            "retained bytes after {context}"
        );
        assert_eq!(
            self.journal.next_sequence(),
            self.model.next_sequence,
            "next sequence after {context}"
        );
        assert!(
            self.journal.retained_count() <= self.journal.policy().max_retained_exports,
            "count bound after {context}"
        );
        assert!(
            self.journal.retained_bytes() <= self.journal.policy().max_retained_bytes,
            "byte bound after {context}"
        );
        for pair in self.model.records.windows(2) {
            assert!(
                pair[0].sequence < pair[1].sequence,
                "FIFO sequence order after {context}"
            );
        }
    }

    fn step_record(&mut self, bytes: usize) {
        let records_before = self.model.records.len();
        let real = self.journal.record(bytes);
        let expected = self.model.record(bytes);
        assert_eq!(real, expected, "record({bytes})");
        self.assert_state("record");
        match &real {
            Ok(_) => {
                self.counters.accepted += 1;
                if self.model.records.len() < records_before + 1 {
                    self.counters.evicted += 1;
                }
            }
            Err(DiagnosticExportActionError::ExportBytesExceedRetentionLimit { .. }) => {
                self.counters.oversized += 1;
            }
            Err(DiagnosticExportActionError::SequenceExhausted) => {
                unreachable!("the walk cannot exhaust the u64 sequence space");
            }
            Err(_) => {
                unreachable!("record cannot report policy or bundle failures");
            }
        }
    }
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    let mut offset = 0;
    let max_retained_exports = 1 + (next_u64(data, &mut offset) % 64) as usize;
    let max_retained_bytes = 1 + (next_u64(data, &mut offset) % 2_048) as usize;
    let policy = ExportRetentionPolicy::bounded(max_retained_exports, max_retained_bytes);
    let mut harness = JournalWalk::new(policy);

    // Deterministic edge coverage on every input: an oversized bundle is
    // rejected before any record, an exact-bound bundle is accepted, a second
    // exact-bound bundle forces eviction (by count when the bound is one,
    // otherwise by bytes), and zero-byte and unit-byte bundles are accepted.
    harness.step_record(max_retained_bytes + 1);
    harness.step_record(max_retained_bytes);
    harness.step_record(max_retained_bytes);
    harness.step_record(0);
    harness.step_record(1);

    // Fuzz-derived record sizes under the input-selected policy shape.
    let steps = 1 + data.len() % 64;
    for _ in 0..steps {
        let bytes = match next_u64(data, &mut offset) % 16 {
            0 => 0,
            1 => max_retained_bytes,
            2 => max_retained_bytes + 1,
            3 => max_retained_bytes * 2 + 1,
            4 => 1,
            5 => max_retained_bytes / 2,
            _ => 1 + (next_u64(data, &mut offset) % max_retained_bytes as u64) as usize,
        };
        harness.step_record(bytes);
    }

    assert!(
        harness.counters.accepted > 0,
        "the prologue must accept at least one record"
    );
    assert!(
        harness.counters.oversized > 0,
        "the prologue must reject an oversized bundle"
    );
    assert!(
        harness.counters.evicted > 0,
        "the prologue must evict the oldest record"
    );
});
