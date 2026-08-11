//! Deterministic property coverage for the bounded interleaved channel mapper.
//!
//! The mapper is the realtime channel-layout boundary between a negotiated
//! source layout and the output device layout: mono must duplicate to every
//! output channel, a smaller layout is copied and zero-extended, and a larger
//! layout is folded into the output by averaging the channels at the same
//! modulo position. This test drives every input/output layout pair through a
//! fixed pseudo-random sequence of frame counts and values against an
//! independent model and asserts every outcome and every output sample agree.

use sonimesh_audio::{ChannelMapper, ChannelMappingError, MAX_AUDIO_CHANNELS};

/// Tolerance for mapped samples. Both sides use the same ordered `f32` sums,
/// so results are bit-identical today; the tolerance keeps the test honest if
/// the mapping policy is re-expressed without changing its contract.
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

/// Bounded pseudo-random sample value in `[-2.0, 2.0)`.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn sample_value(rng: &mut XorShift64) -> f32 {
    let raw = f64::from(rng.next_u32()) / f64::from(u32::MAX);
    (raw as f32) * 4.0 - 2.0
}

/// Independent reference model for the documented mapping policy.
///
/// The implementation uses one indexing style with explicit while loops; this
/// model expresses the same policy with `copy_from_slice`, `fill`, and
/// `skip`/`step_by` iterators, so the implementation's branch and index
/// arithmetic cannot be copied by construction.
struct MapperModel {
    input_channels: usize,
    output_channels: usize,
}

impl MapperModel {
    fn new(input_channels: u16, output_channels: u16) -> Self {
        Self {
            input_channels: usize::from(input_channels),
            output_channels: usize::from(output_channels),
        }
    }

    #[allow(clippy::cast_precision_loss)]
    fn map(&self, input: &[f32], output: &mut [f32]) -> Result<usize, ChannelMappingError> {
        output.fill(0.0);
        let in_channels = self.input_channels;
        let out_channels = self.output_channels;
        if input.len() % in_channels != 0 {
            return Err(ChannelMappingError::InputNotFrameAligned);
        }
        let frames = input.len() / in_channels;
        let output_samples = frames
            .checked_mul(out_channels)
            .ok_or(ChannelMappingError::OutputTooSmall)?;
        if output_samples > output.len() {
            return Err(ChannelMappingError::OutputTooSmall);
        }
        for frame in 0..frames {
            let input_frame = &input[frame * in_channels..(frame + 1) * in_channels];
            let output_frame = &mut output[frame * out_channels..(frame + 1) * out_channels];
            if in_channels == out_channels {
                output_frame.copy_from_slice(input_frame);
            } else if in_channels == 1 {
                output_frame.fill(input_frame[0]);
            } else if out_channels == 1 {
                output_frame[0] = input_frame.iter().copied().sum::<f32>() / in_channels as f32;
            } else if in_channels < out_channels {
                output_frame[..in_channels].copy_from_slice(input_frame);
            } else {
                for (output_channel, sample) in output_frame.iter_mut().enumerate() {
                    let sum: f32 = input_frame
                        .iter()
                        .skip(output_channel)
                        .step_by(out_channels)
                        .sum();
                    let count = input_frame
                        .iter()
                        .skip(output_channel)
                        .step_by(out_channels)
                        .count();
                    *sample = sum / count as f32;
                }
            }
        }
        Ok(output_samples)
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

/// Drives every layout pair through aligned happy paths plus random lengths
/// that also exercise the alignment and capacity error paths, asserting both
/// sides return the identical outcome and identical output buffers.
///
/// Test values are tiny so the usize-to-float casts cannot lose precision, and
/// `mapper`/`model` plus `actual`/`expected` are short local names, not a
/// design smell, so the noisy pedantic casts and `similar_names` lints are
/// allowed here.
#[allow(
    clippy::cast_precision_loss,
    clippy::similar_names,
    clippy::too_many_lines
)]
#[test]
fn every_layout_pair_matches_an_independent_mapping_model() {
    let mut rng = XorShift64(0xCAFE_0000_2026_0004);
    for input_channels in 1..=MAX_AUDIO_CHANNELS {
        for output_channels in 1..=MAX_AUDIO_CHANNELS {
            let mapper = ChannelMapper::new(input_channels, output_channels).unwrap();
            let model = MapperModel::new(input_channels, output_channels);
            let layout = format!("layout {input_channels}->{output_channels}");

            // Aligned input with exactly-sized output: the mapping contract.
            for _ in 0..1_500 {
                let frames = rng.next_usize(17);
                let input: Vec<f32> = (0..frames * usize::from(input_channels))
                    .map(|_| sample_value(&mut rng))
                    .collect();
                let mut actual = vec![f32::NAN; frames * usize::from(output_channels)];
                let mut expected = vec![f32::NAN; frames * usize::from(output_channels)];
                let actual_outcome = mapper.map(&input, &mut actual).unwrap();
                let expected_outcome = model.map(&input, &mut expected).unwrap();
                assert_eq!(
                    actual_outcome,
                    frames * usize::from(output_channels),
                    "{layout} returned sample count"
                );
                assert_eq!(actual_outcome, expected_outcome, "{layout} outcome");
                assert_samples_close(
                    &actual,
                    &expected,
                    &format!("{layout} happy {frames} frames"),
                );
            }

            // Random input and output lengths: both sides must agree on the
            // exact outcome, including alignment and capacity errors, and the
            // output buffers must agree (silence on error) either way.
            for _ in 0..500 {
                let input: Vec<f32> = (0..rng.next_usize(34))
                    .map(|_| sample_value(&mut rng))
                    .collect();
                let mut actual = vec![f32::NAN; rng.next_usize(35)];
                let mut expected = vec![f32::NAN; actual.len()];
                let actual_outcome = mapper.map(&input, &mut actual);
                let expected_outcome = model.map(&input, &mut expected);
                assert_eq!(actual_outcome, expected_outcome, "{layout} mixed outcome");
                assert_samples_close(
                    &actual,
                    &expected,
                    &format!("{layout} mixed in {} out {}", input.len(), actual.len()),
                );
            }
        }
    }
}
