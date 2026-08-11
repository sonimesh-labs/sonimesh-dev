//! Deterministic property coverage for the authenticated UDP source pin.
//!
//! The source pin is a fail-closed security boundary: protected processing may
//! run only for the endpoint pinned by the first successful outcome, a changed
//! endpoint must be rejected before any protected work, failed processing must
//! never change the binding, and only a fresh-session clear permits a new
//! source to bind. This test drives a fixed pseudo-random interleaving of
//! processing attempts and clears against an independent model and asserts
//! every outcome, the closure side-effect guard, and the pinned endpoint agree.

use std::cell::Cell;
use std::net::SocketAddr;

use sonimesh_network::{SourceEndpointAdmissionError, SourceEndpointPin};

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

fn endpoints() -> [SocketAddr; 4] {
    [
        "192.0.2.1:4000".parse().unwrap(),
        "192.0.2.2:4001".parse().unwrap(),
        "192.0.2.3:4002".parse().unwrap(),
        "192.0.2.4:4003".parse().unwrap(),
    ]
}

/// Independent reference model with the same fail-closed rules, so the fixed
/// slot bookkeeping of the implementation cannot be replicated by construction.
#[derive(Clone, Copy, Debug, Default)]
struct PinModel {
    pinned: Option<SocketAddr>,
}

#[derive(Debug, Eq, PartialEq)]
enum ModelOutcome {
    Accepted,
    ProcessingFailed,
    Rejected,
}

impl PinModel {
    fn process(&mut self, source: SocketAddr, succeeds: bool) -> ModelOutcome {
        if self.pinned.is_some_and(|expected| expected != source) {
            return ModelOutcome::Rejected;
        }
        if succeeds {
            self.pinned.get_or_insert(source);
            ModelOutcome::Accepted
        } else {
            ModelOutcome::ProcessingFailed
        }
    }

    fn clear(&mut self) {
        self.pinned = None;
    }
}

#[test]
fn source_pin_never_accepts_a_changed_endpoint_or_mutates_on_failure() {
    let mut rng = XorShift64(0x51A7_CCE5_2026_0001);
    let mut pin = SourceEndpointPin::new();
    let mut model = PinModel::default();
    let pool = endpoints();

    let mut counts = [0_u32; 4];

    for _ in 0..100_000 {
        let roll = rng.next_u32() % 8;
        if roll == 7 {
            pin.clear_for_fresh_session();
            model.clear();
            counts[3] += 1;
        } else {
            let source = pool[rng.next_usize(pool.len())];
            let succeeds = roll % 2 == 0;
            let called = Cell::new(false);
            let result = pin.process(source, || {
                called.set(true);
                if succeeds {
                    Ok::<u32, &str>(1)
                } else {
                    Err("rejected")
                }
            });
            let expected = model.process(source, succeeds);
            match (result, expected) {
                (Ok(_), ModelOutcome::Accepted) => {
                    counts[0] += 1;
                    assert!(called.get(), "matching endpoint runs protected work");
                }
                (
                    Err(SourceEndpointAdmissionError::SourceEndpointChanged),
                    ModelOutcome::Rejected,
                ) => {
                    counts[1] += 1;
                    assert!(
                        !called.get(),
                        "changed endpoint must never run protected work"
                    );
                }
                (
                    Err(SourceEndpointAdmissionError::Processing(_)),
                    ModelOutcome::ProcessingFailed,
                ) => {
                    counts[2] += 1;
                    assert!(called.get(), "matching endpoint runs protected work");
                }
                (result, expected) => {
                    panic!("outcome mismatch: {result:?} vs {expected:?}");
                }
            }
        }
        assert_eq!(pin.pinned(), model.pinned, "pinned endpoint drift");
    }
    assert!(
        counts.iter().all(|&count| count > 0),
        "the fixed sequence mix must exercise every pin outcome: {counts:?}"
    );
}
