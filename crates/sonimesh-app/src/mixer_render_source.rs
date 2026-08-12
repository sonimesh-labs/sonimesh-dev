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
