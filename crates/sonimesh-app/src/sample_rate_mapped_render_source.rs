use core::fmt;

use sonimesh_audio::{LinearResampler, ResamplerError};
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

/// Realtime-safe nominal sample-rate conversion source wrapper.
///
/// The wrapped source and this adapter use the same interleaved channel layout.
/// Input storage is allocated in bounded chunks during construction; render
/// performs only source pulls, compaction, and linear interpolation. This is a
/// nominal converter, not the final drift-control or quality-qualified ASRC.
pub struct SampleRateMappedRenderSource {
    source: Box<dyn RenderSource>,
    resampler: LinearResampler,
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

        Ok(Self {
            source,
            resampler,
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
