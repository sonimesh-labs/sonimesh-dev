use std::sync::Arc;

use sonimesh_audio::{
    GainSmoother, MAX_PCM_QUEUE_BLOCKS, RealtimePcmConsumer, RealtimePcmPopOutcome,
};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::RenderSource;

const DISCONTINUITY_FADE_SAMPLES: u16 = 128;

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
    counters: Arc<StreamCounters>,
}

impl<const SAMPLES_PER_BLOCK: usize> PcmRenderSource<SAMPLES_PER_BLOCK> {
    /// Creates a callback source from the sole consumer endpoint of a PCM queue.
    #[must_use]
    pub fn new(
        consumer: RealtimePcmConsumer<SAMPLES_PER_BLOCK>,
        counters: Arc<StreamCounters>,
    ) -> Self {
        Self {
            consumer,
            scratch: [0.0; SAMPLES_PER_BLOCK],
            scratch_len: 0,
            scratch_offset: 0,
            discontinuity_fade: GainSmoother::new(1.0),
            counters,
        }
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
            written += copy_len;
            self.scratch_offset += copy_len;
        }
    }
}
