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
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sonimesh_audio::{
        MAX_PCM_QUEUE_BLOCKS, RealtimePcmConsumer, RealtimePcmPushOutcome, realtime_pcm_queue,
    };
    use sonimesh_diagnostics::StreamCounters;
    use sonimesh_platform::RenderSource;

    use super::PcmRenderSource;
    use crate::GainControlHandle;

    const SAMPLES: usize = 256;

    fn source<const N: usize>(
        consumer: RealtimePcmConsumer<N>,
        gain: GainControlHandle,
    ) -> PcmRenderSource<N> {
        PcmRenderSource::new(consumer, Arc::new(StreamCounters::default()), gain)
    }

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1e-6,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn render_copies_queued_blocks_and_reports_unity_gain() {
        let (mut producer, consumer) = realtime_pcm_queue::<SAMPLES>(4).unwrap();
        assert_eq!(
            producer.push(&[0.5; SAMPLES]),
            RealtimePcmPushOutcome::Inserted
        );
        let counters = Arc::new(StreamCounters::default());
        let mut source = PcmRenderSource::<SAMPLES>::new(
            consumer,
            Arc::clone(&counters),
            GainControlHandle::new(),
        );

        let mut output = [0.25; SAMPLES];
        source.render(&mut output);

        assert!(output.iter().all(|sample| (sample - 0.5).abs() < 1e-6));
        let handle = source.gain_handle();
        assert!((handle.gain() - 1.0).abs() < 1e-6);
        assert!(!handle.is_muted());

        let snapshot = counters.snapshot();
        assert_eq!(snapshot.pcm_underflows, 0);
        assert_eq!(snapshot.pcm_discontinuities, 0);
        assert_eq!(snapshot.pcm_refill_limit_hits, 0);
    }

    #[test]
    fn gain_handle_clone_publishes_into_the_same_stream() {
        let (_, consumer) = realtime_pcm_queue::<SAMPLES>(1).unwrap();
        let source = source(consumer, GainControlHandle::new());

        let control = source.gain_handle();
        assert_eq!(control.set_gain(0.25), Ok(()));
        control.set_muted(true);

        assert!((source.gain_handle().gain() - 0.25).abs() < 1e-6);
        assert!(source.gain_handle().is_muted());
    }

    #[test]
    fn underflow_writes_silence_and_increments_the_counter() {
        let (_, consumer) = realtime_pcm_queue::<SAMPLES>(2).unwrap();
        let counters = Arc::new(StreamCounters::default());
        let mut source = PcmRenderSource::<SAMPLES>::new(
            consumer,
            Arc::clone(&counters),
            GainControlHandle::new(),
        );

        let mut output = [0.25; SAMPLES];
        source.render(&mut output);

        assert!(output.iter().all(|sample| sample.abs() < 1e-6));
        assert_eq!(counters.snapshot().pcm_underflows, 1);
    }

    #[test]
    fn render_fills_a_larger_output_from_multiple_blocks() {
        let (mut producer, consumer) = realtime_pcm_queue::<SAMPLES>(4).unwrap();
        assert_eq!(
            producer.push(&[0.25; SAMPLES]),
            RealtimePcmPushOutcome::Inserted
        );
        assert_eq!(
            producer.push(&[0.75; SAMPLES]),
            RealtimePcmPushOutcome::Inserted
        );
        let mut source = source(consumer, GainControlHandle::new());

        let mut output = [0.0; SAMPLES * 2];
        source.render(&mut output);

        assert!(
            output[..SAMPLES]
                .iter()
                .all(|sample| (sample - 0.25).abs() < 1e-6)
        );
        assert!(
            output[SAMPLES..]
                .iter()
                .all(|sample| (sample - 0.75).abs() < 1e-6)
        );
    }

    #[test]
    fn render_continues_from_a_partial_scratch_block_before_refilling() {
        let (mut producer, consumer) = realtime_pcm_queue::<SAMPLES>(2).unwrap();
        assert_eq!(
            producer.push(&[0.5; SAMPLES]),
            RealtimePcmPushOutcome::Inserted
        );
        let counters = Arc::new(StreamCounters::default());
        let mut source = PcmRenderSource::<SAMPLES>::new(
            consumer,
            Arc::clone(&counters),
            GainControlHandle::new(),
        );

        let mut first = [0.0; 100];
        source.render(&mut first);
        assert!(first.iter().all(|sample| (sample - 0.5).abs() < 1e-6));

        let mut second = [0.0; 100];
        source.render(&mut second);
        assert!(second.iter().all(|sample| (sample - 0.5).abs() < 1e-6));

        let mut third = [0.0; 100];
        source.render(&mut third);
        assert!(third[..56].iter().all(|sample| (sample - 0.5).abs() < 1e-6));
        assert!(third[56..].iter().all(|sample| sample.abs() < 1e-6));
        assert_eq!(counters.snapshot().pcm_underflows, 1);
    }

    #[test]
    fn discontinuity_applies_a_bounded_fade_and_increments_the_counter() {
        let (mut producer, consumer) = realtime_pcm_queue::<SAMPLES>(2).unwrap();
        assert_eq!(
            producer.push(&[1.0; SAMPLES]),
            RealtimePcmPushOutcome::Inserted
        );
        let counters = Arc::new(StreamCounters::default());
        let mut source = PcmRenderSource::<SAMPLES>::new(
            consumer,
            Arc::clone(&counters),
            GainControlHandle::new(),
        );

        let mut drained = [0.0; SAMPLES];
        source.render(&mut drained);

        producer.begin_epoch();
        assert_eq!(
            producer.push(&[1.0; SAMPLES]),
            RealtimePcmPushOutcome::Inserted
        );

        let mut output = [0.0; SAMPLES];
        source.render(&mut output);

        assert_eq!(counters.snapshot().pcm_discontinuities, 1);
        assert_close(output[0], 1.0_f32 / 128.0);
        assert_close(output[127], 1.0);
        assert_close(output[SAMPLES - 1], 1.0);
        assert!(output[..128].windows(2).all(|pair| pair[1] >= pair[0]));
    }

    #[test]
    fn published_gain_ramps_to_the_new_target() {
        let (mut producer, consumer) = realtime_pcm_queue::<SAMPLES>(1).unwrap();
        assert_eq!(
            producer.push(&[1.0; SAMPLES]),
            RealtimePcmPushOutcome::Inserted
        );
        let gain = GainControlHandle::new();
        let mut source = source(consumer, gain.clone());
        assert_eq!(gain.set_gain(0.5), Ok(()));

        let mut output = [0.0; SAMPLES];
        source.render(&mut output);

        assert!(output[0] > 0.9);
        assert_close(output[127], 0.5);
        assert_close(output[SAMPLES - 1], 0.5);
    }

    #[test]
    fn mute_drives_the_output_to_silence() {
        let (mut producer, consumer) = realtime_pcm_queue::<SAMPLES>(1).unwrap();
        assert_eq!(
            producer.push(&[1.0; SAMPLES]),
            RealtimePcmPushOutcome::Inserted
        );
        let gain = GainControlHandle::new();
        let mut source = source(consumer, gain.clone());

        gain.set_muted(true);
        let mut output = [0.25; SAMPLES];
        source.render(&mut output);

        assert!(output[0] < 1.0);
        assert!(output[127].abs() < 1e-6);
        assert!(output[SAMPLES - 1].abs() < 1e-6);
        assert!(source.gain_handle().is_muted());
    }

    #[test]
    fn unchanged_gain_does_not_reset_a_partial_ramp() {
        let (mut producer, consumer) = realtime_pcm_queue::<SAMPLES>(1).unwrap();
        assert_eq!(
            producer.push(&[1.0; SAMPLES]),
            RealtimePcmPushOutcome::Inserted
        );
        let gain = GainControlHandle::new();
        let mut source = source(consumer, gain.clone());
        assert_eq!(gain.set_gain(0.5), Ok(()));

        let mut first = [0.0; 64];
        source.render(&mut first);
        assert_close(first[63], 0.75);

        let mut second = [0.0; 64];
        source.render(&mut second);
        assert!(second[0] < 0.75);
        assert_close(second[63], 0.5);
    }

    #[test]
    fn refill_budget_caps_the_queue_drain_and_counts_the_hit() {
        let (mut producer, consumer) = realtime_pcm_queue::<1>(MAX_PCM_QUEUE_BLOCKS).unwrap();
        for _ in 0..MAX_PCM_QUEUE_BLOCKS {
            assert_eq!(producer.push(&[1.0]), RealtimePcmPushOutcome::Inserted);
        }
        let counters = Arc::new(StreamCounters::default());
        let mut source =
            PcmRenderSource::<1>::new(consumer, Arc::clone(&counters), GainControlHandle::new());

        let mut output = [0.0; SAMPLES];
        source.render(&mut output);

        assert!(
            output[..MAX_PCM_QUEUE_BLOCKS]
                .iter()
                .all(|sample| (sample - 1.0).abs() < 1e-6)
        );
        assert!(
            output[MAX_PCM_QUEUE_BLOCKS..]
                .iter()
                .all(|sample| sample.abs() < 1e-6)
        );
        let snapshot = counters.snapshot();
        assert_eq!(snapshot.pcm_refill_limit_hits, 1);
        assert_eq!(snapshot.pcm_underflows, 0);
    }
}
