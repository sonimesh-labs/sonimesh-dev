#![no_main]

//! Fuzz the command-driven mixer membership boundary.
//!
//! Arbitrary publish/render/push/gain scripts drive the bounded lock-free
//! command queue shared between a [`MixerMembershipHandle`] and the callback
//! [`CommandDrivenMixer`]. The mixer must never panic, must apply published
//! commands at callback granularity in FIFO order with the lowest-free-slot
//! policy, must fail closed at the fixed slot bound and on out-of-range
//! removes, and must keep render output finite while producers carry finite
//! samples and published gains stay within the validated linear range. Every
//! observable mixer state is compared against an independent FIFO model of
//! the same pending queue and slot table.

use std::collections::VecDeque;

use libfuzzer_sys::fuzz_target;
use sonimesh_audio::{
    CommandDrivenMixer, GainControlHandle, MAX_MIXER_COMMAND_QUEUE, MixerMembershipError,
    RealtimePcmProducer, realtime_pcm_queue,
};

const SLOTS: usize = 4;
const SAMPLES_PER_BLOCK: usize = 64;
const QUEUE_CAPACITY_BLOCKS: usize = 16;
const MAX_OUTPUT_SAMPLES: usize = 512;

/// One membership command waiting in the modeled bounded FIFO queue.
enum PendingCommand<const SAMPLES_PER_BLOCK: usize> {
    Insert {
        producer: RealtimePcmProducer<SAMPLES_PER_BLOCK>,
        gain: GainControlHandle,
    },
    Remove {
        slot: usize,
    },
}

/// Applies the modeled FIFO queue to the modeled slot table, mirroring
/// `CommandDrivenMixer::drain_pending` with the lowest-free-slot policy and
/// dropping commands that cannot be applied (for example an insert while every
/// slot is occupied).
fn drain_model<const SAMPLES_PER_BLOCK: usize>(
    pending: &mut VecDeque<PendingCommand<SAMPLES_PER_BLOCK>>,
    slots: &mut [Option<(RealtimePcmProducer<SAMPLES_PER_BLOCK>, GainControlHandle)>],
) {
    while let Some(command) = pending.pop_front() {
        match command {
            PendingCommand::Insert { producer, gain } => {
                if let Some(empty) = slots.iter_mut().find(|slot| slot.is_none()) {
                    *empty = Some((producer, gain));
                }
            }
            PendingCommand::Remove { slot } => {
                if let Some(target) = slots.get_mut(slot) {
                    *target = None;
                }
            }
        }
    }
}

/// Asserts the observable mixer state matches the independent FIFO model.
fn assert_state<const SAMPLES_PER_BLOCK: usize>(
    mixer: &CommandDrivenMixer<SLOTS, SAMPLES_PER_BLOCK>,
    slots: &[Option<(RealtimePcmProducer<SAMPLES_PER_BLOCK>, GainControlHandle)>],
    pending: &VecDeque<PendingCommand<SAMPLES_PER_BLOCK>>,
) {
    let active = slots.iter().filter(|slot| slot.is_some()).count();
    assert_eq!(
        mixer.active(),
        active,
        "active slot count diverged from the model"
    );
    assert_eq!(
        mixer.pending_commands(),
        pending.len(),
        "pending command count diverged from the model"
    );
    for (index, slot) in slots.iter().enumerate() {
        assert_eq!(
            mixer.is_active(index),
            slot.is_some(),
            "slot {index} membership diverged from the model"
        );
    }
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    let mut mixer = CommandDrivenMixer::<SLOTS, SAMPLES_PER_BLOCK>::new()
        .expect("fixed slot and block counts are non-zero");
    let membership = mixer.membership();
    let mut pending: VecDeque<PendingCommand<SAMPLES_PER_BLOCK>> = VecDeque::new();
    let mut slots: Vec<Option<(RealtimePcmProducer<SAMPLES_PER_BLOCK>, GainControlHandle)>> =
        (0..SLOTS).map(|_| None).collect();

    // Deterministic finite samples in [1.0, 2.0): the mixer sums what the
    // producers carry, so finite producer data must yield finite output.
    let mut scratch = [0.0f32; MAX_OUTPUT_SAMPLES];
    for index in 0..scratch.len() {
        let first = u32::from(data[index % data.len()]);
        let second = u32::from(data[(index + 3) % data.len()]);
        scratch[index] = f32::from_bits(0x3f80_0000 | (first << 15) | (second << 7));
    }

    let mut rendered = [0.0f32; MAX_OUTPUT_SAMPLES];
    let mut offset = 0;
    while offset + 1 < data.len() {
        let op = data[offset] % 6;
        let parameter = usize::from(data[offset + 1]);
        offset += 2;
        match op {
            0 => {
                // Publish one stream activation at the lowest free slot.
                let (producer, consumer) =
                    realtime_pcm_queue::<SAMPLES_PER_BLOCK>(QUEUE_CAPACITY_BLOCKS)
                        .expect("fixed queue capacity is valid");
                let gain = GainControlHandle::new();
                match membership.insert(consumer, gain.clone(), None) {
                    Ok(()) => pending.push_back(PendingCommand::Insert { producer, gain }),
                    Err(MixerMembershipError::CommandQueueFull) => {
                        assert_eq!(membership.pending_commands(), MAX_MIXER_COMMAND_QUEUE);
                    }
                    Err(_) => unreachable!("insert only fails on CommandQueueFull"),
                }
            }
            1 => {
                // Publish a deactivation for an arbitrary slot, including
                // out-of-range indices that must fail fast.
                let slot = parameter % (SLOTS + 2);
                match membership.remove(slot) {
                    Ok(()) => pending.push_back(PendingCommand::Remove { slot }),
                    Err(MixerMembershipError::SlotOutOfRange) => {
                        assert!(slot >= SLOTS);
                    }
                    Err(MixerMembershipError::CommandQueueFull) => {
                        assert_eq!(membership.pending_commands(), MAX_MIXER_COMMAND_QUEUE);
                    }
                }
            }
            2 => {
                // Render one callback block; all pending commands apply first.
                let len = parameter % (MAX_OUTPUT_SAMPLES + 1);
                let output = &mut rendered[..len];
                mixer.render(output);
                drain_model(&mut pending, &mut slots);
                assert_state(&mixer, &slots, &pending);
                assert!(output.iter().all(|sample| sample.is_finite()));
            }
            3 => {
                // Push one finite block into an active producer.
                if let Some((producer, _)) = slots[parameter % SLOTS].as_mut() {
                    let len = 1 + parameter % (SAMPLES_PER_BLOCK * 2);
                    let _ = producer.push(&scratch[..len]);
                }
            }
            4 => {
                // Publish a bounded gain or mute change to an active slot.
                if let Some((_, gain)) = slots[parameter % SLOTS].as_mut() {
                    if parameter % 2 == 0 {
                        let value = 0.25 * f32::from(data[offset % data.len()]) / 64.0;
                        let _ = gain.set_gain(value);
                    } else {
                        gain.set_muted(parameter % 4 == 1);
                    }
                }
            }
            5 => {
                // Advance membership without rendering.
                mixer.drain_pending();
                drain_model(&mut pending, &mut slots);
                assert_state(&mixer, &slots, &pending);
            }
            _ => unreachable!("op is the remainder of a u8 modulo 6"),
        }
    }
});
