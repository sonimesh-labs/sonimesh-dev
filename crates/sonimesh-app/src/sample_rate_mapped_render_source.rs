use core::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use sonimesh_audio::{DriftDecision, DriftResyncDirection, MAX_DRIFT_ADJUSTMENT_PPM};
use sonimesh_audio::{LinearResampler, ResamplerError};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::{MAX_CALLBACK_FRAMES, RenderSource};

const OUTPUT_CHUNK_FRAMES: usize = 256;

/// Error while constructing a bounded sample-rate source adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SampleRateMappedRenderSourceError {
    /// The input/output rate or channel layout is outside the audio bound.
    Resampler(ResamplerError),
    /// The source scratch bound is zero or exceeds the platform callback limit.
    InvalidMaximumCallbackFrames,
    /// The preallocated scratch size cannot be represented on this target.
    ScratchSizeOverflow,
}

impl fmt::Display for SampleRateMappedRenderSourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resampler(error) => write!(formatter, "sample-rate conversion failed: {error}"),
            Self::InvalidMaximumCallbackFrames => {
                formatter.write_str("invalid maximum callback frame bound")
            }
            Self::ScratchSizeOverflow => {
                formatter.write_str("sample-rate conversion scratch size overflowed")
            }
        }
    }
}

impl std::error::Error for SampleRateMappedRenderSourceError {}

/// Failure while applying a non-realtime drift decision to a source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriftAdjustmentError {
    /// The requested ppm correction exceeds the shared bound.
    InvalidPpm,
    /// The stream owner must recreate or flush state before continuing.
    HardResyncRequired(DriftResyncDirection),
}

impl fmt::Display for DriftAdjustmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPpm => formatter.write_str("drift adjustment exceeds the ppm bound"),
            Self::HardResyncRequired(direction) => {
                write!(
                    formatter,
                    "drift controller requires a {direction:?} hard resync"
                )
            }
        }
    }
}

impl std::error::Error for DriftAdjustmentError {}

/// Control-domain handle for the source's bounded ppm correction.
#[derive(Clone)]
pub struct DriftAdjustmentHandle {
    ppm: Arc<AtomicI32>,
    metrics: Option<Arc<StreamCounters>>,
}

impl DriftAdjustmentHandle {
    /// Creates a control-domain adjustment handle with optional diagnostics.
    #[must_use]
    pub fn new(metrics: Option<Arc<StreamCounters>>) -> Self {
        Self {
            ppm: Arc::new(AtomicI32::new(0)),
            metrics,
        }
    }

    /// Applies a precomputed drift decision without touching source phase state.
    ///
    /// A hard resynchronization is returned to the stream owner instead of
    /// being silently applied to a running source.
    ///
    /// # Errors
    ///
    /// Returns [`DriftAdjustmentError`] for an invalid correction or hard reset.
    pub fn apply_decision(&self, decision: DriftDecision) -> Result<(), DriftAdjustmentError> {
        match decision {
            DriftDecision::Adjust { ppm } => self.set_ppm(ppm),
            DriftDecision::HardResync { direction } => {
                self.ppm.store(0, Ordering::Release);
                if let Some(metrics) = &self.metrics {
                    metrics.set_drift_adjustment_ppm(0);
                    metrics.drift_hard_resync();
                }
                Err(DriftAdjustmentError::HardResyncRequired(direction))
            }
        }
    }

    /// Stores one bounded ppm correction for the next source callback.
    ///
    /// # Errors
    ///
    /// Returns [`DriftAdjustmentError::InvalidPpm`] when the value is outside
    /// the shared controller bound.
    pub fn set_ppm(&self, ppm: i32) -> Result<(), DriftAdjustmentError> {
        if !(-MAX_DRIFT_ADJUSTMENT_PPM..=MAX_DRIFT_ADJUSTMENT_PPM).contains(&ppm) {
            return Err(DriftAdjustmentError::InvalidPpm);
        }
        self.ppm.store(ppm, Ordering::Release);
        if let Some(metrics) = &self.metrics {
            metrics.set_drift_adjustment_ppm(ppm);
        }
        Ok(())
    }

    /// Returns the last control-domain ppm value published to the source.
    #[must_use]
    pub fn current_ppm(&self) -> i32 {
        self.ppm.load(Ordering::Acquire)
    }

    pub(crate) fn publish_nominal_sample_rates(
        &self,
        input_sample_rate_hz: u32,
        output_sample_rate_hz: u32,
    ) {
        if let Some(metrics) = &self.metrics {
            metrics.set_nominal_sample_rates(input_sample_rate_hz, output_sample_rate_hz);
            metrics.set_drift_adjustment_ppm(self.current_ppm());
        }
    }

    pub(crate) fn without_metrics(&self) -> Self {
        Self {
            ppm: Arc::clone(&self.ppm),
            metrics: None,
        }
    }
}

/// Realtime-safe nominal sample-rate conversion source wrapper.
///
/// The wrapped source and this adapter use the same interleaved channel layout.
/// Input storage is allocated in bounded chunks during construction; render
/// performs only source pulls, compaction, and linear interpolation. This is a
/// nominal converter, not the final drift-control or quality-qualified ASRC.
pub struct SampleRateMappedRenderSource {
    source: Box<dyn RenderSource>,
    resampler: LinearResampler,
    drift_ppm: Arc<AtomicI32>,
    applied_drift_ppm: i32,
    input_scratch: Box<[f32]>,
    input_frames: usize,
    maximum_output_frames: usize,
}

impl SampleRateMappedRenderSource {
    /// Wraps a source with explicit rates, channels, and callback bounds.
    ///
    /// # Errors
    ///
    /// Returns an error before realtime work when rates, channels, or scratch
    /// bounds are invalid.
    pub fn new(
        source: Box<dyn RenderSource>,
        input_sample_rate_hz: u32,
        output_sample_rate_hz: u32,
        channels: u16,
        maximum_callback_frames: u32,
    ) -> Result<Self, SampleRateMappedRenderSourceError> {
        Self::new_with_drift_control(
            source,
            input_sample_rate_hz,
            output_sample_rate_hz,
            channels,
            maximum_callback_frames,
        )
        .map(|(source, _handle)| source)
    }

    /// Constructs a source and its control-domain drift handle.
    ///
    /// The returned handle may be retained by a stream owner. Applying a ppm
    /// decision only performs an atomic store; the callback observes it on its
    /// next invocation and updates local resampler state without a lock.
    ///
    /// # Errors
    ///
    /// Returns an error before realtime work when rates, channels, or scratch
    /// bounds are invalid.
    pub fn new_with_drift_control(
        source: Box<dyn RenderSource>,
        input_sample_rate_hz: u32,
        output_sample_rate_hz: u32,
        channels: u16,
        maximum_callback_frames: u32,
    ) -> Result<(Self, DriftAdjustmentHandle), SampleRateMappedRenderSourceError> {
        Self::new_with_drift_control_and_metrics(
            source,
            input_sample_rate_hz,
            output_sample_rate_hz,
            channels,
            maximum_callback_frames,
            None,
        )
    }

    /// Constructs a source and publishes its low-rate resampling metrics.
    ///
    /// The metrics object is updated only by control-domain construction and
    /// handle writes; the realtime callback never serializes or formats it.
    ///
    /// # Errors
    ///
    /// Returns an error before realtime work when rates, channels, or scratch
    /// bounds are invalid.
    pub fn new_with_drift_control_and_metrics(
        source: Box<dyn RenderSource>,
        input_sample_rate_hz: u32,
        output_sample_rate_hz: u32,
        channels: u16,
        maximum_callback_frames: u32,
        metrics: Option<Arc<StreamCounters>>,
    ) -> Result<(Self, DriftAdjustmentHandle), SampleRateMappedRenderSourceError> {
        let handle = DriftAdjustmentHandle::new(metrics);
        let source = Self::new_with_drift_handle(
            source,
            input_sample_rate_hz,
            output_sample_rate_hz,
            channels,
            maximum_callback_frames,
            &handle,
        )?;
        Ok((source, handle))
    }

    /// Constructs a source sharing an existing control-domain drift handle.
    ///
    /// Reusing the handle across output recreation preserves the latest bounded
    /// correction without putting controller state in the realtime callback.
    ///
    /// # Errors
    ///
    /// Returns an error before realtime work when rates, channels, or scratch
    /// bounds are invalid.
    pub fn new_with_drift_handle(
        source: Box<dyn RenderSource>,
        input_sample_rate_hz: u32,
        output_sample_rate_hz: u32,
        channels: u16,
        maximum_callback_frames: u32,
        handle: &DriftAdjustmentHandle,
    ) -> Result<Self, SampleRateMappedRenderSourceError> {
        let resampler = LinearResampler::new(input_sample_rate_hz, output_sample_rate_hz, channels)
            .map_err(SampleRateMappedRenderSourceError::Resampler)?;
        if maximum_callback_frames == 0 || maximum_callback_frames > MAX_CALLBACK_FRAMES {
            return Err(SampleRateMappedRenderSourceError::InvalidMaximumCallbackFrames);
        }

        let maximum_output_frames = usize::try_from(maximum_callback_frames)
            .map_err(|_| SampleRateMappedRenderSourceError::ScratchSizeOverflow)?;
        let chunk_frames = maximum_output_frames.min(OUTPUT_CHUNK_FRAMES);
        let input_capacity_frames = resampler
            .input_frames_for_output(chunk_frames)
            .ok_or(SampleRateMappedRenderSourceError::ScratchSizeOverflow)?;
        let input_samples = input_capacity_frames
            .checked_mul(usize::from(resampler.channels()))
            .ok_or(SampleRateMappedRenderSourceError::ScratchSizeOverflow)?;

        handle.publish_nominal_sample_rates(input_sample_rate_hz, output_sample_rate_hz);
        Ok(Self {
            source,
            resampler,
            drift_ppm: Arc::clone(&handle.ppm),
            applied_drift_ppm: 0,
            input_scratch: vec![0.0; input_samples].into_boxed_slice(),
            input_frames: 0,
            maximum_output_frames,
        })
    }

    /// Returns the fixed channel layout shared by the wrapped source and output.
    #[must_use]
    pub const fn channels(&self) -> u16 {
        self.resampler.channels()
    }

    fn ensure_input(&mut self, required_frames: usize) {
        if required_frames <= self.input_frames {
            return;
        }
        let capacity_frames = self.input_scratch.len() / usize::from(self.channels());
        let target_frames = required_frames.min(capacity_frames);
        if target_frames <= self.input_frames {
            return;
        }

        let channels = usize::from(self.channels());
        let start = self.input_frames * channels;
        let end = target_frames * channels;
        self.source.render(&mut self.input_scratch[start..end]);
        self.input_frames = target_frames;
    }

    fn consume_input(&mut self, frames: usize) {
        if frames == 0 {
            return;
        }
        let frames = frames.min(self.input_frames);
        let channels = usize::from(self.channels());
        let occupied_samples = self.input_frames * channels;
        self.input_scratch
            .copy_within(frames * channels..occupied_samples, 0);
        self.input_frames -= frames;
    }
}

impl RenderSource for SampleRateMappedRenderSource {
    fn render(&mut self, output: &mut [f32]) {
        output.fill(0.0);
        let requested_drift_ppm = self.drift_ppm.load(Ordering::Acquire);
        if requested_drift_ppm != self.applied_drift_ppm {
            if self
                .resampler
                .set_rate_adjustment_ppm(requested_drift_ppm)
                .is_err()
            {
                return;
            }
            self.applied_drift_ppm = requested_drift_ppm;
        }
        let channels = usize::from(self.channels());
        let output_frames = output.len() / channels;
        if output_frames > self.maximum_output_frames {
            return;
        }

        let mut rendered_frames = 0;
        while rendered_frames < output_frames {
            let chunk_frames = (output_frames - rendered_frames).min(OUTPUT_CHUNK_FRAMES);
            let required_input = self
                .resampler
                .input_frames_for_output(chunk_frames)
                .unwrap_or(self.input_scratch.len() / channels);
            self.ensure_input(required_input);

            let output_start = rendered_frames * channels;
            let output_end = output_start + chunk_frames * channels;
            let outcome = self.resampler.process(
                &self.input_scratch[..self.input_frames * channels],
                &mut output[output_start..output_end],
            );
            let Ok(outcome) = outcome else {
                return;
            };
            self.consume_input(outcome.consumed_input_frames);
            if outcome.produced_output_frames == 0 {
                return;
            }
            rendered_frames += outcome.produced_output_frames;
        }
    }
}
