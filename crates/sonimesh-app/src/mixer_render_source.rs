//! Callback-owned Phase 2 render source for the fixed-slot mixer.
//!
//! `MixerRenderSource` is the realtime output source of the Phase 2
//! multi-stream path: it owns the [`CommandDrivenMixer`] plus the
//! bounded-output limiter and renders by draining pending membership commands,
//! mixing every active slot in fixed order, and pulling the sum back inside the
//! limiter ceiling. The control domain shares the membership boundary through a
//! [`MixerMembershipHandle`]. Each slot's queue must already hold output-format
//! interleaved PCM (the same sample rate and channel layout as the platform
//! output), so the mixer stays a plain summing bus with no per-slot format
//! conversion inside the callback.

use core::fmt;

use sonimesh_audio::{CommandDrivenMixer, MixerMembershipHandle, PeakLimiter};
use sonimesh_platform::{OutputConfig, RenderSource};

/// Error while constructing the mixer-backed output source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MixerRenderSourceError {
    /// The selected output configuration violates a platform-neutral bound.
    InvalidConfig,
    /// The mixer rejected the requested slot or block configuration.
    MixerConfig,
}

impl fmt::Display for MixerRenderSourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig => formatter.write_str("invalid mixer output configuration"),
            Self::MixerConfig => formatter.write_str("invalid mixer slot or block configuration"),
        }
    }
}

impl std::error::Error for MixerRenderSourceError {}

/// Realtime output source owning the fixed-slot mixer and the bounded limiter.
///
/// Constructed once per platform output lifetime from the negotiated
/// [`OutputConfig`]. [`RenderSource::render`] never allocates, locks, decodes,
/// logs, or touches the network; it drains pending membership commands, mixes
/// active slots, and applies the limiter.
pub struct MixerRenderSource<const SLOTS: usize, const PCM_SAMPLES_PER_BLOCK: usize> {
    mixer: CommandDrivenMixer<SLOTS, PCM_SAMPLES_PER_BLOCK>,
    limiter: PeakLimiter,
    channels: u16,
}

impl<const SLOTS: usize, const PCM_SAMPLES_PER_BLOCK: usize>
    MixerRenderSource<SLOTS, PCM_SAMPLES_PER_BLOCK>
{
    /// Creates an empty mixer output source for one platform output lifetime.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when `config` violates the shared output
    /// bounds or the mixer rejects `SLOTS`/`PCM_SAMPLES_PER_BLOCK`.
    pub fn new(config: &OutputConfig) -> Result<Self, MixerRenderSourceError> {
        config
            .validate()
            .map_err(|_| MixerRenderSourceError::InvalidConfig)?;
        let mixer = CommandDrivenMixer::new().map_err(|_| MixerRenderSourceError::MixerConfig)?;
        Ok(Self {
            mixer,
            limiter: PeakLimiter::default(),
            channels: config.channels,
        })
    }

    /// Returns the control-domain membership handle sharing this source's
    /// command queue.
    #[must_use]
    pub fn membership(&self) -> MixerMembershipHandle<PCM_SAMPLES_PER_BLOCK> {
        self.mixer.membership()
    }

    /// Returns the interleaved channel count of this source's output bus.
    #[must_use]
    pub const fn channels(&self) -> u16 {
        self.channels
    }
}

impl<const SLOTS: usize, const PCM_SAMPLES_PER_BLOCK: usize> RenderSource
    for MixerRenderSource<SLOTS, PCM_SAMPLES_PER_BLOCK>
{
    fn render(&mut self, output: &mut [f32]) {
        self.mixer.render(output);
        if self
            .limiter
            .apply_interleaved(output, usize::from(self.channels))
            .is_err()
        {
            output.fill(0.0);
        }
    }
}
#[cfg(test)]
mod tests {
    use sonimesh_audio::{GainControlHandle, RealtimePcmPushOutcome, realtime_pcm_queue};
    use sonimesh_platform::{OutputConfig, RenderSource, SampleFormat};

    use super::{MixerRenderSource, MixerRenderSourceError};

    const SLOTS: usize = 2;
    const SAMPLES: usize = 256;

    fn config(channels: u16) -> OutputConfig {
        OutputConfig {
            sample_rate_hz: 48_000,
            channels,
            sample_format: SampleFormat::Float32,
            requested_callback_frames: Some(480),
            maximum_callback_frames: 1_024,
        }
    }

    fn source() -> MixerRenderSource<SLOTS, SAMPLES> {
        MixerRenderSource::new(&config(2)).expect("fixed test source is valid")
    }

    fn activated_source() -> (
        MixerRenderSource<SLOTS, SAMPLES>,
        sonimesh_audio::RealtimePcmProducer<SAMPLES>,
    ) {
        let mut source = source();
        let membership = source.membership();
        let (producer, consumer) =
            realtime_pcm_queue::<SAMPLES>(8).expect("bounded test queue is valid");
        assert_eq!(
            membership.insert(consumer, GainControlHandle::new(), None),
            Ok(())
        );
        assert_eq!(membership.pending_commands(), 1);
        source.render(&mut [0.0; SAMPLES]);
        (source, producer)
    }

    #[test]
    fn new_rejects_an_invalid_output_config() {
        assert!(matches!(
            MixerRenderSource::<SLOTS, SAMPLES>::new(&config(0)),
            Err(MixerRenderSourceError::InvalidConfig)
        ));
    }

    #[test]
    fn new_accepts_a_valid_config_and_exposes_channels() {
        let source = source();
        assert_eq!(source.channels(), 2);
        assert_eq!(source.membership().pending_commands(), 0);
    }

    #[test]
    fn render_starts_with_silence_and_applies_pending_commands() {
        let mut source = source();
        let membership = source.membership();
        let (_, consumer) = realtime_pcm_queue::<SAMPLES>(8).expect("bounded test queue is valid");
        membership
            .insert(consumer, GainControlHandle::new(), None)
            .unwrap();
        assert_eq!(membership.pending_commands(), 1);

        let mut output = [0.5; SAMPLES];
        source.render(&mut output);

        assert_eq!(membership.pending_commands(), 0);
        assert!(output.iter().all(|sample| sample.abs() < 1e-6));
    }

    #[test]
    fn render_mixes_an_inserted_slot_with_unity_gain() {
        let (mut source, mut producer) = activated_source();
        assert_eq!(
            producer.push(&[0.5; SAMPLES]),
            RealtimePcmPushOutcome::Inserted
        );

        let mut output = [0.0; SAMPLES];
        source.render(&mut output);

        assert!(output.iter().all(|sample| (sample - 0.5).abs() < 1e-6));
    }

    #[test]
    fn render_sums_multiple_active_slots() {
        let mut source = source();
        let membership = source.membership();
        let (mut first_producer, first_consumer) =
            realtime_pcm_queue::<SAMPLES>(8).expect("bounded test queue is valid");
        let (mut second_producer, second_consumer) =
            realtime_pcm_queue::<SAMPLES>(8).expect("bounded test queue is valid");
        membership
            .insert(first_consumer, GainControlHandle::new(), None)
            .unwrap();
        membership
            .insert(second_consumer, GainControlHandle::new(), None)
            .unwrap();
        source.render(&mut [0.0; SAMPLES]);

        first_producer.push(&[0.25; SAMPLES]);
        second_producer.push(&[0.5; SAMPLES]);

        let mut output = [0.0; SAMPLES];
        source.render(&mut output);

        assert!(output.iter().all(|sample| (sample - 0.75).abs() < 1e-6));
    }

    #[test]
    fn render_limits_the_sum_to_the_ceiling() {
        let (mut source, mut producer) = activated_source();
        assert_eq!(
            producer.push(&[2.0; SAMPLES]),
            RealtimePcmPushOutcome::Inserted
        );

        let mut output = [0.0; SAMPLES];
        source.render(&mut output);

        assert!(
            output
                .iter()
                .all(|sample| sample.is_finite() && *sample <= 1.0 + 1e-6)
        );
    }
}
