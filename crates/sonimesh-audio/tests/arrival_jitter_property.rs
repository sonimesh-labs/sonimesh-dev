//! Deterministic property coverage for relative arrival-jitter measurement.
//!
//! The estimator must report exactly the injected clock variation: when a
//! sender timeline is shifted by a constant base delay plus a bounded
//! per-frame noise walk, each observation equals the change in that noise, a
//! stable tail converges to zero, and reset or a backward clock never corrupts
//! later measurements.

use sonimesh_audio::ArrivalJitter;

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
}

/// Sample rate and frame geometry of the constructed sender timeline.
const SAMPLE_RATE_HZ: u32 = 48_000;
const FRAME_SAMPLES: u64 = 480;
const FRAME_MS: u64 = 10;
/// Constant one-way delay applied to every arrival, in milliseconds.
const BASE_DELAY_MS: u64 = 137;
/// Observations per property run.
const STEPS: usize = 2_000;

/// Draws the next bounded noise value from a random walk around the previous
/// one. Each step changes the arrival time by at most 10 ms, so the arrival
/// tick stream stays non-decreasing at the 10 ms frame cadence.
fn next_noise(random: &mut XorShift64, previous: u64) -> u64 {
    let step = i64::from(random.next_u32() % 21) - 10;
    u64::try_from((i64::try_from(previous).unwrap() + step).clamp(0, 60)).unwrap()
}

#[test]
fn measured_jitter_matches_the_injected_random_walk() {
    for seed in [0x1A11_2026_0000_0001, 0x1A11_2026_0000_0002] {
        let mut random = XorShift64(seed);
        let mut jitter = ArrivalJitter::new(SAMPLE_RATE_HZ, 1_000).unwrap();
        let mut previous_noise = 0_u64;
        for frame in 0..STEPS {
            let noise = next_noise(&mut random, previous_noise);
            let arrival_ms = BASE_DELAY_MS + u64::try_from(frame).unwrap() * FRAME_MS + noise;
            let samples = u64::try_from(frame).unwrap() * FRAME_SAMPLES;
            let expected = frame
                .checked_sub(1)
                .map(|_| u32::try_from(noise.abs_diff(previous_noise)).unwrap());
            assert_eq!(
                jitter.observe(arrival_ms, samples),
                expected,
                "frame {frame} must report exactly the injected noise change"
            );
            previous_noise = noise;
        }
    }
}

#[test]
fn a_stable_tail_converges_to_zero_jitter() {
    let mut random = XorShift64(0x1A11_2026_0000_0003);
    let mut jitter = ArrivalJitter::new(SAMPLE_RATE_HZ, 1_000).unwrap();
    let mut previous_noise = 0_u64;
    for frame in 0..STEPS {
        let noise = next_noise(&mut random, previous_noise);
        let arrival_ms = BASE_DELAY_MS + u64::try_from(frame).unwrap() * FRAME_MS + noise;
        let _ = jitter.observe(arrival_ms, u64::try_from(frame).unwrap() * FRAME_SAMPLES);
        previous_noise = noise;
    }
    // The noise freezes at the last walk value, so the arrival timeline keeps
    // advancing at the exact frame cadence and every observation is zero.
    let frozen_noise = previous_noise;
    for frame in STEPS..(STEPS * 2) {
        let arrival_ms = BASE_DELAY_MS + u64::try_from(frame).unwrap() * FRAME_MS + frozen_noise;
        let samples = u64::try_from(frame).unwrap() * FRAME_SAMPLES;
        assert_eq!(
            jitter.observe(arrival_ms, samples),
            Some(0),
            "frozen frame {frame} must report zero"
        );
    }
}

#[test]
fn reset_and_backward_clock_preserve_later_measurements() {
    let mut random = XorShift64(0x1A11_2026_0000_0004);
    let mut jitter = ArrivalJitter::new(SAMPLE_RATE_HZ, 1_000).unwrap();
    let mut previous_noise = 0_u64;
    for frame in 0..STEPS {
        let noise = next_noise(&mut random, previous_noise);
        let arrival_ms = BASE_DELAY_MS + u64::try_from(frame).unwrap() * FRAME_MS + noise;
        let samples = u64::try_from(frame).unwrap() * FRAME_SAMPLES;
        if frame == STEPS / 3 {
            jitter.reset();
            assert_eq!(jitter.observe(arrival_ms, samples), None);
        } else if frame == STEPS / 2 {
            // A backward tick relative to the earlier baseline is skipped, then
            // a fresh baseline is established so later frames stay exact.
            assert_eq!(jitter.observe(0, samples), None);
            jitter.reset();
            assert_eq!(jitter.observe(arrival_ms, samples), None);
        } else {
            let expected = frame
                .checked_sub(1)
                .map(|_| u32::try_from(noise.abs_diff(previous_noise)).unwrap());
            assert_eq!(
                jitter.observe(arrival_ms, samples),
                expected,
                "frame {frame} must stay exact across reset and clock glitches"
            );
        }
        previous_noise = noise;
    }
}
