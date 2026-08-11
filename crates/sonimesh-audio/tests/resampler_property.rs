//! Deterministic property coverage for the bounded linear resampler.
//!
//! The resampler is the realtime conversion primitive between source and
//! output sample rates: it must keep exact phase accounting across chunked
//! calls, retain the right tail of input for the next call, pad trailing
//! output with silence when input runs out, and never let channel data bleed
//! between interleaved channels. This test drives a fixed pseudo-random
//! sequence of chunked feeds against an independent phase-tracking model and
//! asserts every outcome, every output sample, and the retained-tail contract
//! agree.

use sonimesh_audio::{LinearResampler, ResampleOutcome};

/// Rate pairs covering equal, integer up/down, non-integer conversion, and
/// the platform bounds.
const RATE_PAIRS: [(u32, u32); 7] = [
    (48_000, 48_000),
    (48_000, 96_000),
    (96_000, 48_000),
    (44_100, 48_000),
    (48_000, 44_100),
    (8_000, 192_000),
    (192_000, 8_000),
];

/// Channel layouts from mono up to the multi-channel mix boundary.
const CHANNELS: [u16; 4] = [1, 2, 3, 4];

/// Tolerance for interpolated samples. The model mirrors the implementation's
/// `f32` interpolation, so results are bit-identical today; the tolerance
/// keeps the test honest if a reviewed higher-quality interpolation stage is
/// swapped in without changing the resampler contract.
const SAMPLE_TOLERANCE: f32 = 1e-6;

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

/// Picks one element of a non-empty slice.
fn pick<'a, T>(rng: &mut XorShift64, items: &'a [T]) -> &'a T {
    &items[rng.next_usize(items.len())]
}

/// Deterministic stream sample: distinct per channel so interleaving bugs are
/// visible, and bounded so interpolation stays far from any float limit.
#[allow(clippy::cast_precision_loss)]
const fn source_sample(stream_frame: usize, channel: usize) -> f32 {
    ((stream_frame % 97) as f32) * 0.25 - 3.0 + (channel as f32) * 3.0
}

/// Independent reference model for the resampler contract.
///
/// The implementation interleaves phase bookkeeping with interpolation in one
/// loop; this model keeps the two concerns separate and is written only from
/// the contract, so a phase, retention, or channel-index mistake in the
/// implementation cannot be copied by construction.
struct ResamplerModel {
    channels: usize,
    nominal_ratio: f64,
    ratio: f64,
    phase: f64,
}

impl ResamplerModel {
    fn new(input_rate_hz: u32, output_rate_hz: u32, channels: usize) -> Self {
        let nominal_ratio = f64::from(input_rate_hz) / f64::from(output_rate_hz);
        Self {
            channels,
            nominal_ratio,
            ratio: nominal_ratio,
            phase: 0.0,
        }
    }

    fn set_rate_adjustment_ppm(&mut self, adjustment_ppm: i32) {
        self.ratio = self.nominal_ratio * (1.0 + f64::from(adjustment_ppm) / 1_000_000.0);
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn process(&mut self, input: &[f32], output: &mut [f32]) -> ResampleOutcome {
        output.fill(0.0);
        let input_frames = input.len() / self.channels;
        let output_frames = output.len() / self.channels;
        let mut produced = 0;
        while produced < output_frames {
            let base = self.phase.floor() as usize;
            // Interpolation needs frames `base` and `base + 1`.
            if base + 1 >= input_frames {
                break;
            }
            let fraction = self.phase.fract() as f32;
            let out = produced * self.channels;
            for channel in 0..self.channels {
                let first = input[base * self.channels + channel];
                let second = input[(base + 1) * self.channels + channel];
                output[out + channel] = first + (second - first) * fraction;
            }
            produced += 1;
            self.phase += self.ratio;
        }
        let consumed = (self.phase.floor() as usize).min(input_frames.saturating_sub(1));
        self.phase -= consumed as f64;
        ResampleOutcome {
            consumed_input_frames: consumed,
            produced_output_frames: produced,
        }
    }
}

/// Asserts two interleaved sample buffers agree within the tolerance.
fn assert_samples_close(actual: &[f32], expected: &[f32], context: &str) {
    assert_eq!(actual.len(), expected.len(), "length mismatch: {context}");
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        let delta = (actual - expected).abs();
        assert!(
            delta <= SAMPLE_TOLERANCE,
            "sample {index} differs ({actual} vs {expected}, delta {delta}): {context}"
        );
    }
}

/// Drives every rate pair, channel count, and a spread of ppm corrections
/// through ~100k chunked streaming steps, asserting every outcome, every
/// output sample, and the consumed/retained accounting identity against the
/// model.
///
/// Test values are tiny so the usize-to-float casts cannot lose precision, and
/// `resampler`/`model` plus `actual`/`expected` are short local names, not a
/// design smell, so the noisy pedantic casts and `similar_names` lints are
/// allowed here.
#[allow(
    clippy::cast_precision_loss,
    clippy::similar_names,
    clippy::too_many_lines
)]
#[test]
fn chunked_streaming_matches_an_independent_phase_model() {
    const STEPS_PER_SCENARIO: usize = 1_000;
    const PPM_SETTINGS: [i32; 5] = [0, 2_000, -2_000, 137, -883];
    let mut rng = XorShift64(0x5EED_0000_2026_0002);

    for &(input_rate, output_rate) in &RATE_PAIRS {
        for &channels in &CHANNELS {
            for &ppm in &PPM_SETTINGS {
                let mut resampler =
                    LinearResampler::new(input_rate, output_rate, channels).unwrap();
                resampler.set_rate_adjustment_ppm(ppm).unwrap();
                let mut model = ResamplerModel::new(input_rate, output_rate, usize::from(channels));
                model.set_rate_adjustment_ppm(ppm);

                let mut retained: Vec<f32> = Vec::new();
                let mut stream_frame = 0usize;
                let mut consumed_total = 0usize;
                let mut fed_total = 0usize;

                for step in 0..STEPS_PER_SCENARIO {
                    let feed_frames = rng.next_usize(33);
                    let fresh: Vec<f32> = (0..feed_frames * usize::from(channels))
                        .map(|i| {
                            source_sample(
                                stream_frame + i / usize::from(channels),
                                i % usize::from(channels),
                            )
                        })
                        .collect();
                    stream_frame += feed_frames;
                    fed_total += feed_frames;

                    let mut input = retained.clone();
                    input.extend_from_slice(&fresh);

                    let out_frames = rng.next_usize(49);
                    let mut actual = vec![f32::NAN; out_frames * usize::from(channels)];
                    let mut expected = vec![f32::NAN; out_frames * usize::from(channels)];
                    let actual_outcome = resampler.process(&input, &mut actual).unwrap();
                    let expected_outcome = model.process(&input, &mut expected);

                    let context = format!(
                        "rates {input_rate}->{output_rate} ch {channels} ppm {ppm} \
                         step {step} in {} out {out_frames}",
                        input.len() / usize::from(channels)
                    );
                    assert_eq!(actual_outcome, expected_outcome, "{context}");
                    assert_samples_close(&actual, &expected, &context);

                    consumed_total += actual_outcome.consumed_input_frames;
                    retained = input
                        [actual_outcome.consumed_input_frames * usize::from(channels)..]
                        .to_vec();
                }

                // Every fed frame is either consumed or still retained.
                assert_eq!(
                    consumed_total + retained.len() / usize::from(channels),
                    fed_total,
                    "input accounting drifted for rates {input_rate}->{output_rate} \
                     ch {channels} ppm {ppm}"
                );
            }
        }
    }
}

/// Feeds a single bounded chunk with no prior retention and requests a large
/// output: the resampler must produce exactly what the model produces, pad the
/// tail with silence, keep at least one interpolation frame for the next call,
/// and make progress whenever at least two frames are available.
///
/// `resampler`/`model` and `actual`/`expected` are short local names, not a
/// design smell, so the noisy `similar_names` lint is allowed here.
#[allow(
    clippy::cast_precision_loss,
    clippy::similar_names,
    clippy::too_many_lines
)]
#[test]
fn insufficient_input_pads_silence_and_keeps_one_interpolation_frame() {
    let mut rng = XorShift64(0xBADC_0FFE_2026_0003);
    for scenario in 0..10_000 {
        let &(input_rate, output_rate) = pick(&mut rng, &RATE_PAIRS);
        let &channels = pick(&mut rng, &CHANNELS);
        let ppm = i32::try_from(rng.next_usize(4_001)).unwrap() - 2_000;
        let feed_frames = rng.next_usize(18);

        let mut resampler = LinearResampler::new(input_rate, output_rate, channels).unwrap();
        resampler.set_rate_adjustment_ppm(ppm).unwrap();
        let mut model = ResamplerModel::new(input_rate, output_rate, usize::from(channels));
        model.set_rate_adjustment_ppm(ppm);

        let input: Vec<f32> = (0..feed_frames * usize::from(channels))
            .map(|i| source_sample(i / usize::from(channels), i % usize::from(channels)))
            .collect();
        let out_frames = 200 + rng.next_usize(800);
        let mut actual = vec![f32::NAN; out_frames * usize::from(channels)];
        let mut expected = vec![f32::NAN; out_frames * usize::from(channels)];
        let actual_outcome = resampler.process(&input, &mut actual).unwrap();
        let expected_outcome = model.process(&input, &mut expected);

        let context = format!(
            "scenario {scenario} rates {input_rate}->{output_rate} ch {channels} \
             ppm {ppm} feed {feed_frames} out {out_frames}"
        );
        assert_eq!(actual_outcome, expected_outcome, "{context}");
        assert_samples_close(&actual, &expected, &context);

        let produced = actual_outcome.produced_output_frames;
        let consumed = actual_outcome.consumed_input_frames;
        // The tail past the produced frames is untouched silence.
        assert!(
            actual[produced * usize::from(channels)..]
                .iter()
                .all(|sample| sample.abs() < f32::EPSILON),
            "trailing output is not silence: {context}"
        );
        // One frame is always retained for the next interpolation pair.
        if feed_frames >= 1 {
            assert!(
                feed_frames - consumed >= 1,
                "no retained interpolation frame: {context}"
            );
        }
        if feed_frames >= 2 {
            assert!(produced >= 1, "no progress with two frames: {context}");
        }
        if feed_frames == 0 {
            assert_eq!(produced, 0, "empty input produced output: {context}");
            assert_eq!(consumed, 0, "empty input consumed frames: {context}");
        }
    }
}
