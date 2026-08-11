use std::sync::Arc;

use sonimesh_audio::{
    GainSmoother, MAX_PCM_QUEUE_BLOCKS, RealtimePcmConsumer, RealtimePcmPopOutcome,
};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::RenderSource;

use crate::GainControlHandle;

const DISCONTINUITY_FADE_SAMPLES: u16 = 128;
const GAIN_RAMP_SAMPLES: u16 = 128;

/// Realtime output source backed by one stream's decoded PCM queue.
///
/// PCM is expected to be interleaved and already mapped to the negotiated
/// output layout. The callback only copies queued samples, advances a local
/// block cursor, and emits bounded silence when the queue cannot keep up.
pub struct PcmRenderSource<const SAMPLES_PER_BLOCK: usize> {
    consumer: RealtimePcmConsumer<SAMPLES_PER_BLOCK>,
    scratch: [f32; SAMPLES_PER_BLOCK],
    scratch_len: usize,
    scratch_offset: usize,
    discontinuity_fade: GainSmoother,
    gain_smoother: GainSmoother,
    gain: GainControlHandle,
    counters: Arc<StreamCounters>,
}

impl<const SAMPLES_PER_BLOCK: usize> PcmRenderSource<SAMPLES_PER_BLOCK> {
    /// Creates a callback source from the sole consumer endpoint of a PCM queue.
    ///
    /// The supplied [`GainControlHandle`] is shared with the desktop control
    /// domain, so a user's gain and mute choice survives output recreation.
    #[must_use]
    pub fn new(
        consumer: RealtimePcmConsumer<SAMPLES_PER_BLOCK>,
        counters: Arc<StreamCounters>,
        gain: GainControlHandle,
    ) -> Self {
        Self {
            consumer,
            scratch: [0.0; SAMPLES_PER_BLOCK],
            scratch_len: 0,
            scratch_offset: 0,
            discontinuity_fade: GainSmoother::new(1.0),
            gain_smoother: GainSmoother::new(1.0),
            gain,
            counters,
        }
    }

    /// Returns a clone of the gain and mute handle shared with this source.
    #[must_use]
    pub fn gain_handle(&self) -> GainControlHandle {
        self.gain.clone()
    }

    /// Applies the published gain or mute target with a bounded block ramp.
    ///
    /// The ramp restarts only when the target changes, so an unchanged gain
    /// never resets a partial ramp or prevents convergence.
    fn apply_gain(&mut self, samples: &mut [f32]) {
        let state = self.gain.snapshot();
        let target = if state.muted { 0.0 } else { state.gain };
        // Both values originate from the handle's atomic bit storage, so a
        // bit-exact comparison is the same as an exact value comparison.
        if target.to_bits() != self.gain_smoother.target().to_bits() {
            self.gain_smoother.set_target(target, GAIN_RAMP_SAMPLES);
        }
        self.gain_smoother.apply(samples);
    }
}

impl<const SAMPLES_PER_BLOCK: usize> RenderSource for PcmRenderSource<SAMPLES_PER_BLOCK> {
    fn render(&mut self, output: &mut [f32]) {
        output.fill(0.0);
        let mut written = 0;
        let mut refills = 0;

        while written < output.len() {
            if self.scratch_offset == self.scratch_len {
                if refills == MAX_PCM_QUEUE_BLOCKS {
                    self.counters.pcm_refill_limit_hit();
                    break;
                }
                refills += 1;
                match self.consumer.pop_into(&mut self.scratch) {
                    RealtimePcmPopOutcome::Filled {
                        samples,
                        discontinuity,
                        ..
                    } => {
                        if discontinuity {
                            self.counters.pcm_discontinuity();
                            self.discontinuity_fade = GainSmoother::new(0.0);
                            self.discontinuity_fade
                                .set_target(1.0, DISCONTINUITY_FADE_SAMPLES);
                        }
                        self.scratch_len = samples;
                        self.scratch_offset = 0;
                        if samples == 0 {
                            continue;
                        }
                    }
                    RealtimePcmPopOutcome::Underflow { .. } => {
                        self.counters.pcm_underflow();
                        break;
                    }
                }
            }

            let available = self.scratch_len - self.scratch_offset;
            let copy_len = available.min(output.len() - written);
            output[written..written + copy_len].copy_from_slice(
                &self.scratch[self.scratch_offset..self.scratch_offset + copy_len],
            );
            self.discontinuity_fade
                .apply(&mut output[written..written + copy_len]);
            self.apply_gain(&mut output[written..written + copy_len]);
            written += copy_len;
            self.scratch_offset += copy_len;
        }
    }
}
