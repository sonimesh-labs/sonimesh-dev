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

impl std::error::Error for ChannelMappedRenderSourceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Mapping(error) => Some(error),
            Self::InvalidMaximumCallbackFrames | Self::ScratchSizeOverflow => None,
        }
    }
}

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
#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use sonimesh_audio::ChannelMappingError;
    use sonimesh_platform::{MAX_CALLBACK_FRAMES, RenderSource};

    use super::{ChannelMappedRenderSource, ChannelMappedRenderSourceError};

    struct FakeSource {
        value: f32,
        calls: Arc<AtomicUsize>,
    }

    impl FakeSource {
        fn new(value: f32, calls: Arc<AtomicUsize>) -> Self {
            Self { value, calls }
        }
    }

    impl RenderSource for FakeSource {
        fn render(&mut self, output: &mut [f32]) {
            self.calls.fetch_add(1, Ordering::Relaxed);
            output.fill(self.value);
        }
    }

    fn source(value: f32) -> ChannelMappedRenderSource {
        ChannelMappedRenderSource::new(
            Box::new(FakeSource::new(value, Arc::new(AtomicUsize::new(0)))),
            1,
            2,
            480,
        )
        .expect("fixed test source is valid")
    }

    #[test]
    fn new_rejects_invalid_channel_layouts() {
        assert!(matches!(
            ChannelMappedRenderSource::new(
                Box::new(FakeSource::new(0.5, Arc::new(AtomicUsize::new(0)))),
                0,
                2,
                480
            ),
            Err(ChannelMappedRenderSourceError::Mapping(
                ChannelMappingError::InvalidInputChannelCount
            ))
        ));
        assert!(matches!(
            ChannelMappedRenderSource::new(
                Box::new(FakeSource::new(0.5, Arc::new(AtomicUsize::new(0)))),
                1,
                0,
                480
            ),
            Err(ChannelMappedRenderSourceError::Mapping(
                ChannelMappingError::InvalidOutputChannelCount
            ))
        ));
    }

    #[test]
    fn new_rejects_zero_or_oversized_callback_bounds() {
        assert!(matches!(
            ChannelMappedRenderSource::new(
                Box::new(FakeSource::new(0.5, Arc::new(AtomicUsize::new(0)))),
                1,
                2,
                0
            ),
            Err(ChannelMappedRenderSourceError::InvalidMaximumCallbackFrames)
        ));
        assert!(matches!(
            ChannelMappedRenderSource::new(
                Box::new(FakeSource::new(0.5, Arc::new(AtomicUsize::new(0)))),
                1,
                2,
                MAX_CALLBACK_FRAMES + 1
            ),
            Err(ChannelMappedRenderSourceError::InvalidMaximumCallbackFrames)
        ));
    }

    #[test]
    fn new_exposes_the_wrapped_layouts() {
        let source = source(0.25);
        assert_eq!(source.input_channels(), 1);
        assert_eq!(source.output_channels(), 2);
    }

    #[test]
    fn render_maps_each_mono_frame_to_both_output_channels() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut source = ChannelMappedRenderSource::new(
            Box::new(FakeSource::new(0.25, Arc::clone(&calls))),
            1,
            2,
            480,
        )
        .unwrap();

        let mut output = [0.0; 256];
        source.render(&mut output);

        assert!(output.iter().all(|sample| (sample - 0.25).abs() < 1e-6));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn render_writes_silence_when_the_callback_exceeds_the_bound() {
        let mut source = ChannelMappedRenderSource::new(
            Box::new(FakeSource::new(0.25, Arc::new(AtomicUsize::new(0)))),
            1,
            2,
            4,
        )
        .unwrap();

        let mut output = [1.0; 16];
        source.render(&mut output);

        assert!(output.iter().all(|sample| sample.abs() < 1e-6));
    }

    #[test]
    fn render_leaves_a_tail_silent_for_a_partial_frame() {
        let mut source = source(0.25);

        let mut output = [0.5; 5];
        source.render(&mut output);

        assert!((output[0] - 0.25).abs() < 1e-6);
        assert!((output[3] - 0.25).abs() < 1e-6);
        assert!(output[4].abs() < 1e-6);
    }
}
