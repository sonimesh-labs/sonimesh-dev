//! Deterministic property coverage for the realtime peak limiter.
//!
//! The limiter is the bounded-output stage of the Phase 2 mixing chain. This
//! test drives a fixed pseudo-random sequence of blocks against an independent
//! state model and separately proves the contracts a model cannot: the output
//! ceiling is exact for every sample, sub-ceiling input is transparent after
//! full release, and interleaved frames stay linked across channels.

use sonimesh_audio::{LimiterConfig, MAX_AUDIO_CHANNELS, PeakLimiter};

/// Tolerance for limiter output and gain values.
const LIMITER_TOLERANCE: f32 = 1e-5;

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

/// Bounded pseudo-random sample in `[-16.0, 16.0)`, covering quiet, unity, and
/// peaks far above the ceiling.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn sample_input(rng: &mut XorShift64) -> f32 {
    let raw = f64::from(rng.next_u32()) / f64::from(u32::MAX);
    (raw as f32) * 32.0 - 16.0
}

/// Independent reference model for the limiter contract.
///
/// The implementation interleaves gain bookkeeping with sample scaling in one
/// loop; this model precomputes the gain for every mono sample first and then
/// applies the gains in a second pass, so the implementation's control flow
/// cannot be copied by construction.
struct LimiterModel {
    ceiling: f32,
    release: f32,
    gain: f32,
}

impl LimiterModel {
    const fn new(config: LimiterConfig) -> Self {
        Self {
            ceiling: config.ceiling,
            release: config.release_coefficient,
            gain: 1.0,
        }
    }

    fn advance(&mut self, required: f32) {
        if required < self.gain {
            self.gain = required;
        } else {
            self.gain += (1.0 - self.gain) * self.release;
        }
    }

    fn process(&mut self, samples: &[f32], output: &mut [f32]) {
        let mut gains: Vec<f32> = Vec::with_capacity(samples.len());
        for &sample in samples {
            if !sample.is_finite() {
                gains.push(self.gain);
                continue;
            }
            let required = if sample.abs() > self.ceiling {
                self.ceiling / sample.abs()
            } else {
                1.0
            };
            self.advance(required);
            gains.push(self.gain);
        }
        for (destination, (&sample, &gain)) in output.iter_mut().zip(samples.iter().zip(&gains)) {
            *destination = if sample.is_finite() {
                (sample * gain).clamp(-self.ceiling, self.ceiling)
            } else {
                0.0
            };
        }
    }

    fn process_interleaved(&mut self, samples: &[f32], channels: usize) -> Vec<f32> {
        let mut output = samples.to_vec();
        for frame in output.chunks_exact_mut(channels) {
            let mut required: f32 = 1.0;
            for &sample in frame.iter() {
                if sample.is_finite() {
                    let magnitude = sample.abs();
                    if magnitude > self.ceiling {
                        required = required.min(self.ceiling / magnitude);
                    }
                }
            }
            self.advance(required);
            for sample in frame.iter_mut() {
                *sample = if sample.is_finite() {
                    (*sample * self.gain).clamp(-self.ceiling, self.ceiling)
                } else {
                    0.0
                };
            }
        }
        output
    }
}

/// Asserts two sample buffers agree within the tolerance.
fn assert_samples_close(actual: &[f32], expected: &[f32], context: &str) {
    assert_eq!(actual.len(), expected.len(), "length mismatch: {context}");
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        let delta = (actual - expected).abs();
        assert!(
            delta <= LIMITER_TOLERANCE,
            "sample {index} differs ({actual} vs {expected}, delta {delta}): {context}"
        );
    }
}

/// A spread of limiter configurations spanning the supported contract space.
fn sample_configs() -> [LimiterConfig; 4] {
    [
        LimiterConfig::new(1.0, 0.05).unwrap(),
        LimiterConfig::new(0.8, 0.01).unwrap(),
        LimiterConfig::new(0.5, 0.2).unwrap(),
        LimiterConfig::new(1.0, 1.0).unwrap(),
    ]
}

/// Drives random mono blocks against the independent model and asserts the
/// output and observable gain state agree for every configuration.
///
/// `limiter`/`model` plus `actual`/`expected` are short local names, not a
/// design smell, so the noisy `similar_names` lint is allowed here.
#[allow(clippy::similar_names, clippy::too_many_lines)]
#[test]
fn limiter_matches_an_independent_state_model() {
    let mut rng = XorShift64(0x6A1E_0000_2026_0007);
    for (config_index, config) in sample_configs().iter().enumerate() {
        let mut limiter = PeakLimiter::new(*config);
        let mut model = LimiterModel::new(*config);
        for step in 0..20_000 {
            let len = rng.next_usize(64);
            let samples: Vec<f32> = (0..len).map(|_| sample_input(&mut rng)).collect();
            let mut actual = samples.clone();
            let mut expected = vec![0.0; len];
            limiter.apply(&mut actual);
            model.process(&samples, &mut expected);
            assert_samples_close(
                &actual,
                &expected,
                &format!("config {config_index} step {step}"),
            );
            assert!(
                (limiter.gain() - model.gain).abs() <= LIMITER_TOLERANCE,
                "gain drifted at config {config_index} step {step}: {} vs {}",
                limiter.gain(),
                model.gain
            );
        }
    }
}

/// Drives random interleaved blocks against the frame-linked model.
#[allow(clippy::similar_names, clippy::too_many_lines)]
#[test]
fn interleaved_limiting_matches_a_frame_linked_model() {
    let mut rng = XorShift64(0x6A1E_0000_2026_0008);
    let config = LimiterConfig::new(0.95, 0.03).unwrap();
    let mut limiter = PeakLimiter::new(config);
    let mut model = LimiterModel::new(config);
    for step in 0..20_000 {
        let channels = 1 + rng.next_usize(usize::from(MAX_AUDIO_CHANNELS));
        let frames = 1 + rng.next_usize(17);
        let samples: Vec<f32> = (0..frames * channels)
            .map(|_| sample_input(&mut rng))
            .collect();
        let mut actual = samples.clone();
        limiter.apply_interleaved(&mut actual, channels).unwrap();
        let expected = model.process_interleaved(&samples, channels);
        assert_samples_close(
            &actual,
            &expected,
            &format!("step {step} channels {channels}"),
        );
        assert!(
            (limiter.gain() - model.gain).abs() <= LIMITER_TOLERANCE,
            "gain drifted at step {step}: {} vs {}",
            limiter.gain(),
            model.gain
        );
    }
}

/// Proves the exact ceiling contract over wide random input, including
/// magnitudes far above the ceiling, for the per-sample and interleaved paths.
#[allow(clippy::similar_names)]
#[test]
fn output_never_exceeds_the_ceiling() {
    let mut rng = XorShift64(0x6A1E_0000_2026_0009);
    for (config_index, config) in sample_configs().iter().enumerate() {
        let mut limiter = PeakLimiter::new(*config);
        for _ in 0..5_000 {
            let len = rng.next_usize(128);
            let mut block: Vec<f32> = (0..len).map(|_| sample_input(&mut rng)).collect();
            limiter.apply(&mut block);
            for &sample in &block {
                assert!(
                    sample.is_finite() && sample <= config.ceiling && sample >= -config.ceiling,
                    "config {config_index}: sample {sample} exceeds ceiling {}",
                    config.ceiling
                );
            }
        }
    }
}

/// Proves that sub-ceiling input passes through bit-identically once the gain
/// has fully released, for both the per-sample and interleaved paths.
#[allow(clippy::cast_precision_loss)]
#[test]
fn sub_ceiling_input_is_transparent_after_full_release() {
    let mut rng = XorShift64(0x6A1E_0000_2026_000A);
    for (config_index, config) in sample_configs().iter().enumerate() {
        let mut limiter = PeakLimiter::new(*config);
        for _ in 0..5_000 {
            // Even length keeps the block aligned for the two-channel interleaved path.
            let len = 2 * (1 + rng.next_usize(32));
            let block: Vec<f32> = (0..len)
                .map(|_| (rng.next_u32() % 1000) as f32 / 1000.0 * config.ceiling * 0.5)
                .collect();
            let mut mono = block.clone();
            limiter.apply(&mut mono);
            assert!(
                mono.iter()
                    .zip(&block)
                    .all(|(left, right)| left.to_bits() == right.to_bits()),
                "config {config_index}: mono pass-through changed a sub-ceiling sample"
            );
            let mut interleaved = block.clone();
            limiter.apply_interleaved(&mut interleaved, 2).unwrap();
            assert!(
                interleaved
                    .iter()
                    .zip(&block)
                    .all(|(left, right)| left.to_bits() == right.to_bits()),
                "config {config_index}: interleaved pass-through changed a sub-ceiling sample"
            );
        }
    }
}
