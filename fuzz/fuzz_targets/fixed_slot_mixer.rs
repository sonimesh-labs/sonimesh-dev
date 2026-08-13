#![no_main]

//! Fuzz the fixed-slot realtime mixer and its membership transitions.
//!
//! Arbitrary insert/remove/render/push/gain scripts drive the preallocated
//! slot table and its per-slot PCM consumers. The mixer must never panic,
//! must fail membership changes closed at the fixed bounds, and must keep
//! render output finite while producers carry finite samples and the
//! published gains stay within the validated linear range.

use libfuzzer_sys::fuzz_target;
use sonimesh_audio::{FixedSlotMixer, GainControlHandle, MixerError, realtime_pcm_queue};

const SLOTS: usize = 4;
const SAMPLES_PER_BLOCK: usize = 64;
const QUEUE_CAPACITY_BLOCKS: usize = 16;
const MAX_OUTPUT_SAMPLES: usize = 512;

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    let mut mixer = FixedSlotMixer::<SLOTS, SAMPLES_PER_BLOCK>::new()
        .expect("fixed slot and block counts are non-zero");
    let mut producers: Vec<Option<sonimesh_audio::RealtimePcmProducer<SAMPLES_PER_BLOCK>>> =
        (0..SLOTS).map(|_| None).collect();
    let mut handles: Vec<Option<GainControlHandle>> = (0..SLOTS).map(|_| None).collect();

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
        let op = data[offset] % 5;
        let parameter = usize::from(data[offset + 1]);
        offset += 2;
        match op {
            0 => {
                // Activate one stream at the lowest free slot.
                let (producer, consumer) =
                    realtime_pcm_queue::<SAMPLES_PER_BLOCK>(QUEUE_CAPACITY_BLOCKS)
                        .expect("fixed queue capacity is valid");
                let handle = GainControlHandle::new();
                match mixer.insert(consumer, handle.clone(), None) {
                    Ok(index) => {
                        assert!(index < SLOTS && producers[index].is_none());
                        producers[index] = Some(producer);
                        handles[index] = Some(handle);
                    }
                    Err(MixerError::SlotFull) => {
                        assert_eq!(mixer.active(), SLOTS);
                    }
                    Err(_) => unreachable!("insert only fails on SlotFull"),
                }
            }
            1 => {
                // Remove an arbitrary slot, including out-of-range indices.
                let index = parameter % (SLOTS + 2);
                if index >= SLOTS {
                    assert!(matches!(
                        mixer.remove(index),
                        Err(MixerError::SlotOutOfRange)
                    ));
                } else if producers[index].is_none() {
                    assert!(matches!(mixer.remove(index), Err(MixerError::SlotInactive)));
                } else {
                    assert!(mixer.remove(index).is_ok());
                    producers[index] = None;
                    handles[index] = None;
                }
            }
            2 => {
                // Render an output buffer of arbitrary length.
                let len = parameter % (MAX_OUTPUT_SAMPLES + 1);
                let output = &mut rendered[..len];
                mixer.render(output);
                assert!(output.iter().all(|sample| sample.is_finite()));
            }
            3 => {
                // Push one finite block into an active producer.
                if let Some(producer) = producers[parameter % SLOTS].as_mut() {
                    let len = 1 + parameter % (SAMPLES_PER_BLOCK * 2);
                    let _ = producer.push(&scratch[..len]);
                }
            }
            4 => {
                // Publish a bounded gain or mute change to one active slot.
                if let Some(handle) = handles[parameter % SLOTS].as_ref() {
                    if parameter % 2 == 0 {
                        let _ = handle.set_gain(0.25 * f32::from(data[offset % data.len()]) / 64.0);
                    } else {
                        handle.set_muted(parameter % 4 == 1);
                    }
                }
            }
            _ => unreachable!("op is the remainder of a u8 modulo 5"),
        }
    }
});
