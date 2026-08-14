//! Deterministic property coverage for the export retention journal (ADR 0011).
//!
//! [`ExportRetentionJournal`] keeps the newest export receipts by evicting the
//! oldest record until the fixed count and byte bounds hold, and rejects a
//! single bundle that cannot fit the byte bound. This test walks a real
//! journal against an independent spec model that computes the retained set as
//! the longest newest-first suffix satisfying both bounds (instead of an
//! eviction loop) and sums bytes from the retained list (instead of tracking a
//! running total), then compares the count, total bytes, and next sequence
//! after every record.

use sonimesh_app::{
    DiagnosticExportActionError, ExportReceipt, ExportRetentionJournal, ExportRetentionPolicy,
};

/// Small deterministic xorshift64 generator; no external dependency.
struct XorShift64(u64);

impl XorShift64 {
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 32) as u32
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        self.next_u32() as usize % bound
    }
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

/// Branch counters accumulated across the seeded walk.
#[derive(Default)]
struct WalkCounters {
    accepted: usize,
    oversized: usize,
    evicted: usize,
}

#[allow(clippy::struct_field_names)]
struct Walk<'a> {
    journal: ExportRetentionJournal,
    model: JournalModel,
    rng: XorShift64,
    walk_counters: &'a mut WalkCounters,
}

impl Walk<'_> {
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

    fn step_record(&mut self) {
        let max_bytes = self.journal.policy().max_retained_bytes;
        let bytes = match self.rng.next_u32() % 10 {
            0 => 0,
            1 => max_bytes,
            2 => max_bytes + 1,
            3 => max_bytes * 2 + 1,
            _ => 1 + self.rng.next_usize(max_bytes),
        };
        let records_before = self.model.records.len();
        let real = self.journal.record(bytes);
        let expected = self.model.record(bytes);
        assert_eq!(real, expected, "record({bytes})");
        self.assert_state("record");
        match &real {
            Ok(_) => {
                self.walk_counters.accepted += 1;
                if self.model.records.len() < records_before + 1 {
                    self.walk_counters.evicted += 1;
                }
            }
            Err(DiagnosticExportActionError::ExportBytesExceedRetentionLimit { .. }) => {
                self.walk_counters.oversized += 1;
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

fn run_walk(seed: u64, policy: ExportRetentionPolicy, iterations: usize) -> WalkCounters {
    let mut walk_counters = WalkCounters::default();
    let mut walk = Walk {
        journal: ExportRetentionJournal::new(policy).expect("valid policy"),
        model: JournalModel::new(policy),
        rng: XorShift64(seed),
        walk_counters: &mut walk_counters,
    };

    for _ in 0..iterations {
        walk.step_record();
    }
    walk_counters
}

#[test]
fn seeded_journal_walk_matches_a_spec_model() {
    // One policy per walk, spanning count-dominant, byte-dominant, and tight
    // bounds so both eviction triggers and the exact-bound edge are exercised.
    let policies = [
        ExportRetentionPolicy::bounded(8, 1_000),
        ExportRetentionPolicy::bounded(1, 1_000),
        ExportRetentionPolicy::bounded(16, 50),
        ExportRetentionPolicy::bounded(2, 10),
        ExportRetentionPolicy::bounded(3, 3),
    ];

    for (index, policy) in policies.into_iter().enumerate() {
        let counters = run_walk(0xC0FF_EE00_2026_0050 + index as u64, policy, 250_000);
        assert!(
            counters.accepted > 0,
            "policy {policy:?}: accepted was never exercised"
        );
        assert!(
            counters.oversized > 0,
            "policy {policy:?}: oversized was never exercised"
        );
        assert!(
            counters.evicted > 0,
            "policy {policy:?}: eviction was never exercised"
        );
    }
}
