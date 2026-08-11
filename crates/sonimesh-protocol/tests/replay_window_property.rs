//! Deterministic property coverage for the authenticated replay window.
//!
//! The replay window is a security boundary: it must accept each in-window
//! sequence exactly once and reject duplicates and out-of-window sequences
//! before any plaintext reaches a decoder. This test drives the window with a
//! fixed pseudo-random sequence mix and compares every decision against an
//! independent set-based model so bit-rotation or window-width bugs cannot
//! silently change the admission contract.

use std::collections::HashSet;

use sonimesh_protocol::{ReplayDecision, ReplayWindow};

/// Width of the authenticated replay window mirrored from the protocol crate.
const WINDOW_BITS: u32 = 64;

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
}

#[test]
fn replay_decisions_match_an_independent_in_window_model() {
    let mut rng = XorShift64(0x5EED_1234_5678_9ABC);
    let mut window = ReplayWindow::new();
    let mut accepted = HashSet::new();
    let mut accepted_order = Vec::new();
    let mut highest: Option<u32> = None;

    for _ in 0..100_000 {
        let roll = rng.next_u32() % 8;
        let sequence = match roll {
            // Arbitrary sequence, including large forward jumps.
            0..=3 => rng.next_u32(),
            // In-window reorder near the current highest.
            4..=5 => match highest {
                Some(highest) => highest.saturating_sub(rng.next_u32() % WINDOW_BITS),
                None => rng.next_u32(),
            },
            // Duplicate of a recently accepted sequence.
            6 => {
                let len = accepted_order.len();
                if len == 0 {
                    rng.next_u32()
                } else {
                    let recent = len.min(128);
                    accepted_order[len - 1 - (rng.next_u32() as usize % recent)]
                }
            }
            // Far behind the highest, outside the replay window.
            _ => match highest {
                Some(highest) => highest.saturating_sub(WINDOW_BITS + 1 + rng.next_u32() % 10_000),
                None => rng.next_u32(),
            },
        };

        let decision = window.accept_authenticated(sequence);
        match decision {
            ReplayDecision::Accepted => {
                if let Some(highest) = highest {
                    if sequence <= highest {
                        let distance = highest - sequence;
                        assert!(
                            distance < WINDOW_BITS,
                            "in-window sequence {sequence} is {distance} behind {highest}"
                        );
                        assert!(
                            !accepted.contains(&sequence),
                            "in-window sequence {sequence} was accepted twice"
                        );
                    }
                }
                if highest.is_none_or(|highest| sequence > highest) {
                    highest = Some(sequence);
                }
                if accepted.insert(sequence) {
                    accepted_order.push(sequence);
                }
            }
            ReplayDecision::Duplicate => {
                let highest = highest.expect("duplicate requires an active window");
                let distance = highest - sequence;
                assert!(
                    distance < WINDOW_BITS,
                    "duplicate {sequence} is {distance} behind {highest}"
                );
                assert!(
                    accepted.contains(&sequence),
                    "duplicate {sequence} was never accepted"
                );
            }
            ReplayDecision::TooOld => {
                let highest = highest.expect("too-old requires an active window");
                let distance = highest - sequence;
                assert!(
                    distance >= WINDOW_BITS,
                    "too-old {sequence} is only {distance} behind {highest}"
                );
            }
        }
    }
}
