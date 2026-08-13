//! Deterministic property coverage for the correlation offset estimator.
//!
//! `estimate_offset_ms` must recover an injected integer sample delay across
//! sample rates, probe lengths, coarse factors, and seeds, and it must remain
//! total and deterministic over arbitrary finite inputs. This test sweeps
//! seeded recordings against an independent ground truth (the injected lag)
//! and fuzzes short arbitrary signals to prove the documented domain never
//! panics and that repeated calls agree exactly.

use sonimesh_latency_measurement::{OffsetConfig, OffsetError, estimate_offset_ms, test_signal};

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

    fn pick<'a, T>(&mut self, choices: &'a [T]) -> &'a T {
        &choices[self.next_usize(choices.len())]
    }
}

const SAMPLE_RATES: &[u32] = &[8_000, 16_000, 48_000];
const SEEDS: &[u32] = &[0x1111_2222];

/// Injected lags for one sample rate: the leading edge, tiny offsets, lags
/// deep inside the one-second default search window, and a lag exactly at the
/// window boundary (`max_lag` equals the sample rate for the default search).
fn lags_for_rate(rate_hz: u32) -> [usize; 7] {
    [
        0,
        1,
        7,
        100,
        usize::try_from(rate_hz / 3).expect("lag fits usize"),
        usize::try_from((3 * rate_hz) / 4).expect("lag fits usize"),
        usize::try_from(rate_hz).expect("lag fits usize"),
    ]
}

/// Three probe/coarse configurations: the standard 4k probe at 16x, a shorter
/// probe at 8x, and an aggressive 64x coarse factor.
fn configs() -> Vec<OffsetConfig> {
    let mut standard = OffsetConfig::new(48_000);
    standard.probe_len = 4_096;
    let mut small = OffsetConfig::new(48_000);
    small.probe_len = 2_048;
    small.coarse_factor = 8;
    let mut coarse = OffsetConfig::new(48_000);
    coarse.probe_len = 4_096;
    coarse.coarse_factor = 64;
    vec![standard, small, coarse]
}

/// Builds a recording with `delay_samples` of leading silence followed by the
/// full reference signal.
fn delayed_recording(delay_samples: usize, reference: &[f32]) -> Vec<f32> {
    let mut recording = vec![0.0_f32; delay_samples];
    recording.extend_from_slice(reference);
    recording
}

/// Every injected integer lag must be recovered across rates, seeds, probe
/// lengths, and coarse factors; the reported milliseconds must agree with the
/// lag at the configured rate; and repeated calls must be bit-identical.
#[test]
fn injected_integer_lags_are_recovered_across_configurations() {
    const TOLERANCE_SAMPLES: f64 = 0.75;

    for &rate in SAMPLE_RATES {
        for &seed in SEEDS {
            let reference = test_signal(rate, 2, seed);
            for &lag in &lags_for_rate(rate) {
                let recording = delayed_recording(lag, &reference);
                for mut config in configs() {
                    config.sample_rate_hz = rate;
                    let estimate = estimate_offset_ms(&config, &reference, &recording)
                        .unwrap_or_else(|error| {
                            panic!("rate={rate} seed={seed:#x} lag={lag}: {error}")
                        });

                    let lag_f64 = f64::from(u32::try_from(lag).expect("lag fits u32"));
                    assert!(
                        (estimate.lag_samples - lag_f64).abs() <= TOLERANCE_SAMPLES,
                        "rate={rate} seed={seed:#x} lag={lag} estimated={}",
                        estimate.lag_samples
                    );
                    let expected_ms = lag_f64 * 1_000.0 / f64::from(rate);
                    assert!(
                        (estimate.offset_ms - expected_ms).abs()
                            <= TOLERANCE_SAMPLES * 1_000.0 / f64::from(rate),
                        "rate={rate} seed={seed:#x} lag={lag} offset_ms={}",
                        estimate.offset_ms
                    );
                    assert!(estimate.peak_score > 0.0, "peak score is positive");
                    assert!(
                        estimate.distinctness_ratio >= config.distinctness_ratio,
                        "distinctness meets the configured threshold"
                    );

                    let again = estimate_offset_ms(&config, &reference, &recording)
                        .expect("repeat call succeeds");
                    assert_eq!(
                        estimate, again,
                        "rate={rate} seed={seed:#x} lag={lag} must be deterministic"
                    );
                }
            }
        }
    }
}

/// Builds a recording whose reference appears at a fractional sample delay,
/// sampling the reference at fractional positions with linear interpolation.
///
/// The fractional position is bounded by the reference length, so the float to
/// integer conversions cannot truncate or sign-wrap in practice.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn fractional_recording(delay_samples: f64, reference: &[f32]) -> Vec<f32> {
    let integer = delay_samples.floor() as usize;
    let fraction = delay_samples - integer as f64;
    let mut recording = vec![0.0_f32; integer];
    for k in 0..=reference.len() {
        let position = k as f64 - fraction;
        let value = if position < 0.0 {
            0.0
        } else {
            let lower = position.floor() as usize;
            let upper = lower.saturating_add(1).min(reference.len() - 1);
            let weight = position - lower as f64;
            reference[lower] * (1.0 - weight) as f32 + reference[upper] * weight as f32
        };
        recording.push(value);
    }
    recording
}

/// Finite sample values used by the totality fuzz; deliberately no NaN or
/// infinity because the estimator documents a finite-sample domain.
const SAMPLE_VALUES: &[f32] = &[-1.0, -0.5, 0.0, 0.25, 0.5, 1.0];

fn random_samples(rng: &mut XorShift64, length: usize) -> Vec<f32> {
    (0..length).map(|_| *rng.pick(SAMPLE_VALUES)).collect()
}

fn random_config(rng: &mut XorShift64) -> OffsetConfig {
    let mut config = OffsetConfig::new(*rng.pick(&[0_u32, 1, 8_000, 44_100, 48_000]));
    config.max_search_ms = *rng.pick(&[0.0, 0.5, 1.0, 100.0, 1_000.0]);
    config.coarse_factor = *rng.pick(&[
        1_usize,
        2,
        4,
        16,
        64,
        1 << 20,
        usize::MAX / 4,
        usize::MAX / 2,
    ]);
    config.probe_len = *rng.pick(&[0_usize, 1, 16, 64, 512]);
    config.distinctness_ratio = *rng.pick(&[0.0, 1.0, 3.0, 100.0]);
    config
}

/// Arbitrary finite inputs must never panic, repeated calls must agree
/// exactly, and every reported estimate must satisfy the estimator's own
/// documented invariants.
#[test]
fn arbitrary_finite_inputs_never_panic_and_estimation_is_deterministic() {
    const STEPS: usize = 800;
    let mut rng = XorShift64(0x0FF5_0000_2026_0021);

    for _ in 0..STEPS {
        let reference_len = rng.next_usize(1_025);
        let recording_len = rng.next_usize(1_025);
        let reference = random_samples(&mut rng, reference_len);
        let recording = random_samples(&mut rng, recording_len);
        let config = random_config(&mut rng);

        let first = estimate_offset_ms(&config, &reference, &recording);
        let second = estimate_offset_ms(&config, &reference, &recording);
        assert_eq!(first, second, "estimation is deterministic");

        if let Ok(estimate) = first {
            assert!(estimate.peak_score > 0.0, "reported peak score is positive");
            assert!(estimate.lag_samples >= 0.0, "reported lag is non-negative");
            assert!(estimate.offset_ms.is_finite(), "reported offset is finite");
            assert!(
                estimate.distinctness_ratio >= config.distinctness_ratio,
                "reported distinctness meets the configured threshold"
            );
        }
    }
}

/// Fractional injected delays strictly inside the search window must be
/// refined to the sub-sample value by the parabolic fit, across rates and
/// probe configurations. The leading edge is excluded because refinement only
/// applies to lags strictly between the window bounds.
#[test]
fn fractional_lags_are_refined_to_the_fractional_sample() {
    const TOLERANCE_SAMPLES: f64 = 0.4;
    let cases = [
        (8_000_u32, 1.25_f64),
        (8_000, 7.25),
        (8_000, 100.5),
        (48_000, 7.25),
        (48_000, 12_000.5),
    ];
    for &(rate, delay) in &cases {
        let reference = test_signal(rate, 2, 0x2222_3333);
        let recording = fractional_recording(delay, &reference);
        for mut config in configs() {
            config.sample_rate_hz = rate;
            let estimate = estimate_offset_ms(&config, &reference, &recording)
                .unwrap_or_else(|error| panic!("rate={rate} delay={delay}: {error}"));
            assert!(
                (estimate.lag_samples - delay).abs() <= TOLERANCE_SAMPLES,
                "rate={rate} delay={delay} estimated={}",
                estimate.lag_samples
            );
        }
    }
}

/// A sub-sample delay at the leading edge of the search window is reported as
/// the nearest integer lag: the parabolic refinement only applies to lags
/// strictly inside the window, so the estimate stays on the integer grid.
#[test]
fn leading_edge_fractional_lag_quantizes_to_the_integer_grid() {
    const TOLERANCE_SAMPLES: f64 = 0.5;
    for &rate in SAMPLE_RATES {
        let reference = test_signal(rate, 2, 0x2222_3333);
        let recording = fractional_recording(0.5, &reference);
        for mut config in configs() {
            config.sample_rate_hz = rate;
            let estimate = estimate_offset_ms(&config, &reference, &recording)
                .unwrap_or_else(|error| panic!("rate={rate} delay=0.5: {error}"));
            assert!(
                (estimate.lag_samples - 0.5).abs() <= TOLERANCE_SAMPLES,
                "rate={rate} delay=0.5 estimated={}",
                estimate.lag_samples
            );
        }
    }
}

/// Non-finite or negative configuration bounds are rejected before any
/// correlation work, so a caller cannot silently disable the distinctness
/// check with a NaN or negative ratio. Finite non-negative bounds (including
/// the documented zero ratio) are never reported as invalid configuration.
#[test]
fn non_finite_or_negative_bounds_are_rejected() {
    let reference = test_signal(48_000, 1, 7);
    let recording = delayed_recording(0, &reference);
    let mut config = OffsetConfig::new(48_000);

    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, -0.5] {
        config.distinctness_ratio = bad;
        assert_eq!(
            estimate_offset_ms(&config, &reference, &recording),
            Err(OffsetError::InvalidConfig),
            "ratio {bad}"
        );
        config.distinctness_ratio = 3.0;
        config.max_search_ms = bad;
        assert_eq!(
            estimate_offset_ms(&config, &reference, &recording),
            Err(OffsetError::InvalidConfig),
            "max_search_ms {bad}"
        );
        config.max_search_ms = 1_000.0;
    }

    config.distinctness_ratio = 0.0;
    config.max_search_ms = 1_000.0;
    assert!(!matches!(
        estimate_offset_ms(&config, &reference, &recording),
        Err(OffsetError::InvalidConfig)
    ));
    config.distinctness_ratio = 3.0;
    config.max_search_ms = 0.0;
    assert!(!matches!(
        estimate_offset_ms(&config, &reference, &recording),
        Err(OffsetError::InvalidConfig)
    ));
}
