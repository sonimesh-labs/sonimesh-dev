#![no_main]

//! Fuzz the authenticated replay window (security boundary).
//!
//! The replay window is the packet-dedup gate between AEAD authentication and
//! decoder admission: it must accept each in-window sequence exactly once and
//! reject duplicates and out-of-window sequences before any plaintext reaches a
//! decoder. This target drives the window with deterministic u32 and
//! window-width edge sequences plus fuzz-derived forward, reorder, duplicate,
//! and far-behind operations, comparing every decision against an independent
//! set-based model so bit-rotation or window-width bugs cannot silently change
//! the admission contract.

use std::collections::HashSet;

use libfuzzer_sys::fuzz_target;
use sonimesh_protocol::{ReplayDecision, ReplayWindow};

/// Width of the authenticated replay window mirrored from the protocol crate.
const WINDOW_BITS: u32 = 64;

/// A window plus an independent set-based admission model.
struct WindowModel {
    window: ReplayWindow,
    accepted: HashSet<u32>,
    accepted_order: Vec<u32>,
    highest: Option<u32>,
}

impl WindowModel {
    fn new() -> Self {
        Self {
            window: ReplayWindow::new(),
            accepted: HashSet::new(),
            accepted_order: Vec::new(),
            highest: None,
        }
    }

    /// Drives one sequence and asserts the window matches the independent model.
    fn drive(&mut self, sequence: u32) {
        let decision = self.window.accept_authenticated(sequence);
        match decision {
            ReplayDecision::Accepted => {
                assert!(
                    !self.accepted.contains(&sequence),
                    "sequence {sequence} was accepted twice"
                );
                if self.highest.is_none_or(|highest| sequence > highest) {
                    self.highest = Some(sequence);
                }
                self.accepted.insert(sequence);
                self.accepted_order.push(sequence);
            }
            ReplayDecision::Duplicate => {
                let highest = self.highest.expect("duplicate requires an active window");
                let distance = highest - sequence;
                assert!(
                    distance < WINDOW_BITS,
                    "duplicate {sequence} is {distance} behind {highest}"
                );
                assert!(
                    self.accepted.contains(&sequence),
                    "duplicate {sequence} was never accepted"
                );
            }
            ReplayDecision::TooOld => {
                let highest = self.highest.expect("too-old requires an active window");
                let distance = highest - sequence;
                assert!(
                    distance >= WINDOW_BITS,
                    "too-old {sequence} is only {distance} behind {highest}"
                );
            }
        }
    }

    /// Replays a recently accepted sequence when one exists.
    fn replay_recent(&mut self, recency: u32) {
        if self.accepted_order.is_empty() {
            return;
        }
        let len = self.accepted_order.len();
        let index = len - 1 - (recency as usize % len.min(128));
        let sequence = self.accepted_order[index];
        self.drive(sequence);
    }
}

fuzz_target!(|data: &[u8]| {
    let mut model = WindowModel::new();

    // Deterministic edge coverage on every run: u32 boundaries and the exact
    // window-width boundary near a small highest sequence.
    for sequence in [0, 1, 63, 64, 65, u32::MAX - 1, u32::MAX] {
        model.drive(sequence);
    }

    // Fuzz-derived operations. Each operation consumes five bytes: one opcode
    // plus a big-endian 32-bit payload.
    let mut offset = 0;
    while offset + 5 <= data.len() {
        let op = data[offset] % 4;
        let payload = u32::from_be_bytes([
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
            data[offset + 4],
        ]);
        offset += 5;
        match op {
            // Arbitrary forward or reordered sequence, including huge jumps.
            0 => model.drive(payload),
            // In-window reorder near the current highest.
            1 => {
                let highest = model.highest.unwrap_or(payload);
                model.drive(highest.saturating_sub(payload % WINDOW_BITS));
            }
            // Far behind the current highest, outside the window.
            2 => {
                let highest = model.highest.unwrap_or(payload);
                model.drive(highest.saturating_sub(WINDOW_BITS + payload % 10_000));
            }
            // Duplicate of a recently accepted sequence.
            _ => model.replay_recent(payload),
        }
    }
});
