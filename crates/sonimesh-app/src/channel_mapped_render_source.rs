use core::fmt;

use sonimesh_audio::{ChannelMapper, ChannelMappingError};
use sonimesh_platform::{MAX_CALLBACK_FRAMES, RenderSource};

/// Error while constructing a bounded channel-mapping render source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelMappedRenderSourceError {
    /// The channel layouts are outside the shared audio bound.
    Mapping(ChannelMappingError),
    /// The source scratch bound is zero or exceeds the platform callback limit.
    InvalidMaximumCallbackFrames,
    /// The preallocated scratch size cannot be represented on this target.
    ScratchSizeOverflow,
}

impl fmt::Display for ChannelMappedRenderSourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mapping(error) => write!(formatter, "channel mapping failed: {error}"),
            Self::InvalidMaximumCallbackFrames => {
                formatter.write_str("invalid maximum callback frame bound")
            }
            Self::ScratchSizeOverflow => {
                formatter.write_str("channel mapping scratch size overflowed")
            }
        }
    }
}

impl std::error::Error for ChannelMappedRenderSourceError {}

/// Realtime-safe source wrapper that maps a decoded layout to the output layout.
///
/// All scratch storage is allocated during construction. `render` only fills
/// the fixed input scratch, applies the bounded mapper, and writes silence for
/// any callback tail or callback that exceeds the configured bound.
pub struct ChannelMappedRenderSource {
    source: Box<dyn RenderSource>,
    mapper: ChannelMapper,
    input_scratch: Box<[f32]>,
    maximum_output_samples: usize,
}

impl ChannelMappedRenderSource {
    /// Wraps a source with explicit input/output channels and a fixed callback bound.
    ///
    /// # Errors
    ///
    /// Returns an error before any realtime work when channel counts or the
    /// preallocation bound are invalid.
    pub fn new(
        source: Box<dyn RenderSource>,
        input_channels: u16,
        output_channels: u16,
        maximum_callback_frames: u32,
    ) -> Result<Self, ChannelMappedRenderSourceError> {
        let mapper = ChannelMapper::new(input_channels, output_channels)
            .map_err(ChannelMappedRenderSourceError::Mapping)?;
        if maximum_callback_frames == 0 || maximum_callback_frames > MAX_CALLBACK_FRAMES {
            return Err(ChannelMappedRenderSourceError::InvalidMaximumCallbackFrames);
        }

        let maximum_frames = usize::try_from(maximum_callback_frames)
            .map_err(|_| ChannelMappedRenderSourceError::ScratchSizeOverflow)?;
        let input_scratch_samples = maximum_frames
            .checked_mul(usize::from(mapper.input_channels()))
            .ok_or(ChannelMappedRenderSourceError::ScratchSizeOverflow)?;
        let maximum_output_samples = maximum_frames
            .checked_mul(usize::from(mapper.output_channels()))
            .ok_or(ChannelMappedRenderSourceError::ScratchSizeOverflow)?;

        Ok(Self {
            source,
            mapper,
            input_scratch: vec![0.0; input_scratch_samples].into_boxed_slice(),
            maximum_output_samples,
        })
    }

    /// Returns the input layout consumed from the wrapped source.
    #[must_use]
    pub const fn input_channels(&self) -> u16 {
        self.mapper.input_channels()
    }

    /// Returns the output layout written by this wrapper.
    #[must_use]
    pub const fn output_channels(&self) -> u16 {
        self.mapper.output_channels()
    }
}

impl RenderSource for ChannelMappedRenderSource {
    fn render(&mut self, output: &mut [f32]) {
        output.fill(0.0);
        let output_channels = usize::from(self.mapper.output_channels());
        let frames = output.len() / output_channels;
        let output_samples = frames * output_channels;
        if output_samples > self.maximum_output_samples {
            return;
        }

        let input_samples = frames * usize::from(self.mapper.input_channels());
        self.source.render(&mut self.input_scratch[..input_samples]);
        let _ = self.mapper.map(
            &self.input_scratch[..input_samples],
            &mut output[..output_samples],
        );
    }
}
