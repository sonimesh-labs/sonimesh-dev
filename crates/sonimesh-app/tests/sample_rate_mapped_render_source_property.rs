//! Deterministic property coverage for the bounded sample-rate output adapter.
//!
//! The adapter is the realtime output path: it must convert a nominal
//! input/output sample-rate pair through bounded linear interpolation while a
//! control-domain handle publishes drift corrections, and it must never drop,
//! duplicate, or reorder input frames at internal 256-frame chunk boundaries.
//! This test drives random callback sizes and drift changes against an
//! independent continuous resampler model and asserts every rendered sample,
//! the retained-input accounting, and the handle's bounded-correction rules.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use sonimesh_app::{DriftAdjustmentError, DriftAdjustmentHandle, SampleRateMappedRenderSource};
use sonimesh_audio::{DriftDecision, DriftResyncDirection, MAX_DRIFT_ADJUSTMENT_PPM};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::RenderSource;

/// Composition-level sample tolerance. The adapter processes a render as
/// several internal 256-frame chunks while the independent model resamples the
/// same input in one call, so f64 phase bookkeeping rounds in a different
/// order and the interpolated f32 samples can differ by a few ulps (~2e-6 on
/// this bounded signal). A dropped, duplicated, or reordered frame shifts the
/// phase by at least one input frame and produces an O(1) sample difference,
/// far above this bound.
const SAMPLE_TOLERANCE: f32 = 1e-4;
const RENDERS_PER_SCENARIO: usize = 250;
/// The adapter chunks callbacks into 256-frame slices; renders up to this size
/// cross that boundary, exercising chunked pulls and retained-input compaction.
const MAXIMUM_CALLBACK_FRAMES: u32 = 512;
const MAX_RENDER_FRAMES: usize = MAXIMUM_CALLBACK_FRAMES as usize;
const RATE_PAIRS: [(u32, u32); 7] = [
    (48_000, 48_000),
    (48_000, 96_000),
    (96_000, 48_000),
    (44_100, 48_000),
    (48_000, 44_100),
    (8_000, 192_000),
    (192_000, 8_000),
];
const CHANNELS: [u16; 3] = [1, 2, 3];
const PPM_SETTINGS: [i32; 6] = [0, 2_000, -2_000, 137, -883, 999];

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

/// Deterministic stream sample: distinct per channel so interleaving bugs are
/// visible, and bounded so interpolation stays far from any float limit.
#[allow(clippy::cast_precision_loss)]
const fn source_sample(stream_frame: usize, channel: usize) -> f32 {
    ((stream_frame % 97) as f32) * 0.25 - 3.0 + (channel as f32) * 3.0
}

/// Position-deterministic source: every render fills the slice with the signal
/// at its current absolute stream position, so the independent model can
/// predict the exact source contribution without replicating the adapter's
/// chunking. The shared position counter also reports how many input frames
/// the adapter pulled during one render callback.
struct SignalSource {
    position: Arc<AtomicUsize>,
    channels: usize,
}

impl SignalSource {
    fn new(channels: u16) -> (Self, Arc<AtomicUsize>) {
        let position = Arc::new(AtomicUsize::new(0));
        (
            Self {
                position: Arc::clone(&position),
                channels: usize::from(channels),
            },
            position,
        )
    }
}

impl RenderSource for SignalSource {
    fn render(&mut self, interleaved_output: &mut [f32]) {
        let start = self.position.load(Ordering::Relaxed);
        let channels = self.channels;
        let frames = interleaved_output.len() / channels;
        for (index, sample) in interleaved_output.iter_mut().enumerate() {
            *sample = source_sample(start + index / channels, index % channels);
        }
        self.position.store(start + frames, Ordering::Relaxed);
    }
}

/// Independent reference model for the adapter's internal resampler.
///
/// Written only from the resampler contract: each output frame interpolates
/// between input frames `floor(phase)` and `floor(phase) + 1`, `phase` advances
/// by the rate ratio, and consumed frames are subtracted from `phase` at the
/// end of a call. The adapter interleaves this with chunked input pulls and
/// retained-input compaction; this model keeps the two concerns separate, so a
/// phase, retention, or channel-index mistake cannot be copied by construction.
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

    /// Resamples `input` into `output` and returns consumed input frames.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn process(&mut self, input: &[f32], output: &mut [f32]) -> usize {
        output.fill(0.0);
        let input_frames = input.len() / self.channels;
        let output_frames = output.len() / self.channels;
        let mut produced = 0;
        while produced < output_frames {
            let base = self.phase.floor() as usize;
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
        consumed
    }
}

/// Independent state model for the bounded drift-correction handle.
#[derive(Clone, Copy)]
struct HandleModel {
    ppm: i32,
    hard_resyncs: u64,
}

impl HandleModel {
    fn set_ppm(&mut self, ppm: i32) -> Result<(), DriftAdjustmentError> {
        if !(-MAX_DRIFT_ADJUSTMENT_PPM..=MAX_DRIFT_ADJUSTMENT_PPM).contains(&ppm) {
            return Err(DriftAdjustmentError::InvalidPpm);
        }
        self.ppm = ppm;
        Ok(())
    }

    fn apply_decision(&mut self, decision: DriftDecision) -> Result<(), DriftAdjustmentError> {
        match decision {
            DriftDecision::Adjust { ppm } => self.set_ppm(ppm),
            DriftDecision::HardResync { direction } => {
                self.ppm = 0;
                self.hard_resyncs += 1;
                Err(DriftAdjustmentError::HardResyncRequired(direction))
            }
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

/// Drives every rate pair and channel count through random callback sizes and
/// drift corrections, asserting every rendered sample against the independent
/// continuous resampler model. The model tracks its own retained-input cursor
/// (source position minus consumed frames), so dropped, duplicated, or
/// reordered frames at chunk boundaries cannot hide behind the adapter's
/// scratch bookkeeping.
#[allow(clippy::similar_names, clippy::too_many_lines)]
#[test]
fn chunked_rendering_matches_an_independent_continuous_resampler_model() {
    let mut rng = XorShift64(0xF00D_0000_2026_000D);

    for &(input_rate, output_rate) in &RATE_PAIRS {
        for &channels in &CHANNELS {
            let (signal, position) = SignalSource::new(channels);
            let (mut mapped, handle) = SampleRateMappedRenderSource::new_with_drift_control(
                Box::new(signal),
                input_rate,
                output_rate,
                channels,
                MAXIMUM_CALLBACK_FRAMES,
            )
            .unwrap();
            let mut model = ResamplerModel::new(input_rate, output_rate, usize::from(channels));
            // First unconsumed source frame, mirroring the adapter's retained
            // input position; both start at stream frame 0.
            let mut cursor = 0usize;
            let mut current_ppm = 0_i32;

            for step in 0..RENDERS_PER_SCENARIO {
                let roll = rng.next_u32() % 4;
                match roll {
                    0 => {
                        let ppm = PPM_SETTINGS[rng.next_usize(PPM_SETTINGS.len())];
                        handle.set_ppm(ppm).unwrap();
                        current_ppm = ppm;
                    }
                    1 => {
                        let out_of_range = MAX_DRIFT_ADJUSTMENT_PPM
                            + 1
                            + i32::try_from(rng.next_u32() % 100).expect("bounded by 100");
                        assert_eq!(
                            handle.set_ppm(out_of_range),
                            Err(DriftAdjustmentError::InvalidPpm)
                        );
                        assert_eq!(handle.current_ppm(), current_ppm);
                    }
                    _ => {}
                }

                let output_frames = 1 + rng.next_usize(MAX_RENDER_FRAMES);
                let mut output = vec![f32::NAN; output_frames * usize::from(channels)];
                mapped.render(&mut output);
                // The adapter over-pulls input (reserving the interpolation
                // pair per chunk) and retains leftovers, so a render may reuse
                // retained input without touching the source; `after` reports
                // the full input range available to this render either way.
                let after = position.load(Ordering::Relaxed);
                assert!(
                    output.iter().all(|sample| sample.is_finite()),
                    "render must fully initialize finite samples"
                );

                model.set_rate_adjustment_ppm(current_ppm);
                let mut expected = vec![0.0; output.len()];
                let input: Vec<f32> = (cursor..after)
                    .flat_map(|frame| {
                        (0..usize::from(channels)).map(move |channel| source_sample(frame, channel))
                    })
                    .collect();
                let consumed = model.process(&input, &mut expected);
                cursor += consumed;
                assert_samples_close(
                    &output,
                    &expected,
                    &format!("{input_rate}->{output_rate}, {channels} ch, render {step}"),
                );
            }
        }
    }
}

/// Drives random bounded corrections and hard resynchronizations against an
/// independent state model, asserting the published ppm, every outcome, and
/// the low-rate diagnostics counters agree.
#[allow(clippy::cast_possible_wrap, clippy::similar_names)]
#[test]
fn drift_adjustment_handle_matches_an_independent_bounded_correction_model() {
    let mut rng = XorShift64(0xD1A5_0000_2026_000E);
    let counters = Arc::new(StreamCounters::default());
    let handle = DriftAdjustmentHandle::new(Some(Arc::clone(&counters)));
    let mut model = HandleModel {
        ppm: 0,
        hard_resyncs: 0,
    };
    let spread = 2 * MAX_DRIFT_ADJUSTMENT_PPM as u32 + 100;

    for step in 0..20_000 {
        let roll = rng.next_u32() % 10;
        let (actual, expected) = match roll {
            0..=3 => {
                let ppm = (rng.next_u32() % spread) as i32 - MAX_DRIFT_ADJUSTMENT_PPM - 50;
                (handle.set_ppm(ppm), model.set_ppm(ppm))
            }
            4..=7 => {
                let ppm = (rng.next_u32() % spread) as i32 - MAX_DRIFT_ADJUSTMENT_PPM - 50;
                let decision = DriftDecision::Adjust { ppm };
                (
                    handle.apply_decision(decision),
                    model.apply_decision(decision),
                )
            }
            _ => {
                let direction = if rng.next_u32() % 2 == 0 {
                    DriftResyncDirection::High
                } else {
                    DriftResyncDirection::Low
                };
                let decision = DriftDecision::HardResync { direction };
                (
                    handle.apply_decision(decision),
                    model.apply_decision(decision),
                )
            }
        };
        assert_eq!(actual, expected, "outcome mismatch at step {step}");
        assert_eq!(handle.current_ppm(), model.ppm, "published ppm drift");
        let snapshot = counters.snapshot();
        assert_eq!(
            snapshot.drift_adjustment_ppm, model.ppm,
            "metrics ppm drift"
        );
        assert_eq!(
            snapshot.drift_hard_resyncs, model.hard_resyncs,
            "metrics resync drift"
        );
    }
}
