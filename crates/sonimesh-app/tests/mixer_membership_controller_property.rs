//! Deterministic property coverage for the mixer membership controller.
//!
//! ADR 0015 requires the control-domain admission boundary to keep a
//! deterministic projection of the callback's fixed slot table. Seeded random
//! walks drive fast-path and adapted admissions, removals, finite producer
//! pushes, and bounded gain/mute changes through a `MixerMembershipController`
//! sharing one `CommandDrivenMixer`, then compare the controller projection
//! with the mixer's actual slot table after every drain: active counts and
//! per-slot membership must agree, pending command counts must agree and
//! drain to zero, fail-closed admission (format mismatch, table full,
//! out-of-range remove) must leave both sides unchanged, and render output
//! must stay finite while producers carry finite samples.

use std::sync::Arc;

use sonimesh_app::{
    MixerAdmissionError, MixerMembershipController, SourcePcmFormat, StreamFormatAdapter,
};
use sonimesh_audio::{
    CommandDrivenMixer, GainControlHandle, MAX_MIXER_COMMAND_QUEUE, MAX_PCM_QUEUE_BLOCKS,
    RealtimePcmProducer,
};
use sonimesh_diagnostics::StreamCounters;

const SLOTS: usize = 4;
const BLOCK: usize = 64;
const QUEUE_CAPACITY_BLOCKS: usize = 8;
const STEPS: usize = 3_000;
const SEEDS: &[u64] = &[1, 2, 3, 4, 5, 6];

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

    fn next_u8(&mut self) -> u8 {
        self.next_u32().to_le_bytes()[0]
    }
}

fn output_format() -> SourcePcmFormat {
    SourcePcmFormat::new(48_000, 2).expect("fixed output format is valid")
}

fn mismatch_format() -> SourcePcmFormat {
    SourcePcmFormat::new(44_100, 2).expect("fixed mismatch format is valid")
}

fn adapted_source_format() -> SourcePcmFormat {
    SourcePcmFormat::new(48_000, 1).expect("fixed adapted source format is valid")
}

/// Deterministic finite samples in [1.0, 2.0): finite producer data must
/// yield finite mixed output.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn finite_block(value: f32) -> [f32; BLOCK] {
    [1.0 + 0.5 * (value / 255.0); BLOCK]
}

/// Asserts the controller projection matches the actual mixer slot table.
fn assert_projection<const N: usize>(
    mixer: &CommandDrivenMixer<SLOTS, N>,
    controller: &MixerMembershipController<SLOTS, N>,
    step: u32,
) {
    assert_eq!(
        mixer.active(),
        controller.active(),
        "step {step}: active count diverged from the controller projection"
    );
    assert_eq!(
        mixer.pending_commands(),
        controller.pending_commands(),
        "step {step}: pending command count diverged"
    );
    for slot in 0..SLOTS {
        assert_eq!(
            mixer.is_active(slot),
            controller.is_active(slot),
            "step {step}: slot {slot} membership diverged from the controller projection"
        );
    }
}

/// Runs one seeded walk and asserts the projection invariant after every drain.
#[allow(clippy::too_many_lines)]
fn walk(seed: u64, step_limit: usize) {
    let mut rng = XorShift64(seed);
    let mut mixer =
        CommandDrivenMixer::<SLOTS, BLOCK>::new().expect("fixed mixer configuration is valid");
    let membership = mixer.membership();
    let mut controller =
        MixerMembershipController::new(membership, QUEUE_CAPACITY_BLOCKS, output_format())
            .expect("fixed controller configuration is valid");

    let mut producers: Vec<Option<RealtimePcmProducer<BLOCK>>> = (0..SLOTS).map(|_| None).collect();
    let mut gain_handles: Vec<Option<GainControlHandle>> = (0..SLOTS).map(|_| None).collect();
    let mut adapted: Vec<
        Option<(
            RealtimePcmProducer<BLOCK>,
            StreamFormatAdapter<BLOCK, BLOCK>,
        )>,
    > = (0..SLOTS).map(|_| None).collect();

    let mut output = [0.0f32; BLOCK];
    let step_limit = u32::try_from(step_limit).expect("step limit fits u32");
    let mut step = 0_u32;
    while step < step_limit {
        step += 1;
        let op = rng.next_u8() % 8;
        match op {
            0 => {
                // Fast-path admission of an output-format stream.
                let gain = GainControlHandle::new();
                let counters = Some(Arc::new(StreamCounters::default()));
                match controller.admit_stream(output_format(), gain.clone(), counters) {
                    Ok(mut admitted) => {
                        assert!(
                            producers[admitted.slot].is_none() && adapted[admitted.slot].is_none(),
                            "an admitted slot must be fresh"
                        );
                        let value = f32::from(rng.next_u8());
                        let _ = admitted.producer.push(&finite_block(value));
                        producers[admitted.slot] = Some(admitted.producer);
                        gain_handles[admitted.slot] = Some(gain);
                    }
                    Err(MixerAdmissionError::SlotTableFull) => {
                        assert_eq!(controller.active(), SLOTS);
                    }
                    Err(MixerAdmissionError::CommandQueueFull) => {
                        assert_eq!(controller.pending_commands(), MAX_MIXER_COMMAND_QUEUE);
                    }
                    Err(other) => panic!("step {step}: unexpected admission error {other:?}"),
                }
            }
            1 => {
                // A format mismatch must fail closed without reserving a slot.
                let before = controller.active();
                assert!(matches!(
                    controller.admit_stream(mismatch_format(), GainControlHandle::new(), None),
                    Err(MixerAdmissionError::FormatMismatch)
                ));
                assert_eq!(controller.active(), before);
            }
            2 => {
                // Deactivate an arbitrary slot, including out-of-range indices.
                let slot = rng.next_usize(SLOTS + 1);
                match controller.remove_stream(slot) {
                    Ok(()) => {
                        producers[slot] = None;
                        gain_handles[slot] = None;
                        adapted[slot] = None;
                    }
                    Err(MixerAdmissionError::SlotOutOfRange) => {
                        assert!(slot >= SLOTS);
                    }
                    Err(MixerAdmissionError::CommandQueueFull) => {
                        assert_eq!(controller.pending_commands(), MAX_MIXER_COMMAND_QUEUE);
                    }
                    Err(other) => panic!("step {step}: unexpected removal error {other:?}"),
                }
            }
            3 => {
                // Render one callback block: every pending command applies,
                // then the projection must match the actual mixer exactly.
                mixer.render(&mut output);
                assert!(
                    output.iter().all(|sample| sample.is_finite()),
                    "step {step}: render output became non-finite"
                );
                assert_eq!(mixer.pending_commands(), 0);
                assert_projection(&mixer, &controller, step);
            }
            4 => {
                // Adapted admission of a mono source converted to the output
                // format; the adapter is pumped alongside later renders.
                let gain = GainControlHandle::new();
                match controller.admit_adapted_stream::<BLOCK>(
                    adapted_source_format(),
                    QUEUE_CAPACITY_BLOCKS,
                    gain.clone(),
                    None,
                ) {
                    Ok(mut admitted) => {
                        assert!(
                            producers[admitted.slot].is_none() && adapted[admitted.slot].is_none(),
                            "an admitted slot must be fresh"
                        );
                        let value = f32::from(rng.next_u8());
                        let _ = admitted.source_producer.push(&finite_block(value));
                        adapted[admitted.slot] = Some((admitted.source_producer, admitted.adapter));
                        gain_handles[admitted.slot] = Some(gain);
                    }
                    Err(MixerAdmissionError::SlotTableFull) => {
                        assert_eq!(controller.active(), SLOTS);
                    }
                    Err(MixerAdmissionError::CommandQueueFull) => {
                        assert_eq!(controller.pending_commands(), MAX_MIXER_COMMAND_QUEUE);
                    }
                    Err(other) => {
                        panic!("step {step}: unexpected adapted admission error {other:?}")
                    }
                }
            }
            5 => {
                // Feed one active slot with finite audio: fast-path producers
                // directly, adapted streams through their source producer and
                // a pump step.
                let slot = rng.next_usize(SLOTS);
                if let Some(producer) = producers[slot].as_mut() {
                    let value = f32::from(rng.next_u8());
                    let _ = producer.push(&finite_block(value));
                } else if let Some((source_producer, adapter)) = adapted[slot].as_mut() {
                    let value = f32::from(rng.next_u8());
                    let _ = source_producer.push(&finite_block(value));
                    let _ = adapter.pump();
                }
            }
            6 => {
                // Publish a bounded gain or mute change to one active slot.
                if let Some(gain) = gain_handles[rng.next_usize(SLOTS)].as_mut() {
                    if rng.next_u8() % 2 == 0 {
                        let value = f32::from(rng.next_u8()) / 255.0;
                        let _ = gain.set_gain(value);
                    } else {
                        gain.set_muted(rng.next_u8() % 4 == 1);
                    }
                }
            }
            7 => {
                // Advance membership without rendering, then assert sync.
                mixer.drain_pending();
                assert_eq!(mixer.pending_commands(), 0);
                assert_projection(&mixer, &controller, step);
            }
            _ => unreachable!("op is the remainder of a u8 modulo 8"),
        }
    }

    // A final drain leaves the projection consistent on every seed.
    mixer.drain_pending();
    assert_projection(&mixer, &controller, step);
}

#[test]
fn controller_projection_matches_the_mixer_after_every_drain() {
    for &seed in SEEDS {
        walk(seed, STEPS);
    }
}

#[test]
fn controller_rejects_an_oversized_queue_capacity_fail_closed() {
    let mixer = CommandDrivenMixer::<SLOTS, BLOCK>::new().unwrap();
    let membership = mixer.membership();
    assert!(matches!(
        MixerMembershipController::<SLOTS, BLOCK>::new(
            membership,
            MAX_PCM_QUEUE_BLOCKS + 1,
            output_format()
        ),
        Err(MixerAdmissionError::Queue(
            sonimesh_audio::RealtimeQueueConfigError::CapacityTooLarge
        ))
    ));
}
