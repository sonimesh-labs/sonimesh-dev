#![no_main]

//! Fuzz the control-domain mixer membership controller (ADR 0015).
//!
//! `MixerMembershipController` is the app-side admission boundary over the
//! callback's fixed-slot `CommandDrivenMixer`: it publishes bounded commands
//! that the callback applies at the start of its next render and keeps a
//! deterministic projection of the callback's slot table. This target drives
//! arbitrary fast-path and adapted admissions, removals (including
//! out-of-range indices), finite producer pushes, bounded gain and mute
//! changes, renders, and drains, and asserts after every drain that the
//! controller projection matches the real mixer's slot table exactly, that
//! pending commands drain to zero, that fail-closed admission (format
//! mismatch, table full, command queue full) leaves both sides unchanged, and
//! that render output stays finite while producers carry finite samples.
//!
//! The walk mirrors the seeded property test
//! (`crates/sonimesh-app/tests/mixer_membership_controller_property.rs`) so
//! the deterministic and byte-driven paths assert the same contract.

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use sonimesh_app::{
    MixerAdmissionError, MixerMembershipController, SourcePcmFormat, StreamFormatAdapter,
};
use sonimesh_audio::{
    CommandDrivenMixer, GainControlHandle, MAX_MIXER_COMMAND_QUEUE, RealtimePcmProducer,
};
use sonimesh_diagnostics::StreamCounters;

const SLOTS: usize = 4;
const BLOCK: usize = 64;
const QUEUE_CAPACITY_BLOCKS: usize = 8;

/// One adapted slot: the source-format producer and its decode-domain adapter.
type AdaptedSlot = (
    RealtimePcmProducer<BLOCK>,
    StreamFormatAdapter<BLOCK, BLOCK>,
);

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
fn finite_block(value: u8) -> [f32; BLOCK] {
    [1.0 + 0.5 * (f32::from(value) / 255.0); BLOCK]
}

/// Drives the real controller and mixer and asserts the projection contract.
struct MembershipWalk {
    mixer: CommandDrivenMixer<SLOTS, BLOCK>,
    controller: MixerMembershipController<SLOTS, BLOCK>,
    producers: Vec<Option<RealtimePcmProducer<BLOCK>>>,
    gain_handles: Vec<Option<GainControlHandle>>,
    adapted: Vec<Option<AdaptedSlot>>,
    output: [f32; BLOCK],
}

impl MembershipWalk {
    fn new() -> Self {
        let mixer =
            CommandDrivenMixer::<SLOTS, BLOCK>::new().expect("fixed mixer configuration is valid");
        let membership = mixer.membership();
        let controller =
            MixerMembershipController::new(membership, QUEUE_CAPACITY_BLOCKS, output_format())
                .expect("fixed controller configuration is valid");
        Self {
            mixer,
            controller,
            producers: (0..SLOTS).map(|_| None).collect(),
            gain_handles: (0..SLOTS).map(|_| None).collect(),
            adapted: (0..SLOTS).map(|_| None).collect(),
            output: [0.0f32; BLOCK],
        }
    }

    /// Asserts the controller projection matches the actual mixer slot table.
    fn assert_projection(&self, context: &str) {
        assert_eq!(
            self.mixer.active(),
            self.controller.active(),
            "active count diverged after {context}"
        );
        assert_eq!(
            self.mixer.pending_commands(),
            self.controller.pending_commands(),
            "pending command count diverged after {context}"
        );
        for slot in 0..SLOTS {
            assert_eq!(
                self.mixer.is_active(slot),
                self.controller.is_active(slot),
                "slot {slot} membership diverged after {context}"
            );
        }
    }

    /// Applies every pending command through one callback render and asserts
    /// the projection and finite output.
    fn render(&mut self, context: &str) {
        self.mixer.render(&mut self.output);
        assert!(
            self.output.iter().all(|sample| sample.is_finite()),
            "render output became non-finite after {context}"
        );
        assert_eq!(
            self.mixer.pending_commands(),
            0,
            "pending commands must drain after {context}"
        );
        self.assert_projection(context);
    }

    /// Advances membership without rendering, then asserts the projection.
    fn drain(&mut self, context: &str) {
        self.mixer.drain_pending();
        assert_eq!(
            self.mixer.pending_commands(),
            0,
            "pending commands must drain after {context}"
        );
        self.assert_projection(context);
    }

    /// Fast-path admission of an output-format stream into the lowest free
    /// slot, with bookkeeping and a finite producer push on success.
    fn admit_fast(&mut self, value: u8) -> Result<(), MixerAdmissionError> {
        let gain = GainControlHandle::new();
        let counters = Some(Arc::new(StreamCounters::default()));
        let mut admitted = self
            .controller
            .admit_stream(output_format(), gain.clone(), counters)?;
        assert!(
            self.producers[admitted.slot].is_none() && self.adapted[admitted.slot].is_none(),
            "an admitted slot must be fresh"
        );
        let _ = admitted.producer.push(&finite_block(value));
        self.producers[admitted.slot] = Some(admitted.producer);
        self.gain_handles[admitted.slot] = Some(gain);
        Ok(())
    }

    /// Adapted admission of a mono source converted to the output format; the
    /// adapter is pumped alongside later renders.
    fn admit_adapted(&mut self, value: u8) -> Result<(), MixerAdmissionError> {
        let gain = GainControlHandle::new();
        let mut admitted = self.controller.admit_adapted_stream::<BLOCK>(
            adapted_source_format(),
            QUEUE_CAPACITY_BLOCKS,
            gain.clone(),
            None,
        )?;
        assert!(
            self.producers[admitted.slot].is_none() && self.adapted[admitted.slot].is_none(),
            "an admitted slot must be fresh"
        );
        let _ = admitted.source_producer.push(&finite_block(value));
        let _ = admitted.adapter.pump();
        self.adapted[admitted.slot] = Some((admitted.source_producer, admitted.adapter));
        self.gain_handles[admitted.slot] = Some(gain);
        Ok(())
    }

    /// A format mismatch must fail closed without reserving a slot.
    fn admit_mismatch(&mut self) {
        let before = self.controller.active();
        assert!(matches!(
            self.controller
                .admit_stream(mismatch_format(), GainControlHandle::new(), None),
            Err(MixerAdmissionError::FormatMismatch)
        ));
        assert_eq!(self.controller.active(), before);
    }

    /// Deactivates `slot`, dropping the stream's producer and gain handle.
    fn step_remove(&mut self, slot: usize) -> Result<(), MixerAdmissionError> {
        self.controller.remove_stream(slot)?;
        self.producers[slot] = None;
        self.gain_handles[slot] = None;
        self.adapted[slot] = None;
        Ok(())
    }

    /// Feeds one active slot with finite audio: fast-path producers directly,
    /// adapted streams through their source producer and a pump step.
    fn push_to_active(&mut self, slot: usize, value: u8) {
        if let Some(producer) = self.producers[slot].as_mut() {
            let _ = producer.push(&finite_block(value));
        } else if let Some((source_producer, adapter)) = self.adapted[slot].as_mut() {
            let _ = source_producer.push(&finite_block(value));
            let _ = adapter.pump();
        }
    }

    /// Publishes a bounded gain or mute change to one active slot.
    fn gain_change(&mut self, slot: usize, mode: u8, value: u8) {
        if let Some(gain) = self.gain_handles[slot].as_mut() {
            if mode.is_multiple_of(2) {
                let _ = gain.set_gain(f32::from(value) / 255.0);
            } else {
                gain.set_muted(mode % 4 == 1);
            }
        }
    }
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    let mut offset = 0;
    let mut harness = MembershipWalk::new();

    // Deterministic edge coverage on every input: fast-path and adapted
    // admissions, a fail-closed format mismatch, a successful remove, an
    // out-of-range remove, gain and mute changes, producer pushes, renders
    // and drains that apply every pending command, and a filled command
    // queue that returns the retryable error without reserving a slot.
    harness.admit_fast(7).expect("first fast admission");
    harness.admit_mismatch();
    harness.admit_adapted(9).expect("adapted admission");
    harness.render("edge: after admissions");
    harness.step_remove(0).expect("remove slot 0");
    assert!(matches!(
        harness.step_remove(SLOTS),
        Err(MixerAdmissionError::SlotOutOfRange)
    ));
    harness.render("edge: after remove");
    harness.gain_change(1, 2, 200);
    harness.push_to_active(1, 100);
    harness.drain("edge: after gain and push");

    // Fill the bounded command queue so the retryable full error is
    // exercised: three inserts, four removes, four reinserts, four more
    // removes, one insert, and one more command that must fail closed.
    for value in 0..3 {
        harness.admit_fast(value).expect("fill admission");
    }
    for slot in 0..SLOTS {
        harness.step_remove(slot).expect("fill remove");
    }
    for value in 0..4 {
        harness.admit_fast(value).expect("refill admission");
    }
    for slot in 0..SLOTS {
        harness.step_remove(slot).expect("refill remove");
    }
    harness.admit_fast(3).expect("sixteenth command");
    assert!(matches!(
        harness.admit_fast(4),
        Err(MixerAdmissionError::CommandQueueFull)
    ));
    assert_eq!(
        harness.controller.pending_commands(),
        MAX_MIXER_COMMAND_QUEUE,
        "the command queue must be full"
    );
    harness.render("edge: after queue full");

    // Fuzz-derived membership operations.
    let steps = 1 + data.len() % 64;
    for _ in 0..steps {
        match next_u64(data, &mut offset) % 8 {
            0 => match harness.admit_fast((next_u64(data, &mut offset) % 256) as u8) {
                Ok(()) => {}
                Err(MixerAdmissionError::SlotTableFull) => {
                    assert_eq!(harness.controller.active(), SLOTS);
                }
                Err(MixerAdmissionError::CommandQueueFull) => {
                    assert_eq!(
                        harness.controller.pending_commands(),
                        MAX_MIXER_COMMAND_QUEUE
                    );
                }
                Err(other) => panic!("unexpected admission error {other:?}"),
            },
            1 => harness.admit_mismatch(),
            2 => {
                let slot = next_u64(data, &mut offset) as usize % (SLOTS + 1);
                match harness.step_remove(slot) {
                    Ok(()) => {}
                    Err(MixerAdmissionError::SlotOutOfRange) => {
                        assert!(slot >= SLOTS);
                    }
                    Err(MixerAdmissionError::CommandQueueFull) => {
                        assert_eq!(
                            harness.controller.pending_commands(),
                            MAX_MIXER_COMMAND_QUEUE
                        );
                    }
                    Err(other) => panic!("unexpected removal error {other:?}"),
                }
            }
            3 => harness.render("step"),
            4 => match harness.admit_adapted((next_u64(data, &mut offset) % 256) as u8) {
                Ok(()) => {}
                Err(MixerAdmissionError::SlotTableFull) => {
                    assert_eq!(harness.controller.active(), SLOTS);
                }
                Err(MixerAdmissionError::CommandQueueFull) => {
                    assert_eq!(
                        harness.controller.pending_commands(),
                        MAX_MIXER_COMMAND_QUEUE
                    );
                }
                Err(other) => panic!("unexpected adapted admission error {other:?}"),
            },
            5 => {
                let slot = next_u64(data, &mut offset) as usize % SLOTS;
                let value = (next_u64(data, &mut offset) % 256) as u8;
                harness.push_to_active(slot, value);
            }
            6 => {
                let slot = next_u64(data, &mut offset) as usize % SLOTS;
                let mode = (next_u64(data, &mut offset) % 4) as u8;
                let value = (next_u64(data, &mut offset) % 256) as u8;
                harness.gain_change(slot, mode, value);
            }
            _ => harness.drain("step"),
        }
    }

    // A final drain leaves the projection consistent on every input.
    harness.drain("final");
});
