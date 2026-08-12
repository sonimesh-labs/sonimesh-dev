//! Deterministic property coverage for the command-driven mixer boundary.
//!
//! ADR 0015 requires membership changes published by the control domain to
//! take effect at callback granularity through a bounded lock-free queue: the
//! callback drains pending commands in FIFO order at the start of render, an
//! insert occupies the lowest free slot, a stale remove is tolerated, and a
//! full queue fails closed. This test drives a fixed pseudo-random sequence of
//! published insert/remove commands and renders against an independent model
//! that tracks the pending queue, the active-set membership, and each slot's
//! expected realtime counter events, and asserts the observable mixer state,
//! the mixed output, and the per-slot counters agree at every step.

use std::collections::VecDeque;
use std::sync::Arc;

use sonimesh_audio::{
    CommandDrivenMixer, GainControlHandle, MAX_MIXER_COMMAND_QUEUE, MixerMembershipError,
    RealtimePcmProducer, RealtimePcmPushOutcome, realtime_pcm_queue,
};
use sonimesh_diagnostics::StreamCounters;

const SLOTS: usize = 4;
const SAMPLES_PER_BLOCK: usize = 8;
const TOLERANCE: f32 = 1e-6;

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

/// Distinct constant value each slot contributes when fed and active.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn value_for_slot(slot: usize) -> f32 {
    (slot + 1) as f32
}

/// One membership command waiting in the modeled bounded queue.
enum PendingCommand<const SAMPLES_PER_BLOCK: usize> {
    Insert {
        producer: RealtimePcmProducer<SAMPLES_PER_BLOCK>,
        counters: Arc<StreamCounters>,
    },
    Remove {
        slot: usize,
    },
}

/// Asserts the observable membership state matches the independent model.
fn assert_membership<const N: usize>(
    mixer: &CommandDrivenMixer<SLOTS, N>,
    active: [bool; SLOTS],
    step: u32,
) {
    assert_eq!(
        mixer.active(),
        active.iter().filter(|present| **present).count(),
        "step {step}: active slot count diverged"
    );
    for (index, expected) in active.iter().enumerate() {
        assert_eq!(
            mixer.is_active(index),
            *expected,
            "step {step}: slot {index} membership diverged"
        );
    }
}

/// Drives random membership commands through the bounded handle against an
/// independent model, then applies them with a drain and renders. Every active
/// slot is fed the same whole-block count and, half the time, one over-run
/// chunk, so each active slot underflows once on that tail chunk. The model
/// tracks each slot's expected underflow events, and the per-slot counters
/// must agree with it while the mixed output stays the active-slot sum.
#[allow(clippy::similar_names, clippy::too_many_lines)]
#[test]
fn random_membership_commands_keep_render_correct() {
    let mut rng = XorShift64(0x6A1E_0000_2026_000D);
    let mut mixer: CommandDrivenMixer<SLOTS, SAMPLES_PER_BLOCK> =
        CommandDrivenMixer::new().unwrap();
    let membership = mixer.membership();
    let mut pending: VecDeque<PendingCommand<SAMPLES_PER_BLOCK>> = VecDeque::new();
    let mut active: [bool; SLOTS] = [false; SLOTS];
    let mut producers: [Option<RealtimePcmProducer<SAMPLES_PER_BLOCK>>; SLOTS] =
        std::array::from_fn(|_| None);
    let mut counters: [Option<Arc<StreamCounters>>; SLOTS] = std::array::from_fn(|_| None);
    let mut expected_underflows: [u64; SLOTS] = [0; SLOTS];

    for step in 0..20_000_u32 {
        match rng.next_u32() % 3 {
            0 => {
                // Publish one stream activation at the lowest free slot.
                if pending.len() == MAX_MIXER_COMMAND_QUEUE {
                    let (_, consumer) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(16).unwrap();
                    assert_eq!(
                        membership.insert(consumer, GainControlHandle::new(), None),
                        Err(MixerMembershipError::CommandQueueFull),
                        "step {step}: a full queue must fail closed"
                    );
                } else {
                    let (producer, consumer) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(16).unwrap();
                    let slot_counters = Arc::new(StreamCounters::default());
                    membership
                        .insert(
                            consumer,
                            GainControlHandle::new(),
                            Some(Arc::clone(&slot_counters)),
                        )
                        .unwrap();
                    pending.push_back(PendingCommand::Insert {
                        producer,
                        counters: slot_counters,
                    });
                }
            }
            1 => {
                // Publish one stream deactivation, occasionally out of range.
                let slot = rng.next_usize(SLOTS + 2);
                if slot >= SLOTS {
                    assert_eq!(
                        membership.remove(slot),
                        Err(MixerMembershipError::SlotOutOfRange),
                        "step {step}: an out-of-range remove must fail fast"
                    );
                } else if pending.len() == MAX_MIXER_COMMAND_QUEUE {
                    assert_eq!(
                        membership.remove(slot),
                        Err(MixerMembershipError::CommandQueueFull),
                        "step {step}: a full queue must fail closed"
                    );
                } else {
                    membership.remove(slot).unwrap();
                    pending.push_back(PendingCommand::Remove { slot });
                }
            }
            _ => {
                // Apply every pending command FIFO to the model and the mixer.
                while let Some(command) = pending.pop_front() {
                    match command {
                        PendingCommand::Insert {
                            producer,
                            counters: slot_counters,
                        } => {
                            if let Some(slot) = (0..SLOTS).find(|&index| !active[index]) {
                                active[slot] = true;
                                producers[slot] = Some(producer);
                                counters[slot] = Some(slot_counters);
                                expected_underflows[slot] = 0;
                            }
                            // A full table drops the activation, like the
                            // callback does, without blocking other streams.
                        }
                        PendingCommand::Remove { slot } => {
                            if active[slot] {
                                active[slot] = false;
                                producers[slot] = None;
                                counters[slot] = None;
                            }
                            // A stale remove is tolerated like the callback.
                        }
                    }
                }
                mixer.drain_pending();
                assert_eq!(mixer.pending_commands(), 0);
                assert_membership(&mixer, active, step);

                // Feed every active slot the same whole-block count.
                let blocks = 1 + rng.next_usize(3);
                for (slot, present) in active.iter().enumerate() {
                    if *present {
                        let value = value_for_slot(slot);
                        let producer = producers[slot].as_mut().unwrap();
                        for _ in 0..blocks {
                            assert_eq!(
                                producer.push(&[value; SAMPLES_PER_BLOCK]),
                                RealtimePcmPushOutcome::Inserted
                            );
                        }
                    }
                }

                // Render the fed blocks plus, half the time, one over-run
                // chunk that makes every active slot underflow once.
                let over_run = rng.next_u32() % 2 == 0;
                let rendered_blocks = blocks + usize::from(over_run);
                let expected: f32 = active
                    .iter()
                    .enumerate()
                    .filter(|(_, present)| **present)
                    .map(|(slot, _)| value_for_slot(slot))
                    .sum();
                let mut output = vec![0.0; rendered_blocks * SAMPLES_PER_BLOCK];
                mixer.render(&mut output);
                let expected_blocks = blocks * SAMPLES_PER_BLOCK;
                for (index, sample) in output.iter().enumerate() {
                    let block_expected = if index < expected_blocks {
                        expected
                    } else {
                        0.0
                    };
                    assert!(
                        (*sample - block_expected).abs() <= TOLERANCE,
                        "step {step}: sample {index} = {sample}, expected {block_expected}"
                    );
                }

                for slot in 0..SLOTS {
                    if active[slot] {
                        if over_run {
                            expected_underflows[slot] += 1;
                        }
                        let snapshot = counters[slot].as_ref().unwrap().snapshot();
                        assert_eq!(
                            snapshot.pcm_underflows, expected_underflows[slot],
                            "step {step}: slot {slot} underflow count diverged"
                        );
                        assert_eq!(snapshot.pcm_refill_limit_hits, 0);
                        assert_eq!(snapshot.pcm_discontinuities, 0);
                    }
                }
                assert_membership(&mixer, active, step);
            }
        }
    }
}
