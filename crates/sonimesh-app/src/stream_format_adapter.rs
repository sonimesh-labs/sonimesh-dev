//! Decode-domain format adapter between a decoded source and a mixer slot.
//!
//! ADR 0015's Phase 2 mixer is a plain summing bus: every slot queue holds
//! output-format interleaved PCM at the platform output's sample rate and
//! channel layout. Decoded streams arrive in their negotiated source format
//! (for example 48 kHz stereo Opus), so each stream needs a bounded
//! source-to-output conversion before its blocks enter the mixer queue. This
//! module provides that per-stream pump: it drains decoded source blocks,
//! applies nominal sample-rate conversion followed by channel mapping, and
//! pushes bounded output-format blocks into the stream's mixer slot queue.
//!
//! The pump runs in the decode/control domain, never inside the output
//! callback. All conversion scratch is allocated at construction and the pump
//! performs no locking or allocation, so the realtime side of the boundary
//! stays a plain read of pre-adapted output-format blocks.

use core::fmt;

use sonimesh_audio::{
    ChannelMapper, ChannelMappingError, LinearResampler, MAX_DRIFT_ADJUSTMENT_PPM,
    RealtimePcmConsumer, RealtimePcmPopOutcome, RealtimePcmProducer, RealtimePcmPushOutcome,
    ResamplerError,
};

use crate::SourcePcmFormat;

/// Failure while constructing or running one stream format adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamFormatAdapterError {
    /// The source or output rate or channel layout is outside the shared bounds.
    Resampler(ResamplerError),
    /// The source or output channel layout is outside the shared bounds.
    Mapping(ChannelMappingError),
    /// A source or output PCM block must hold at least one sample.
    ZeroSamplesPerBlock,
    /// A source or output PCM block cannot hold one frame of its layout.
    BlockTooSmallForLayout,
    /// The preallocated conversion scratch cannot be represented on this target.
    ScratchSizeOverflow,
    /// A bounded queue write rejected the conversion block.
    Queue(RealtimePcmPushOutcome),
}

impl fmt::Display for StreamFormatAdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resampler(error) => write!(formatter, "stream format conversion failed: {error}"),
            Self::Mapping(error) => write!(formatter, "stream channel mapping failed: {error}"),
            Self::ZeroSamplesPerBlock => {
                formatter.write_str("format adapter PCM blocks must hold at least one sample")
            }
            Self::BlockTooSmallForLayout => {
                formatter.write_str("format adapter PCM block cannot hold one frame")
            }
            Self::ScratchSizeOverflow => {
                formatter.write_str("format adapter conversion scratch size overflowed")
            }
            Self::Queue(outcome) => write!(
                formatter,
                "format adapter output block rejected by the bounded queue: {outcome:?}"
            ),
        }
    }
}

impl std::error::Error for StreamFormatAdapterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Resampler(error) => Some(error),
            Self::Mapping(error) => Some(error),
            Self::ZeroSamplesPerBlock
            | Self::BlockTooSmallForLayout
            | Self::ScratchSizeOverflow
            | Self::Queue(_) => None,
        }
    }
}

/// Outcome of one format-adapter pump step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PumpOutcome {
    /// No output frame could be produced; the slot underflows if left unfed.
    Underflow,
    /// One output-format block was pushed to the mixer slot queue.
    Pushed {
        /// Output-format frames in the pushed block.
        frames: usize,
        /// Bounded queue write result.
        queue: RealtimePcmPushOutcome,
    },
}

/// Per-stream bounded source-to-output format pump.
///
/// Owned by the decode/control domain. [`Self::pump`] drains decoded
/// source-format blocks, converts them to the output format, and pushes
/// bounded output-format blocks into the paired mixer slot queue; the output
/// callback never touches this type.
pub struct StreamFormatAdapter<
    const SOURCE_SAMPLES_PER_BLOCK: usize,
    const OUTPUT_SAMPLES_PER_BLOCK: usize,
> {
    source: RealtimePcmConsumer<SOURCE_SAMPLES_PER_BLOCK>,
    output: RealtimePcmProducer<OUTPUT_SAMPLES_PER_BLOCK>,
    resampler: LinearResampler,
    mapper: ChannelMapper,
    source_channels: usize,
    output_channels: usize,
    max_block_frames: usize,
    input_scratch: Box<[f32]>,
    input_frames: usize,
    source_block: [f32; SOURCE_SAMPLES_PER_BLOCK],
    resample_block: Box<[f32]>,
    mapped_block: Box<[f32]>,
}

impl<const SOURCE_SAMPLES_PER_BLOCK: usize, const OUTPUT_SAMPLES_PER_BLOCK: usize>
    StreamFormatAdapter<SOURCE_SAMPLES_PER_BLOCK, OUTPUT_SAMPLES_PER_BLOCK>
{
    /// Creates one format adapter bridging a decoded source queue to a mixer
    /// slot queue.
    ///
    /// All conversion scratch is allocated here; [`Self::pump`] performs no
    /// allocation.
    ///
    /// # Errors
    ///
    /// Returns an error before any conversion when a rate, channel layout, or
    /// block bound is invalid or the scratch size cannot be represented.
    pub fn new(
        source: RealtimePcmConsumer<SOURCE_SAMPLES_PER_BLOCK>,
        output: RealtimePcmProducer<OUTPUT_SAMPLES_PER_BLOCK>,
        source_format: SourcePcmFormat,
        output_format: SourcePcmFormat,
    ) -> Result<Self, StreamFormatAdapterError> {
        if SOURCE_SAMPLES_PER_BLOCK == 0 || OUTPUT_SAMPLES_PER_BLOCK == 0 {
            return Err(StreamFormatAdapterError::ZeroSamplesPerBlock);
        }
        let source_channels = usize::from(source_format.channels);
        let output_channels = usize::from(output_format.channels);
        let max_block_frames = OUTPUT_SAMPLES_PER_BLOCK / output_channels;
        if max_block_frames == 0 {
            return Err(StreamFormatAdapterError::BlockTooSmallForLayout);
        }

        let mut resampler = LinearResampler::new(
            source_format.sample_rate_hz,
            output_format.sample_rate_hz,
            source_format.channels,
        )
        .map_err(StreamFormatAdapterError::Resampler)?;
        let mapper = ChannelMapper::new(source_format.channels, output_format.channels)
            .map_err(StreamFormatAdapterError::Mapping)?;

        // The largest positive drift correction consumes input fastest, so
        // sizing the input scratch for that bound keeps every later pump
        // within the preallocated storage.
        resampler
            .set_rate_adjustment_ppm(MAX_DRIFT_ADJUSTMENT_PPM)
            .map_err(StreamFormatAdapterError::Resampler)?;
        let worst_required_frames = resampler
            .input_frames_for_output(max_block_frames)
            .ok_or(StreamFormatAdapterError::ScratchSizeOverflow)?;
        resampler
            .set_rate_adjustment_ppm(0)
            .map_err(StreamFormatAdapterError::Resampler)?;

        let source_block_frames = SOURCE_SAMPLES_PER_BLOCK / source_channels;
        if source_block_frames == 0 {
            return Err(StreamFormatAdapterError::BlockTooSmallForLayout);
        }
        let input_capacity_frames = worst_required_frames
            .checked_add(source_block_frames)
            .ok_or(StreamFormatAdapterError::ScratchSizeOverflow)?;
        let input_samples = input_capacity_frames
            .checked_mul(source_channels)
            .ok_or(StreamFormatAdapterError::ScratchSizeOverflow)?;
        let resample_samples = max_block_frames
            .checked_mul(source_channels)
            .ok_or(StreamFormatAdapterError::ScratchSizeOverflow)?;
        let mapped_samples = max_block_frames
            .checked_mul(output_channels)
            .ok_or(StreamFormatAdapterError::ScratchSizeOverflow)?;

        Ok(Self {
            source,
            output,
            resampler,
            mapper,
            source_channels,
            output_channels,
            max_block_frames,
            input_scratch: vec![0.0; input_samples].into_boxed_slice(),
            input_frames: 0,
            source_block: [0.0; SOURCE_SAMPLES_PER_BLOCK],
            resample_block: vec![0.0; resample_samples].into_boxed_slice(),
            mapped_block: vec![0.0; mapped_samples].into_boxed_slice(),
        })
    }

    /// Returns the interleaved channel count consumed from the decoded source.
    #[must_use]
    pub const fn source_channels(&self) -> u16 {
        self.mapper.input_channels()
    }

    /// Returns the interleaved channel count pushed to the mixer slot queue.
    #[must_use]
    pub const fn output_channels(&self) -> u16 {
        self.mapper.output_channels()
    }

    /// Approximate decoded blocks still waiting for conversion.
    #[must_use]
    pub fn queued_source_blocks(&self) -> usize {
        self.source.active_len()
    }

    /// Approximate output-format blocks waiting for the mixer.
    #[must_use]
    pub fn queued_output_blocks(&self) -> usize {
        self.output.active_len()
    }

    /// Applies one bounded drift correction to the next conversion steps.
    ///
    /// # Errors
    ///
    /// Returns a resampler error when the correction is outside the shared
    /// drift bound.
    pub fn set_rate_adjustment_ppm(&mut self, ppm: i32) -> Result<(), ResamplerError> {
        self.resampler.set_rate_adjustment_ppm(ppm)
    }

    /// Drains available decoded source blocks and pushes at most one
    /// output-format block into the mixer slot queue.
    ///
    /// A source underflow or an input too short for one output frame returns
    /// [`PumpOutcome::Underflow`] and retains any partial input for the next
    /// call. A stream-local discontinuity discards retained input and resets
    /// the resampler so no stale samples cross the boundary.
    ///
    /// # Errors
    ///
    /// Returns an error when conversion or the bounded queue write fails; the
    /// adapter remains in a consistent state for a later call.
    pub fn pump(&mut self) -> Result<PumpOutcome, StreamFormatAdapterError> {
        let required_frames = self
            .resampler
            .input_frames_for_output(self.max_block_frames)
            .unwrap_or(self.max_block_frames);
        self.fill_input(required_frames);
        if self.input_frames == 0 {
            return Ok(PumpOutcome::Underflow);
        }

        let source_channels = self.source_channels;
        let outcome = self
            .resampler
            .process(
                &self.input_scratch[..self.input_frames * source_channels],
                &mut self.resample_block,
            )
            .map_err(StreamFormatAdapterError::Resampler)?;
        self.consume_input(outcome.consumed_input_frames);
        if outcome.produced_output_frames == 0 {
            return Ok(PumpOutcome::Underflow);
        }

        let produced = outcome.produced_output_frames;
        let mapped_samples = self
            .mapper
            .map(
                &self.resample_block[..produced * source_channels],
                &mut self.mapped_block[..produced * self.output_channels],
            )
            .map_err(StreamFormatAdapterError::Mapping)?;
        let queue = self.output.push(&self.mapped_block[..mapped_samples]);
        match queue {
            RealtimePcmPushOutcome::EmptyBlock | RealtimePcmPushOutcome::BlockTooLarge => {
                Err(StreamFormatAdapterError::Queue(queue))
            }
            RealtimePcmPushOutcome::Inserted | RealtimePcmPushOutcome::ReplacedOldest => {
                Ok(PumpOutcome::Pushed {
                    frames: produced,
                    queue,
                })
            }
        }
    }

    /// Retires partial conversion state and queued output blocks after a
    /// stream-local reset.
    ///
    /// The control domain calls this together with the stream decode worker's
    /// reset so the mixer discards blocks from the retired epoch.
    pub fn begin_epoch(&mut self) {
        self.input_frames = 0;
        self.resampler.reset();
        self.output.begin_epoch();
    }

    fn fill_input(&mut self, required_frames: usize) {
        let source_channels = self.source_channels;
        let capacity_frames = self.input_scratch.len() / source_channels;
        while self.input_frames < required_frames {
            let outcome = self.source.pop_into(&mut self.source_block);
            let RealtimePcmPopOutcome::Filled {
                samples,
                discontinuity,
                ..
            } = outcome
            else {
                return;
            };
            if discontinuity {
                self.input_frames = 0;
                self.resampler.reset();
            }
            let aligned = samples - samples % source_channels;
            if aligned == 0 {
                continue;
            }
            let block_frames = aligned / source_channels;
            let start = self.input_frames * source_channels;
            if start + aligned > self.input_scratch.len() {
                return;
            }
            self.input_scratch[start..start + aligned]
                .copy_from_slice(&self.source_block[..aligned]);
            self.input_frames += block_frames;
        }
        debug_assert!(self.input_frames <= capacity_frames);
    }

    fn consume_input(&mut self, frames: usize) {
        if frames == 0 {
            return;
        }
        let frames = frames.min(self.input_frames);
        let channels = self.source_channels;
        let occupied = self.input_frames * channels;
        self.input_scratch
            .copy_within(frames * channels..occupied, 0);
        self.input_frames -= frames;
    }
}

#[cfg(test)]
mod tests {
    use sonimesh_audio::{
        MAX_DRIFT_ADJUSTMENT_PPM, RealtimePcmConsumer, RealtimePcmPopOutcome, RealtimePcmProducer,
        RealtimePcmPushOutcome, realtime_pcm_queue,
    };

    use super::{PumpOutcome, StreamFormatAdapter, StreamFormatAdapterError};
    use crate::SourcePcmFormat;

    const SOURCE_SAMPLES: usize = 256;
    const OUTPUT_SAMPLES: usize = 256;

    fn stereo_48k() -> SourcePcmFormat {
        SourcePcmFormat::new(48_000, 2).expect("fixed test format is valid")
    }

    fn queues() -> (
        RealtimePcmProducer<SOURCE_SAMPLES>,
        RealtimePcmConsumer<SOURCE_SAMPLES>,
    ) {
        realtime_pcm_queue::<SOURCE_SAMPLES>(8).expect("bounded test queue is valid")
    }

    fn adapter(
        source: RealtimePcmConsumer<SOURCE_SAMPLES>,
        output: RealtimePcmProducer<OUTPUT_SAMPLES>,
    ) -> StreamFormatAdapter<SOURCE_SAMPLES, OUTPUT_SAMPLES> {
        StreamFormatAdapter::new(source, output, stereo_48k(), stereo_48k())
            .expect("fixed test adapter is valid")
    }

    #[test]
    fn new_rejects_an_invalid_source_layout() {
        let (_, source) = queues();
        let (output, _) = queues();
        let invalid = SourcePcmFormat {
            sample_rate_hz: 48_000,
            channels: 0,
        };

        assert!(matches!(
            StreamFormatAdapter::<SOURCE_SAMPLES, OUTPUT_SAMPLES>::new(
                source,
                output,
                invalid,
                stereo_48k()
            ),
            Err(StreamFormatAdapterError::Resampler(_))
        ));
    }

    #[test]
    fn new_rejects_a_block_too_small_for_the_output_layout() {
        let (_, source) = queues();
        let (output, _) = realtime_pcm_queue::<1>(8).expect("one-sample queue is valid");

        assert!(matches!(
            StreamFormatAdapter::<SOURCE_SAMPLES, 1>::new(
                source,
                output,
                stereo_48k(),
                stereo_48k()
            ),
            Err(StreamFormatAdapterError::BlockTooSmallForLayout)
        ));
    }

    #[test]
    fn pump_underflows_when_the_source_is_empty() {
        let (_, source) = queues();
        let (output, mut output_consumer) = queues();
        let mut adapter = adapter(source, output);

        assert_eq!(adapter.pump().unwrap(), PumpOutcome::Underflow);
        assert_eq!(adapter.queued_source_blocks(), 0);
        assert_eq!(adapter.queued_output_blocks(), 0);

        let mut out = [0.0; OUTPUT_SAMPLES];
        assert!(matches!(
            output_consumer.pop_into(&mut out),
            RealtimePcmPopOutcome::Underflow { .. }
        ));
    }

    #[test]
    fn pump_converts_a_source_block_into_a_pushed_output_block() {
        let (mut source_producer, source) = queues();
        let (output, mut output_consumer) = queues();
        let mut adapter = adapter(source, output);

        assert_eq!(
            source_producer.push(&[0.5; SOURCE_SAMPLES]),
            RealtimePcmPushOutcome::Inserted
        );
        let PumpOutcome::Pushed { frames, queue } = adapter.pump().unwrap() else {
            panic!("one complete source block must convert into an output block");
        };
        assert_eq!(queue, RealtimePcmPushOutcome::Inserted);
        assert!(frames > 0);
        assert_eq!(adapter.queued_output_blocks(), 1);

        let mut out = [0.0; OUTPUT_SAMPLES];
        let RealtimePcmPopOutcome::Filled { samples, .. } = output_consumer.pop_into(&mut out)
        else {
            panic!("pumped output block must be observable from the slot queue");
        };
        assert_eq!(samples, frames * 2);
        assert!(out[..samples].iter().all(|sample| *sample > 0.0));
    }

    #[test]
    fn pump_drains_available_source_blocks_into_conversion_scratch() {
        let (mut source_producer, source) = queues();
        let (output, _) = queues();
        let mut adapter = adapter(source, output);

        source_producer.push(&[0.25; SOURCE_SAMPLES]);
        source_producer.push(&[0.75; SOURCE_SAMPLES]);
        assert_eq!(adapter.queued_source_blocks(), 2);

        adapter.pump().unwrap();
        // Both complete blocks were drained into the adapter's bounded scratch.
        assert_eq!(adapter.queued_source_blocks(), 0);
    }

    #[test]
    fn begin_epoch_retires_queued_output_blocks() {
        let (mut source_producer, source) = queues();
        let (output, _) = queues();
        let mut adapter = adapter(source, output);

        source_producer.push(&[0.5; SOURCE_SAMPLES]);
        adapter.pump().unwrap();
        assert_eq!(adapter.queued_output_blocks(), 1);

        adapter.begin_epoch();
        assert_eq!(adapter.queued_output_blocks(), 0);
        assert_eq!(adapter.queued_source_blocks(), 0);
    }

    #[test]
    fn set_rate_adjustment_ppm_rejects_an_out_of_bound_correction() {
        let (_, source) = queues();
        let (output, _) = queues();
        let mut adapter = adapter(source, output);

        assert!(
            adapter
                .set_rate_adjustment_ppm(MAX_DRIFT_ADJUSTMENT_PPM + 1)
                .is_err()
        );
        assert!(adapter.set_rate_adjustment_ppm(0).is_ok());
    }

    #[test]
    fn error_display_is_actionable() {
        assert_eq!(
            StreamFormatAdapterError::ZeroSamplesPerBlock.to_string(),
            "format adapter PCM blocks must hold at least one sample"
        );
        assert_eq!(
            StreamFormatAdapterError::BlockTooSmallForLayout.to_string(),
            "format adapter PCM block cannot hold one frame"
        );
        assert_eq!(
            StreamFormatAdapterError::ScratchSizeOverflow.to_string(),
            "format adapter conversion scratch size overflowed"
        );
    }
}
