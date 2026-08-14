#![no_main]

//! Fuzz the realtime PCM render source composition.
//!
//! `PcmRenderSource` is the only consumer-facing endpoint on the output
//! callback path: it copies queued PCM blocks across callback boundaries,
//! preserves a partial block's cursor, caps refill attempts per callback,
//! emits bounded silence on underflow, fades back in after a queue
//! discontinuity, and ramps a published per-stream gain or mute target across
//! blocks. This target drives a real source and a real lock-free queue
//! against an independent model of the queue-plus-render composition under a
//! byte-driven operation walk and compares every rendered output slice and
//! every published stream counter after each step, so adversarial queue,
//! epoch, render, gain, and mute scripts cannot panic, allocate past the
//! fixed bounds, or diverge from the modeled output.
//!
//! The model mirrors the seeded property walk
//! (`crates/sonimesh-app/tests/pcm_render_source_property.rs`) so the
//! deterministic and byte-driven paths assert the same contract.

use std::collections::VecDeque;
use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use sonimesh_app::{GainControlHandle, PcmRenderSource};
use sonimesh_audio::{
    MAX_PCM_QUEUE_BLOCKS, RealtimePcmProducer, RealtimePcmPushOutcome, realtime_pcm_queue,
};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::RenderSource;

const SAMPLES_PER_BLOCK: usize = 4;
const QUEUE_CAPACITY: usize = MAX_PCM_QUEUE_BLOCKS;
const MAX_RENDER_LEN: usize = 256;
const DISCONTINUITY_FADE_SAMPLES: u16 = 128;
const GAIN_RAMP_SAMPLES: u16 = 128;

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

/// Pure block payload shared by the real producer and the model walk.
fn block_samples(block_id: u64, count: usize) -> Vec<f32> {
    let base = f32::from(((block_id * 7) % 251) as u16);
    (0..count)
        .map(|index| base + f32::from(u16::try_from(index).expect("small sample offset")))
        .collect()
}

/// Independent model of the bounded SPSC PCM queue's consumer-visible behavior.
struct QueueModel {
    blocks: VecDeque<BlockModel>,
    capacity: usize,
    active_epoch: u64,
    next_sequence: u64,
    expected: (u64, u64),
}

struct BlockModel {
    epoch: u64,
    sequence: u64,
    samples: Vec<f32>,
}

/// One consumer-visible pop result modeled without the real queue's atomics.
enum PopModel {
    Filled {
        samples: Vec<f32>,
        discontinuity: bool,
    },
    Underflow,
}

impl QueueModel {
    fn new(capacity: usize) -> Self {
        Self {
            blocks: VecDeque::new(),
            capacity,
            active_epoch: 0,
            next_sequence: 0,
            expected: (0, 0),
        }
    }

    fn push(&mut self, samples: Vec<f32>) -> RealtimePcmPushOutcome {
        let replaced = self.blocks.len() == self.capacity;
        if replaced {
            self.blocks.pop_front();
        }
        self.blocks.push_back(BlockModel {
            epoch: self.active_epoch,
            sequence: self.next_sequence,
            samples,
        });
        self.next_sequence = self.next_sequence.wrapping_add(1);
        if replaced {
            RealtimePcmPushOutcome::ReplacedOldest
        } else {
            RealtimePcmPushOutcome::Inserted
        }
    }

    fn begin_epoch(&mut self) {
        self.active_epoch = self.active_epoch.wrapping_add(1);
        self.next_sequence = 0;
    }

    /// Pops one callback-visible block, discarding retired-epoch blocks first.
    fn pop_into(&mut self) -> PopModel {
        for _ in 0..MAX_PCM_QUEUE_BLOCKS {
            let Some(block) = self.blocks.pop_front() else {
                return PopModel::Underflow;
            };
            if block.epoch != self.active_epoch {
                continue;
            }
            let actual = (block.epoch, block.sequence);
            let discontinuity = actual != self.expected;
            self.expected = (block.epoch, block.sequence.wrapping_add(1));
            return PopModel::Filled {
                samples: block.samples,
                discontinuity,
            };
        }
        PopModel::Underflow
    }

    fn len(&self) -> usize {
        self.blocks.len()
    }
}

/// Independent linear fade ramp with the same arithmetic shape as `GainSmoother`.
struct FadeModel {
    current: f32,
    target: f32,
    step: f32,
    remaining: u16,
}

impl FadeModel {
    fn constant(gain: f32) -> Self {
        Self {
            current: gain,
            target: gain,
            step: 0.0,
            remaining: 0,
        }
    }

    fn restart_ramp_up(&mut self) {
        self.current = 0.0;
        self.target = 1.0;
        self.step = 1.0 / f32::from(DISCONTINUITY_FADE_SAMPLES);
        self.remaining = DISCONTINUITY_FADE_SAMPLES;
    }

    fn apply_one(&mut self) -> f32 {
        if self.remaining > 0 {
            self.current += self.step;
            self.remaining -= 1;
            if self.remaining == 0 {
                self.current = self.target;
            }
        }
        self.current
    }
}

/// Independent linear gain ramp with the same arithmetic shape as `GainSmoother`.
struct GainModel {
    current: f32,
    target: f32,
    step: f32,
    remaining: u16,
}

impl GainModel {
    fn constant(gain: f32) -> Self {
        Self {
            current: gain,
            target: gain,
            step: 0.0,
            remaining: 0,
        }
    }

    fn set_target(&mut self, target: f32, ramp_samples: u16) {
        self.target = target;
        self.step = (target - self.current) / f32::from(ramp_samples);
        self.remaining = ramp_samples;
    }

    fn apply_one(&mut self) -> f32 {
        if self.remaining > 0 {
            self.current += self.step;
            self.remaining -= 1;
            if self.remaining == 0 {
                self.current = self.target;
            }
        }
        self.current
    }
}

/// Independent model of the queue-plus-render-source composition.
struct RenderModel {
    queue: QueueModel,
    scratch: Vec<f32>,
    fade: FadeModel,
    gain: GainModel,
    gain_target: f32,
    muted: bool,
    underflows: usize,
    discontinuities: usize,
    refill_limit_hits: usize,
}

impl RenderModel {
    fn new(queue_capacity: usize) -> Self {
        Self {
            queue: QueueModel::new(queue_capacity),
            scratch: Vec::new(),
            fade: FadeModel::constant(1.0),
            gain: GainModel::constant(1.0),
            gain_target: 1.0,
            muted: false,
            underflows: 0,
            discontinuities: 0,
            refill_limit_hits: 0,
        }
    }

    fn set_gain(&mut self, gain: f32) {
        self.gain_target = gain;
    }

    fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    /// Restarts the gain ramp only when the published target bit pattern
    /// changes, mirroring the realtime source's no-reset rule.
    fn sync_gain_target(&mut self) {
        let target = if self.muted { 0.0 } else { self.gain_target };
        if target.to_bits() != self.gain.target.to_bits() {
            self.gain.set_target(target, GAIN_RAMP_SAMPLES);
        }
    }

    fn render(&mut self, out_len: usize) -> Vec<f32> {
        let mut output = vec![0.0; out_len];
        let mut written = 0;
        let mut refills = 0;
        while written < out_len {
            if self.scratch.is_empty() {
                if refills == MAX_PCM_QUEUE_BLOCKS {
                    self.refill_limit_hits += 1;
                    break;
                }
                refills += 1;
                match self.queue.pop_into() {
                    PopModel::Filled {
                        samples,
                        discontinuity,
                    } => {
                        if discontinuity {
                            self.discontinuities += 1;
                            self.fade.restart_ramp_up();
                        }
                        self.scratch = samples;
                        if self.scratch.is_empty() {
                            continue;
                        }
                    }
                    PopModel::Underflow => {
                        self.underflows += 1;
                        break;
                    }
                }
            }
            self.sync_gain_target();
            let copy_len = self.scratch.len().min(out_len - written);
            for index in 0..copy_len {
                let fade_factor = self.fade.apply_one();
                let gain_factor = self.gain.apply_one();
                output[written + index] = self.scratch[index] * fade_factor * gain_factor;
            }
            written += copy_len;
            self.scratch.drain(..copy_len);
        }
        output
    }
}

/// Real queue-plus-source composition plus its independent model.
struct RenderHarness {
    producer: RealtimePcmProducer<SAMPLES_PER_BLOCK>,
    source: PcmRenderSource<SAMPLES_PER_BLOCK>,
    gain_handle: GainControlHandle,
    model: RenderModel,
    counters: Arc<StreamCounters>,
    block_id: u64,
}

impl RenderHarness {
    fn new() -> Self {
        let counters = Arc::new(StreamCounters::default());
        let gain_handle = GainControlHandle::new();
        let (producer, consumer) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(QUEUE_CAPACITY)
            .expect("fixed queue capacity is valid");
        Self {
            producer,
            source: PcmRenderSource::new(consumer, Arc::clone(&counters), gain_handle.clone()),
            gain_handle,
            model: RenderModel::new(QUEUE_CAPACITY),
            counters,
            block_id: 0,
        }
    }

    /// Publishes one bounded linear gain and mirrors it in the independent model.
    fn set_gain(&mut self, gain: f32) {
        self.gain_handle
            .set_gain(gain)
            .expect("fuzz gains stay within the bounded linear range");
        self.model.set_gain(gain);
    }

    /// Publishes one mute flag and mirrors it in the independent model.
    fn set_muted(&mut self, muted: bool) {
        self.gain_handle.set_muted(muted);
        self.model.set_muted(muted);
    }

    fn push_samples(&mut self, count: usize) {
        let samples = block_samples(self.block_id, count);
        self.block_id = self.block_id.wrapping_add(1);
        let outcome = self.producer.push(&samples);
        let model_outcome = self.model.queue.push(samples);
        assert_eq!(outcome, model_outcome, "push outcome");
        assert_eq!(
            self.producer.len(),
            self.model.queue.len(),
            "queue length after push"
        );
    }

    fn begin_epoch(&mut self) {
        self.producer.begin_epoch();
        self.model.queue.begin_epoch();
        assert_eq!(
            self.producer.len(),
            self.model.queue.len(),
            "queue length after epoch"
        );
    }

    fn render_len(&mut self, len: usize) {
        let mut output = vec![f32::NAN; len];
        self.source.render(&mut output);
        let expected = self.model.render(len);
        assert_close(&output, &expected, len);
        let snapshot = self.counters.snapshot();
        assert_eq!(
            snapshot.pcm_underflows, self.model.underflows as u64,
            "published underflow count"
        );
        assert_eq!(
            snapshot.pcm_discontinuities, self.model.discontinuities as u64,
            "published discontinuity count"
        );
        assert_eq!(
            snapshot.pcm_refill_limit_hits, self.model.refill_limit_hits as u64,
            "published refill-limit count"
        );
        assert_eq!(
            self.producer.len(),
            self.model.queue.len(),
            "queue length after render"
        );
    }
}

fn assert_close(actual: &[f32], expected: &[f32], out_len: usize) {
    assert_eq!(actual.len(), expected.len(), "rendered length");
    for (index, (actual_sample, expected_sample)) in actual.iter().zip(expected).enumerate() {
        let tolerance = 1.0e-4_f32;
        assert!(
            (actual_sample - expected_sample).abs() <= tolerance,
            "sample {index} of a {out_len}-sample render: got {actual_sample}, expected {expected_sample}"
        );
    }
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    let mut harness = RenderHarness::new();
    let mut offset = 0;

    // Deterministic edge coverage on every input: an empty-callback no-op, an
    // underflow, a normal drain, a discontinuity after an epoch retires queued
    // PCM, a refill-limit hit on a single oversized callback, and the gain,
    // unchanged-target no-reset, mute, and unmute ramp branches.
    harness.render_len(0);
    harness.render_len(1);
    harness.push_samples(1 + (next_u64(data, &mut offset) % SAMPLES_PER_BLOCK as u64) as usize);
    harness.render_len(SAMPLES_PER_BLOCK + 1);
    harness.push_samples(SAMPLES_PER_BLOCK);
    harness.begin_epoch();
    harness.push_samples(SAMPLES_PER_BLOCK);
    harness.render_len(SAMPLES_PER_BLOCK + 1);
    for _ in 0..=(MAX_PCM_QUEUE_BLOCKS + 1) {
        harness.push_samples(1);
    }
    harness.render_len(MAX_PCM_QUEUE_BLOCKS + 1);
    for _ in 0..MAX_PCM_QUEUE_BLOCKS {
        harness.push_samples(SAMPLES_PER_BLOCK);
    }
    harness.set_gain(0.5);
    harness.render_len(64);
    harness.set_gain(0.5); // unchanged target must not restart the ramp
    harness.render_len(64);
    harness.render_len(64); // completed ramp holds 0.5
    harness.set_muted(true);
    harness.render_len(128); // ramp to silence
    harness.render_len(64); // holds silence
    harness.set_muted(false);
    harness.render_len(128); // ramp back to 0.5
    harness.render_len(64); // queue drained; underflows without changing gain

    // Fuzz-derived operations with adversarial queue, epoch, render, gain, and
    // mute states.
    let steps = 1 + data.len() % 128;
    for _ in 0..steps {
        match next_byte(data, &mut offset) % 100 {
            0..=39 => {
                let count = 1 + (next_u64(data, &mut offset) % SAMPLES_PER_BLOCK as u64) as usize;
                harness.push_samples(count);
            }
            40..=49 => harness.begin_epoch(),
            50..=57 => {
                let numerator = u16::try_from(next_u64(data, &mut offset) % 65_536)
                    .expect("bounded gain fits u16");
                harness.set_gain(f32::from(numerator) / f32::from(u16::MAX));
            }
            58..=62 => harness.set_muted(!harness.model.muted),
            _ => {
                let len = (next_u64(data, &mut offset) % (MAX_RENDER_LEN as u64 + 1)) as usize;
                harness.render_len(len);
            }
        }
    }
});
