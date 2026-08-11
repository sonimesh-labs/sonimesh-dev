//! Deterministic property coverage for the bounded encoded jitter buffer.
//!
//! The jitter buffer is a realtime admission boundary: every in-order sequence
//! must reach playout exactly once, gaps must be reported for PLC, late and
//! duplicate frames must be rejected, and a bounded capacity collision is the
//! only stream-local resynchronization trigger. This test drives interleaved
//! insert/playout operations with a fixed pseudo-random sequence mix and
//! compares every outcome against an independent slot-and-set model, so
//! ordering, collision, or exhaustion bugs cannot silently change the
//! admission contract.

use std::collections::BTreeSet;

use sonimesh_audio::{EncodedFrame, InsertOutcome, JitterBuffer, PlayoutOutcome};

const CAPACITY: usize = 8;
const MAX_PAYLOAD: usize = 8;

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

fn frame(sequence: u32) -> EncodedFrame<MAX_PAYLOAD> {
    EncodedFrame::new(
        sequence,
        u64::from(sequence) * 960,
        &[sequence.to_le_bytes()[0]],
    )
    .unwrap()
}

/// Independent reference model using an explicit sequence set instead of a
/// fixed slot array, so slot-residue bugs in the implementation cannot be
/// replicated by construction.
#[derive(Default)]
struct JitterModel {
    buffered: BTreeSet<u32>,
    next: Option<u32>,
    highest: Option<u32>,
    playout_started: bool,
    exhausted: bool,
}

impl JitterModel {
    fn insert(&mut self, sequence: u32) -> InsertOutcome {
        if self.exhausted {
            return InsertOutcome::SequenceExhausted;
        }
        if self.next.is_some_and(|next| sequence < next) {
            return InsertOutcome::Late;
        }

        let reordered = self.highest.is_some_and(|highest| sequence < highest);
        let slot_occupied = self
            .buffered
            .iter()
            .any(|&buffered| buffered as usize % CAPACITY == sequence as usize % CAPACITY);
        if slot_occupied {
            if self.buffered.contains(&sequence) {
                return InsertOutcome::Duplicate;
            }
            return self.resynchronize(sequence);
        }
        if self.buffered.len() == CAPACITY {
            return self.resynchronize(sequence);
        }

        if self.next.is_none() {
            self.next = Some(sequence);
        }
        self.buffered.insert(sequence);
        self.highest = Some(
            self.highest
                .map_or(sequence, |highest| highest.max(sequence)),
        );
        if reordered {
            InsertOutcome::Reordered
        } else {
            InsertOutcome::Inserted
        }
    }

    fn take_next(&mut self) -> ModelPlayout {
        let Some(sequence) = self.next else {
            return ModelPlayout::NotReady;
        };
        if self.buffered.is_empty() {
            if !self.playout_started {
                return ModelPlayout::NotReady;
            }
            self.advance(sequence);
            return ModelPlayout::Missing { sequence };
        }
        if self.buffered.contains(&sequence) {
            self.buffered.remove(&sequence);
            self.advance(sequence);
            self.playout_started = true;
            return ModelPlayout::Frame(sequence);
        }
        self.advance(sequence);
        self.playout_started = true;
        ModelPlayout::Missing { sequence }
    }

    fn resynchronize(&mut self, sequence: u32) -> InsertOutcome {
        let dropped_frames = self.buffered.len();
        self.buffered.clear();
        self.next = Some(sequence);
        self.highest = Some(sequence);
        self.playout_started = true;
        self.exhausted = false;
        self.buffered.insert(sequence);
        InsertOutcome::Resynchronized { dropped_frames }
    }

    fn advance(&mut self, sequence: u32) {
        self.next = sequence.checked_add(1);
        self.exhausted = self.next.is_none();
    }
}

#[derive(Debug, Eq, PartialEq)]
enum ModelPlayout {
    Frame(u32),
    Missing { sequence: u32 },
    NotReady,
}

fn playout_shape(outcome: PlayoutOutcome<MAX_PAYLOAD>) -> ModelPlayout {
    match outcome {
        PlayoutOutcome::Frame(frame) => ModelPlayout::Frame(frame.sequence),
        PlayoutOutcome::Missing { sequence } => ModelPlayout::Missing { sequence },
        PlayoutOutcome::NotReady => ModelPlayout::NotReady,
    }
}

/// Draws a sequence near `anchor` so the bounded window mixes in-order inserts,
/// reorders, duplicates, late frames, and residue collisions.
#[allow(clippy::cast_possible_truncation)]
fn random_sequence(rng: &mut XorShift64, anchor: u32) -> u32 {
    match rng.next_u32() % 3 {
        0 => anchor.saturating_sub(rng.next_usize(CAPACITY * 4) as u32),
        1 => anchor.saturating_add(rng.next_usize(CAPACITY * 4) as u32),
        _ => anchor.saturating_add(rng.next_usize(512) as u32),
    }
}

#[test]
fn insert_and_playout_match_a_model_over_random_sequences() {
    let mut rng = XorShift64(0xAB12_CD34_EF56_7890);
    let mut buffer = JitterBuffer::<CAPACITY, MAX_PAYLOAD>::new();
    let mut model = JitterModel::default();
    let mut counts = [0_u32; 5];

    for _ in 0..100_000 {
        if rng.next_u32() % 10 < 6 {
            let anchor = model.next.or(model.highest).unwrap_or(0);
            let sequence = random_sequence(&mut rng, anchor);
            let model_outcome = model.insert(sequence);
            let buffer_outcome = buffer.insert(frame(sequence));
            assert_eq!(buffer_outcome, model_outcome, "insert of {sequence}");
            match model_outcome {
                InsertOutcome::Inserted => counts[0] += 1,
                InsertOutcome::Reordered => counts[1] += 1,
                InsertOutcome::Duplicate => counts[2] += 1,
                InsertOutcome::Late => counts[3] += 1,
                InsertOutcome::Resynchronized { .. } => counts[4] += 1,
                InsertOutcome::SequenceExhausted | InsertOutcome::Disabled => {}
            }
        } else {
            let model_outcome = model.take_next();
            let buffer_outcome = playout_shape(buffer.take_next());
            assert_eq!(buffer_outcome, model_outcome, "take_next");
        }
        assert_eq!(buffer.len(), model.buffered.len(), "buffered count drift");
    }
    assert!(
        counts.iter().all(|&count| count > 0),
        "the fixed sequence mix must exercise every admission outcome: {counts:?}"
    );
}

#[test]
fn negotiated_start_playout_matches_a_model() {
    let start = 1_000_u32;
    let mut rng = XorShift64(0xDECA_FBAD_1234_5678);
    let mut buffer = JitterBuffer::<CAPACITY, MAX_PAYLOAD>::with_start_sequence(start);
    let mut model = JitterModel {
        next: Some(start),
        playout_started: true,
        ..JitterModel::default()
    };

    for _ in 0..20_000 {
        if rng.next_u32() % 2 == 0 {
            let anchor = model
                .next
                .expect("negotiated start keeps a playout position");
            let sequence = random_sequence(&mut rng, anchor);
            let model_outcome = model.insert(sequence);
            let buffer_outcome = buffer.insert(frame(sequence));
            assert_eq!(buffer_outcome, model_outcome, "insert of {sequence}");
        } else {
            let model_outcome = model.take_next();
            let buffer_outcome = playout_shape(buffer.take_next());
            assert_eq!(buffer_outcome, model_outcome, "take_next");
        }
        assert_eq!(buffer.len(), model.buffered.len(), "buffered count drift");
    }
}
