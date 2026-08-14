//! Deterministic property coverage for the realtime PCM render source.
//!
//! [`PcmRenderSource`] is the only consumer-facing endpoint on the output
//! callback path: it copies queued PCM blocks across callback boundaries,
//! preserves a partial block's cursor, caps refill attempts per callback,
//! emits bounded silence on underflow, fades back in after a queue
//! discontinuity, and ramps a published per-stream gain or mute target across
//! blocks. This test drives a real source and a real lock-free queue
//! against an independent model of the queue-plus-render composition under a
//! seeded operation walk, and compares every rendered output slice and every
//! published stream counter after each step.

use std::collections::VecDeque;
use std::sync::Arc;

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

    /// Uniform value in `0..bound` without narrowing casts in walk code.
    fn next_below(&mut self, bound: usize) -> usize {
        let bound_u32 = u32::try_from(bound).expect("walk bounds stay within u32");
        usize::try_from(self.next_u32() % bound_u32).expect("usize is at least 32 bits")
    }
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
    gain_ramps_started: usize,
    gain_ramp_renders: usize,
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
            gain_ramps_started: 0,
            gain_ramp_renders: 0,
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
            self.gain_ramps_started += 1;
        }
    }

    fn render(&mut self, out_len: usize) -> Vec<f32> {
        let mut output = vec![0.0; out_len];
        let mut written = 0;
        let mut refills = 0;
        let mut ramp_rendered = false;
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
                if self.gain.remaining > 0 {
                    ramp_rendered = true;
                }
                let fade_factor = self.fade.apply_one();
                let gain_factor = self.gain.apply_one();
                output[written + index] = self.scratch[index] * fade_factor * gain_factor;
            }
            written += copy_len;
            self.scratch.drain(..copy_len);
        }
        if ramp_rendered {
            self.gain_ramp_renders += 1;
        }
        output
    }
}

/// Branch coverage accumulated across the seeded walk.
#[derive(Default)]
struct WalkStats {
    pushes: usize,
    pushes_replaced: usize,
    epochs: usize,
    renders: usize,
    zero_renders: usize,
    underflows: usize,
    discontinuities: usize,
    refill_limit_hits: usize,
    gain_published: usize,
    mute_published: usize,
    gain_ramps_started: usize,
    gain_ramp_renders: usize,
}

struct Walk {
    producer: RealtimePcmProducer<SAMPLES_PER_BLOCK>,
    source: PcmRenderSource<SAMPLES_PER_BLOCK>,
    gain_handle: GainControlHandle,
    model: RenderModel,
    counters: Arc<StreamCounters>,
    rng: XorShift64,
    block_id: u64,
    stats: WalkStats,
}

impl Walk {
    /// Publishes one bounded linear gain and mirrors it in the independent model.
    fn step_set_gain(&mut self, gain: f32) {
        self.gain_handle
            .set_gain(gain)
            .expect("walk gains stay within the bounded linear range");
        self.model.set_gain(gain);
        self.stats.gain_published += 1;
    }

    /// Publishes one mute flag and mirrors it in the independent model.
    fn step_set_muted(&mut self, muted: bool) {
        self.gain_handle.set_muted(muted);
        self.model.set_muted(muted);
        self.stats.mute_published += 1;
    }

    /// Uniform bounded linear gain in `0.0..=1.0` without narrowing casts.
    fn random_gain(&mut self) -> f32 {
        let numerator = u16::try_from(self.rng.next_below(65_536)).expect("bounded gain fits u16");
        f32::from(numerator) / f32::from(u16::MAX)
    }

    fn step_push(&mut self) {
        let count = 1 + self.rng.next_below(SAMPLES_PER_BLOCK);
        self.push_samples(count);
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
        self.stats.pushes += 1;
        if outcome == RealtimePcmPushOutcome::ReplacedOldest {
            self.stats.pushes_replaced += 1;
        }
    }

    fn step_begin_epoch(&mut self) {
        self.producer.begin_epoch();
        self.model.queue.begin_epoch();
        assert_eq!(
            self.producer.len(),
            self.model.queue.len(),
            "queue length after epoch"
        );
        self.stats.epochs += 1;
    }

    fn step_render(&mut self) {
        let len = self.rng.next_below(MAX_RENDER_LEN + 1);
        self.render_len(len);
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
        self.stats.renders += 1;
        if len == 0 {
            self.stats.zero_renders += 1;
        }
        self.stats.underflows = self.model.underflows;
        self.stats.discontinuities = self.model.discontinuities;
        self.stats.refill_limit_hits = self.model.refill_limit_hits;
        self.stats.gain_ramps_started = self.model.gain_ramps_started;
        self.stats.gain_ramp_renders = self.model.gain_ramp_renders;
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

#[test]
fn seeded_render_walk_matches_an_independent_composition_model() {
    let counters = Arc::new(StreamCounters::default());
    let gain_handle = GainControlHandle::new();
    let (producer, consumer) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(QUEUE_CAPACITY).unwrap();
    let mut walk = Walk {
        producer,
        source: PcmRenderSource::new(consumer, Arc::clone(&counters), gain_handle.clone()),
        gain_handle,
        model: RenderModel::new(QUEUE_CAPACITY),
        counters,
        rng: XorShift64(0xC0FF_EE00_2026_0042),
        block_id: 0,
        stats: WalkStats::default(),
    };

    // Scripted prologue pins each branch: an empty-callback no-op, an underflow,
    // a normal drain, a discontinuity after an epoch retires queued PCM, and a
    // refill-limit hit on a single oversized callback.
    walk.render_len(0);
    walk.render_len(1);
    walk.step_push();
    walk.render_len(SAMPLES_PER_BLOCK + 1);
    walk.step_push();
    walk.step_begin_epoch();
    walk.step_push();
    walk.render_len(SAMPLES_PER_BLOCK + 1);
    for _ in 0..=(MAX_PCM_QUEUE_BLOCKS + 1) {
        walk.push_samples(1);
    }
    walk.render_len(MAX_PCM_QUEUE_BLOCKS + 1);

    // Gain prologue pins the ramp, unchanged-target no-reset, mute, and unmute
    // branches against the independent gain model. A full queue keeps real
    // samples flowing so each ramp actually scales rendered PCM.
    for _ in 0..MAX_PCM_QUEUE_BLOCKS {
        walk.push_samples(SAMPLES_PER_BLOCK);
    }
    walk.step_set_gain(0.5);
    walk.render_len(64);
    walk.step_set_gain(0.5); // unchanged target must not restart the ramp
    walk.render_len(64);
    walk.render_len(64); // completed ramp holds 0.5
    walk.step_set_muted(true);
    walk.render_len(128); // ramp to silence
    walk.render_len(64); // holds silence
    walk.step_set_muted(false);
    walk.render_len(128); // ramp back to 0.5
    walk.render_len(64); // queue drained; underflows without changing gain

    for _ in 0..300_000 {
        let operation = walk.rng.next_u32() % 100;
        if operation < 40 {
            walk.step_push();
        } else if operation < 50 {
            walk.step_begin_epoch();
        } else if operation < 58 {
            let gain = walk.random_gain();
            walk.step_set_gain(gain);
        } else if operation < 63 {
            walk.step_set_muted(!walk.model.muted);
        } else {
            walk.step_render();
        }
    }

    assert!(walk.stats.pushes > 0, "push was never exercised");
    assert!(
        walk.stats.pushes_replaced > 0,
        "oldest replacement was never exercised"
    );
    assert!(
        walk.stats.epochs > 0,
        "epoch retirement was never exercised"
    );
    assert!(walk.stats.renders > 0, "render was never exercised");
    assert!(
        walk.stats.zero_renders > 0,
        "empty callback was never exercised"
    );
    assert!(walk.stats.underflows > 0, "underflow was never exercised");
    assert!(
        walk.stats.discontinuities > 0,
        "discontinuity was never exercised"
    );
    assert!(
        walk.stats.refill_limit_hits > 0,
        "refill limit was never exercised"
    );
    assert!(
        walk.stats.gain_published > 0,
        "gain publish was never exercised"
    );
    assert!(
        walk.stats.mute_published > 0,
        "mute publish was never exercised"
    );
    assert!(
        walk.stats.gain_ramps_started > 0,
        "gain ramp was never started"
    );
    assert!(
        walk.stats.gain_ramp_renders > 0,
        "gain ramp never scaled rendered samples"
    );
}
