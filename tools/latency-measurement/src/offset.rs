//! Sample-offset estimation between a deterministic reference signal and a
//! recorded capture.
//!
//! The estimator correlates a leading probe of the reference against the
//! recording over a bounded search window. A coarse boxcar-downsampled pass
//! localizes the peak, a fine full-resolution pass finds the integer lag, and a
//! parabolic fit refines the result to a fractional sample. The offset is only
//! accepted when the peak is distinct by `distinctness_ratio`: at full
//! resolution within the fine window around the peak and within the strongest
//! coarse regions across the whole search window, so a strong secondary peak
//! elsewhere in the window rejects the estimate.

use std::fmt;

/// Configuration for [`estimate_offset_ms`].
#[derive(Clone, Copy, Debug)]
pub struct OffsetConfig {
    /// Sample rate shared by the reference and the recording, in hertz.
    pub sample_rate_hz: u32,
    /// How far after the recording start the reference may appear, in
    /// milliseconds.
    pub max_search_ms: f64,
    /// Boxcar downsampling factor used for the coarse search pass.
    pub coarse_factor: usize,
    /// Number of leading reference samples to correlate against the recording.
    pub probe_len: usize,
    /// Minimum ratio between the best and the strongest competing correlation
    /// score before an offset is reported as reliable. The competing score is
    /// measured at full resolution within the fine window around the peak and
    /// within the strongest coarse regions across the whole search window.
    pub distinctness_ratio: f64,
}

impl OffsetConfig {
    /// Returns a configuration with defaults for the given sample rate.
    ///
    /// The defaults are a one-second search window, a 16x coarse factor, a
    /// 32,768-sample probe, and a 3.0 distinctness ratio.
    #[must_use]
    pub fn new(sample_rate_hz: u32) -> Self {
        Self {
            sample_rate_hz,
            max_search_ms: 1_000.0,
            coarse_factor: 16,
            probe_len: 32_768,
            distinctness_ratio: 3.0,
        }
    }
}

/// A correlation-derived offset between a reference and a recording.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OffsetEstimate {
    /// Fractional sample lag at which the reference best matches the recording.
    pub lag_samples: f64,
    /// [`Self::lag_samples`] converted to milliseconds at the configured sample
    /// rate.
    pub offset_ms: f64,
    /// Raw correlation score at the best lag.
    pub peak_score: f64,
    /// Best score divided by the strongest competing score found away from the
    /// peak, or infinity when no positive competing score exists. The competing
    /// score is measured at full resolution within the fine window around the
    /// peak and within the strongest coarse regions across the whole search
    /// window.
    pub distinctness_ratio: f64,
}

/// Failure while estimating a correlation offset.
#[derive(Clone, Debug, PartialEq)]
pub enum OffsetError {
    /// The configured sample rate is zero.
    ZeroSampleRate,
    /// A search or distinctness bound is non-finite or negative.
    InvalidConfig,
    /// The reference signal contains no samples.
    EmptyReference,
    /// The recording contains no samples.
    EmptyRecording,
    /// The recording is shorter than the correlation probe.
    RecordingTooShort {
        /// Recording length in samples.
        recording_len: usize,
        /// Probe length in samples.
        probe_len: usize,
    },
    /// The search window contains no whole-sample lags.
    EmptySearchWindow,
    /// The probe is too short to downsample with the configured coarse factor.
    ProbeTooShort {
        /// Probe length in samples.
        probe_len: usize,
        /// Coarse downsampling factor.
        coarse_factor: usize,
    },
    /// No lag produced a peak distinct enough to trust.
    NoDistinctPeak {
        /// Best lag found in samples.
        best_lag_samples: usize,
        /// Best correlation score found.
        best_score: f64,
        /// Best correlation score found away from the peak.
        second_best_score: f64,
    },
}

impl fmt::Display for OffsetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroSampleRate => formatter.write_str("sample rate must be non-zero"),
            Self::InvalidConfig => {
                formatter.write_str("offset configuration must use finite, non-negative bounds")
            }
            Self::EmptyReference => formatter.write_str("reference signal is empty"),
            Self::EmptyRecording => formatter.write_str("recording is empty"),
            Self::RecordingTooShort {
                recording_len,
                probe_len,
            } => write!(
                formatter,
                "recording has {recording_len} samples but the probe needs {probe_len}"
            ),
            Self::EmptySearchWindow => {
                formatter.write_str("search window contains no whole-sample lags")
            }
            Self::ProbeTooShort {
                probe_len,
                coarse_factor,
            } => write!(
                formatter,
                "probe of {probe_len} samples is shorter than coarse factor {coarse_factor}"
            ),
            Self::NoDistinctPeak {
                best_lag_samples,
                best_score,
                second_best_score,
            } => write!(
                formatter,
                "no distinct correlation peak: best lag {best_lag_samples} with score \
                 {best_score:.3} versus {second_best_score:.3} away from the peak"
            ),
        }
    }
}

impl std::error::Error for OffsetError {}

/// Minimum distance, in samples, between the peak and the score used as the
/// distinctness baseline.
const PEAK_SEPARATION_SAMPLES: usize = 16;

/// Estimates the offset, in milliseconds, between `reference` and `recording`.
///
/// `reference` must start with the deterministic test signal (for example the
/// output of [`crate::signal::test_signal`]) and `recording` must contain that
/// signal shifted no further than `config.max_search_ms` past its start. Both
/// are assumed to share `config.sample_rate_hz`.
///
/// The probe is the first `config.probe_len` reference samples. When the search
/// window is wide enough, the probe and the recording are first
/// boxcar-downsampled by `config.coarse_factor` to locate the peak cheaply, and
/// the exact lag is then found at full resolution around that location. The
/// returned lag includes a parabolic sub-sample refinement.
///
/// The estimate is only accepted when the peak is distinct by
/// `config.distinctness_ratio`: at full resolution within the fine window
/// around the peak and within the strongest coarse regions across the whole
/// search window.
///
/// # Errors
///
/// Returns [`OffsetError`] when the configuration uses non-finite or negative
/// bounds, the inputs are too short, the search window is empty, or no
/// correlation peak is distinct enough to trust.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::too_many_lines
)]
pub fn estimate_offset_ms(
    config: &OffsetConfig,
    reference: &[f32],
    recording: &[f32],
) -> Result<OffsetEstimate, OffsetError> {
    if config.sample_rate_hz == 0 {
        return Err(OffsetError::ZeroSampleRate);
    }
    if !config.max_search_ms.is_finite()
        || config.max_search_ms < 0.0
        || !config.distinctness_ratio.is_finite()
        || config.distinctness_ratio < 0.0
    {
        return Err(OffsetError::InvalidConfig);
    }
    if reference.is_empty() {
        return Err(OffsetError::EmptyReference);
    }
    if recording.is_empty() {
        return Err(OffsetError::EmptyRecording);
    }

    let probe_len = config.probe_len.min(reference.len());
    if recording.len() < probe_len {
        return Err(OffsetError::RecordingTooShort {
            recording_len: recording.len(),
            probe_len,
        });
    }

    let max_lag =
        (config.max_search_ms * f64::from(config.sample_rate_hz) / 1_000.0).round() as usize;
    if max_lag == 0 {
        return Err(OffsetError::EmptySearchWindow);
    }
    // Lags beyond the recording with a full probe overlap are not searchable.
    let search_lags = max_lag.min(recording.len() - probe_len);
    if search_lags == 0 {
        return Err(OffsetError::EmptySearchWindow);
    }

    let probe = &reference[..probe_len];
    let coarse_factor = config.coarse_factor.max(1);
    // Saturating arithmetic keeps an extreme caller-supplied coarse factor
    // from overflowing; the fine search remains bounded by the recording.
    let twice_coarse = 2_usize.saturating_mul(coarse_factor);
    let (coarse, low, high) = if search_lags > 4_usize.saturating_mul(coarse_factor) {
        if probe_len < coarse_factor {
            return Err(OffsetError::ProbeTooShort {
                probe_len,
                coarse_factor,
            });
        }
        let coarse_probe = downsample(probe, coarse_factor);
        let coarse_recording = downsample(&recording[..search_lags + probe_len], coarse_factor);
        let coarse = coarse_search(
            &coarse_probe,
            &coarse_recording,
            search_lags / coarse_factor,
        );
        let center = coarse.best_lag.saturating_mul(coarse_factor);
        (
            Some(coarse),
            center.saturating_sub(twice_coarse),
            center.saturating_add(twice_coarse).min(search_lags),
        )
    } else {
        (None, 0, search_lags)
    };

    let mut best_lag = low;
    let mut best_score = score(probe, &recording[low..low + probe_len]);
    for lag in (low + 1)..=high {
        let candidate = score(probe, &recording[lag..lag + probe_len]);
        if candidate > best_score {
            best_score = candidate;
            best_lag = lag;
        }
    }

    let mut second_best = f64::NEG_INFINITY;
    for lag in low..=high {
        if lag.abs_diff(best_lag) >= PEAK_SEPARATION_SAMPLES {
            second_best = second_best.max(score(probe, &recording[lag..lag + probe_len]));
        }
    }
    // The fine scan above only covered the fine window around the coarse peak.
    // Refine the strongest coarse regions away from the peak at full
    // resolution so a secondary peak anywhere in the search window competes
    // for the distinctness baseline. When the coarse pass did not run, the
    // fine scan already covered every lag in the window.
    if let Some(coarse) = &coarse {
        for &bucket in &coarse.away_buckets {
            let first = bucket.saturating_mul(coarse_factor);
            let last = first.saturating_add(coarse_factor).min(search_lags);
            for lag in first..last {
                if lag.abs_diff(best_lag) >= PEAK_SEPARATION_SAMPLES {
                    second_best = second_best.max(score(probe, &recording[lag..lag + probe_len]));
                }
            }
        }
    }
    if best_score <= 0.0
        || (second_best > 0.0 && best_score < config.distinctness_ratio * second_best)
    {
        return Err(OffsetError::NoDistinctPeak {
            best_lag_samples: best_lag,
            best_score,
            second_best_score: second_best,
        });
    }

    let mut lag_samples = best_lag as f64;
    if best_lag > low && best_lag < high {
        let left = score(probe, &recording[best_lag - 1..best_lag - 1 + probe_len]);
        let right = score(probe, &recording[best_lag + 1..best_lag + 1 + probe_len]);
        let denominator = left - 2.0 * best_score + right;
        if denominator.abs() > f64::EPSILON {
            lag_samples += (0.5 * (left - right) / denominator).clamp(-0.5, 0.5);
        }
    }

    let offset_ms = lag_samples * 1_000.0 / f64::from(config.sample_rate_hz);
    let distinctness_ratio = if second_best > 0.0 {
        best_score / second_best
    } else {
        f64::INFINITY
    };
    Ok(OffsetEstimate {
        lag_samples,
        offset_ms,
        peak_score: best_score,
        distinctness_ratio,
    })
}

/// Correlates `probe` with `window` and returns the accumulated score.
fn score(probe: &[f32], window: &[f32]) -> f64 {
    probe
        .iter()
        .zip(window)
        .map(|(&probe_sample, &window_sample)| f64::from(probe_sample) * f64::from(window_sample))
        .sum()
}

/// Averages non-overlapping `factor`-sample blocks into one coarse sample.
///
/// A trailing partial block is dropped.
#[must_use]
#[allow(clippy::cast_precision_loss)]
fn downsample(samples: &[f32], factor: usize) -> Vec<f32> {
    samples
        .chunks_exact(factor)
        .map(|block| block.iter().sum::<f32>() / factor as f32)
        .collect()
}

/// How many coarse regions away from the peak are refined at full resolution
/// to catch a secondary peak elsewhere in the search window.
const TOP_AWAY_BUCKETS: usize = 4;

/// Result of the coarse full-window search pass.
struct CoarseResult {
    /// Coarse lag (in coarse samples) with the highest correlation score.
    best_lag: usize,
    /// The [`TOP_AWAY_BUCKETS`] strongest coarse lags at least two buckets away
    /// from the peak, ordered by descending score. The caller refines these at
    /// full resolution for the distinctness baseline.
    away_buckets: [usize; TOP_AWAY_BUCKETS],
}

/// Finds the coarse lag with the highest correlation score and the strongest
/// [`TOP_AWAY_BUCKETS`] regions away from it.
///
/// Buckets adjacent to the peak bucket are skipped because the full-resolution
/// fine pass covers that neighborhood; every other bucket is a candidate
/// secondary peak anywhere in the search window.
#[must_use]
fn coarse_search(
    coarse_probe: &[f32],
    coarse_recording: &[f32],
    max_coarse_lag: usize,
) -> CoarseResult {
    let probe_len = coarse_probe.len();
    let max_valid = coarse_recording
        .len()
        .saturating_sub(probe_len)
        .min(max_coarse_lag);
    let mut best_lag = 0;
    let mut best_score = f64::NEG_INFINITY;
    for lag in 0..=max_valid {
        let candidate = score(coarse_probe, &coarse_recording[lag..lag + probe_len]);
        if candidate > best_score {
            best_score = candidate;
            best_lag = lag;
        }
    }
    let mut away_buckets = [0_usize; TOP_AWAY_BUCKETS];
    let mut away_scores = [f64::NEG_INFINITY; TOP_AWAY_BUCKETS];
    for lag in 0..=max_valid {
        if lag.abs_diff(best_lag) < 2 {
            continue;
        }
        let candidate = score(coarse_probe, &coarse_recording[lag..lag + probe_len]);
        for index in 0..TOP_AWAY_BUCKETS {
            if candidate > away_scores[index] {
                away_scores.copy_within(index..TOP_AWAY_BUCKETS - 1, index + 1);
                away_buckets.copy_within(index..TOP_AWAY_BUCKETS - 1, index + 1);
                away_scores[index] = candidate;
                away_buckets[index] = lag;
                break;
            }
        }
    }
    CoarseResult {
        best_lag,
        away_buckets,
    }
}
