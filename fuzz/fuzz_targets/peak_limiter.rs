#![no_main]

//! Fuzz the bounded output-stage peak limiter.
//!
//! Arbitrary validated limiter ceilings and release coefficients meet
//! arbitrary interleaved sample payloads, including non-finite and
//! out-of-range values. The limiter must never panic, must fail non-finite
//! input closed to a bounded zero sample, must keep every output sample
//! finite and within the configured ceiling, and must leave the block
//! untouched when the channel count or frame alignment is rejected.

use libfuzzer_sys::fuzz_target;
use sonimesh_audio::{LimiterConfig, MAX_AUDIO_CHANNELS, PeakLimiter};

const MAX_SCRATCH_SAMPLES: usize = 1_024;

fuzz_target!(|data: &[u8]| {
    if data.len() < 12 {
        return;
    }

    let raw_ceiling = f32::from_bits(u32::from_le_bytes([data[0], data[1], data[2], data[3]]));
    let raw_release = f32::from_bits(u32::from_le_bytes([data[4], data[5], data[6], data[7]]));
    // Exercise the configuration rejection path for non-finite or out-of-range
    // values, then fall back to a guaranteed-valid config so the limiter loop
    // still runs on every input.
    let config = LimiterConfig::new(raw_ceiling, raw_release).unwrap_or_else(|_| {
        LimiterConfig::new(
            0.5 + f32::from(data[8]) / 512.0,
            0.5 + f32::from(data[9]) / 512.0,
        )
        .expect("fallback config is finite and in (0.0, 1.0]")
    });
    let mut limiter = PeakLimiter::new(config);

    let len = usize::from(data[10]) % MAX_SCRATCH_SAMPLES;
    let mut samples = vec![0.0f32; len];
    for index in 0..len {
        samples[index] = f32::from_bits(u32::from_le_bytes([
            data[index % data.len()],
            data[(index + 1) % data.len()],
            data[(index + 2) % data.len()],
            data[(index + 3) % data.len()],
        ]));
    }
    let original = samples.clone();

    // Raw channel counts cover accepted frames, invalid channel counts, and
    // misaligned frames; the block must stay untouched on every rejection.
    let channels = usize::from(data[11]) % (usize::from(MAX_AUDIO_CHANNELS) + 2);
    match limiter.apply_interleaved(&mut samples, channels) {
        Ok(()) => {
            assert!(
                samples
                    .iter()
                    .all(|sample| sample.is_finite() && sample.abs() <= config.ceiling)
            );
        }
        Err(_) => {
            assert!(
                samples
                    .iter()
                    .zip(original.iter())
                    .all(|(left, right)| left.to_bits() == right.to_bits())
            );
        }
    }

    // The mono path must apply the same ceiling guarantee without erroring.
    let mut mono = original;
    limiter.apply(&mut mono);
    assert!(
        mono.iter()
            .all(|sample| sample.is_finite() && sample.abs() <= config.ceiling)
    );
    let gain = limiter.gain();
    assert!(gain.is_finite() && gain > 0.0 && gain <= 1.0);
});
