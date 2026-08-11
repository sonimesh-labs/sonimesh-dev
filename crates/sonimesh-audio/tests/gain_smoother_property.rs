//! Deterministic property coverage for the realtime gain smoother.
//!
//! The smoother is the volume and mute boundary on the output path: a gain
//! change must ramp linearly over a fixed number of samples, snap exactly onto
//! the target when the ramp completes, and then hold that gain without drift
//! across later blocks. This test drives a fixed pseudo-random sequence of
//! retargets and applies against an independent state model, and separately
//! proves the ramp contract (monotone, no overshoot, exact target hold).

use sonimesh_audio::GainSmoother;

/// Tolerance for gain values. The model mirrors the implementation's `f32`
/// arithmetic, so results are bit-identical today; the tolerance keeps the
/// test honest if the ramp is re-expressed without changing its contract.
const GAIN_TOLERANCE: f32 = 1e-6;

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

/// Bounded pseudo-random gain value in `[-4.0, 4.0)`, covering mute, unity,
/// boosts, and sign flips.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn sample_gain(rng: &mut XorShift64) -> f32 {
    let raw = f64::from(rng.next_u32()) / f64::from(u32::MAX);
    (raw as f32) * 8.0 - 4.0
}

/// Independent reference model for the smoother contract.
///
/// The implementation interleaves ramp bookkeeping with sample multiplication
/// in one loop; this model computes the multiplier for each sample first and
/// then applies it, so the implementation's control flow cannot be copied by
/// construction.
struct GainModel {
    current: f32,
    target: f32,
    step: f32,
    remaining: u16,
}

impl GainModel {
    const fn new(gain: f32) -> Self {
        Self {
            current: gain,
            target: gain,
            step: 0.0,
            remaining: 0,
        }
    }

    fn set_target(&mut self, target: f32, ramp_samples: u16) {
        self.target = target;
        if ramp_samples == 0 {
            self.current = target;
            self.step = 0.0;
            self.remaining = 0;
            return;
        }
        self.step = (target - self.current) / f32::from(ramp_samples);
        self.remaining = ramp_samples;
    }

    fn apply(&mut self, samples: &mut [f32]) {
        for sample in samples {
            let multiplier = if self.remaining > 0 {
                self.remaining -= 1;
                self.current += self.step;
                if self.remaining == 0 {
                    self.current = self.target;
                }
                self.current
            } else {
                self.current
            };
            *sample *= multiplier;
        }
    }
}

/// Whether two gain values agree within the contract tolerance.
fn close(left: f32, right: f32) -> bool {
    (left - right).abs() <= GAIN_TOLERANCE
}

/// Asserts two sample buffers agree within the tolerance.
fn assert_samples_close(actual: &[f32], expected: &[f32], context: &str) {
    assert_eq!(actual.len(), expected.len(), "length mismatch: {context}");
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        let delta = (actual - expected).abs();
        assert!(
            delta <= GAIN_TOLERANCE,
            "sample {index} differs ({actual} vs {expected}, delta {delta}): {context}"
        );
    }
}

/// Drives a random interleaving of retargets and applies against the model,
/// asserting every output sample and the observable current/target state.
///
/// Test values are tiny so the usize-to-float casts cannot lose precision, and
/// `smoother`/`model` plus `actual`/`expected` are short local names, not a
/// design smell, so the noisy pedantic casts and `similar_names` lints are
/// allowed here.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::similar_names,
    clippy::too_many_lines
)]
#[test]
fn gain_smoothing_matches_an_independent_state_model() {
    let mut rng = XorShift64(0x6A1E_0000_2026_0006);
    let mut smoother = GainSmoother::new(sample_gain(&mut rng));
    let mut model = GainModel::new(smoother.current());

    for step in 0..50_000 {
        if rng.next_u32() % 3 == 0 {
            let target = sample_gain(&mut rng);
            let ramp = (rng.next_u32() % 301) as u16;
            smoother.set_target(target, ramp);
            model.set_target(target, ramp);
        } else {
            let len = (rng.next_u32() % 49) as usize;
            let mut actual: Vec<f32> = (0..len).map(|_| sample_gain(&mut rng)).collect();
            let mut expected = actual.clone();
            smoother.apply(&mut actual);
            model.apply(&mut expected);
            assert_samples_close(&actual, &expected, &format!("step {step} apply len {len}"));
        }
        assert!(
            close(smoother.current(), model.current) && close(smoother.target(), model.target),
            "state drifted at step {step}: smoother {} {} vs model {} {}",
            smoother.current(),
            smoother.target(),
            model.current,
            model.target
        );
    }
}

/// Applies a full ramp in random splits and proves the behavioral contract an
/// independent model cannot: the applied gains are monotone from the start
/// toward the target, never leave the endpoint span, land exactly on the
/// target, and hold without drift afterwards.
///
/// `smoother`/`multipliers` are short local names, not a design smell, so the
/// noisy `similar_names` lint is allowed here.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::similar_names
)]
#[test]
fn a_completed_ramp_is_monotone_without_overshoot_and_holds_target() {
    let mut rng = XorShift64(0x6A1E_0000_2026_0005);
    for scenario in 0..20_000 {
        let from = sample_gain(&mut rng);
        let to = sample_gain(&mut rng);
        let ramp = 1 + (rng.next_usize(300) as u16);
        // Accumulated `f32` rounding over `ramp` additions bounds how far an
        // intermediate multiplier may sit from the ideal linear position.
        let margin = 2.0 * f32::from(ramp) * f32::EPSILON * (1.0 + from.abs().max(to.abs()));
        let mut smoother = GainSmoother::new(from);
        smoother.set_target(to, ramp);

        let mut remaining = ramp;
        let mut multipliers: Vec<f32> = Vec::new();
        while remaining > 0 {
            let chunk = 1 + (rng.next_usize(usize::from(remaining.min(48))) as u16);
            let mut block = vec![1.0f32; usize::from(chunk)];
            smoother.apply(&mut block);
            multipliers.extend_from_slice(&block);
            remaining -= chunk;
        }

        let (low, high) = if from <= to { (from, to) } else { (to, from) };
        for (index, &value) in multipliers.iter().enumerate() {
            assert!(
                value >= low - margin && value <= high + margin,
                "scenario {scenario} overshoot at sample {index}: {value} outside \
                 [{low}, {high}] by more than {margin}"
            );
            if index > 0 {
                let delta = value - multipliers[index - 1];
                if to >= from {
                    assert!(
                        delta >= -margin,
                        "scenario {scenario} not non-decreasing at sample {index}"
                    );
                } else {
                    assert!(
                        delta <= margin,
                        "scenario {scenario} not non-increasing at sample {index}"
                    );
                }
            }
        }
        assert!(
            close(smoother.current(), to),
            "scenario {scenario} ramp did not end on the target: {} vs {to}",
            smoother.current()
        );
        let mut held = vec![0.5f32; 8];
        smoother.apply(&mut held);
        assert!(
            held.iter().all(|sample| close(*sample, 0.5 * to)),
            "scenario {scenario} gain drifted after the ramp completed"
        );
    }
}
