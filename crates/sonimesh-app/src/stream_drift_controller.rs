use sonimesh_audio::{DriftConfig, DriftConfigError, DriftController, DriftDecision, FrameDecoder};

use crate::{DriftAdjustmentError, DriftAdjustmentHandle, ReceiverStreamPipeline};

/// Control-domain owner that applies one stream's bounded drift decisions.
///
/// Call [`Self::update`] at a low fixed cadence outside the decoder and output
/// callback. It samples only the bounded PCM queue, publishes ordinary ppm
/// changes to the render-source handle, and retires stream-local playout state
/// when occupancy reaches a hard-resynchronization threshold.
pub struct StreamDriftController {
    controller: DriftController,
    adjustment: DriftAdjustmentHandle,
}

impl StreamDriftController {
    /// Creates a controller bound to one stream's source adjustment handle.
    ///
    /// # Errors
    ///
    /// Returns [`DriftConfigError`] when the supplied controller bounds are
    /// invalid or cannot preserve a safe occupancy range.
    pub fn new(
        config: DriftConfig,
        adjustment: DriftAdjustmentHandle,
    ) -> Result<Self, DriftConfigError> {
        Ok(Self {
            controller: DriftController::new(config)?,
            adjustment,
        })
    }

    /// Samples queue occupancy and applies the resulting stream-local action.
    ///
    /// A queue capacity is hard-bounded below `u16::MAX`; saturation preserves
    /// the safe high-side reset behavior if that invariant ever changes.
    ///
    /// # Errors
    ///
    /// Returns [`DriftAdjustmentError`] only if a decision violates the shared
    /// source adjustment bound. A valid [`DriftConfig`] cannot produce it.
    pub fn update<
        D,
        const JITTER_CAPACITY: usize,
        const MAX_ENCODED_BYTES: usize,
        const PCM_SAMPLES_PER_BLOCK: usize,
    >(
        &mut self,
        pipeline: &mut ReceiverStreamPipeline<
            D,
            JITTER_CAPACITY,
            MAX_ENCODED_BYTES,
            PCM_SAMPLES_PER_BLOCK,
        >,
    ) -> Result<DriftDecision, DriftAdjustmentError>
    where
        D: FrameDecoder,
    {
        let occupancy_blocks = u16::try_from(pipeline.queued_pcm_blocks()).unwrap_or(u16::MAX);
        let decision = self.controller.update(occupancy_blocks);
        match self.adjustment.apply_decision(decision) {
            Ok(()) => Ok(decision),
            Err(DriftAdjustmentError::HardResyncRequired(_)) => {
                pipeline.hard_resynchronize();
                Ok(decision)
            }
            Err(error) => Err(error),
        }
    }

    /// Returns the latest bounded correction requested by the underlying controller.
    #[must_use]
    pub const fn adjustment_ppm(&self) -> i32 {
        self.controller.adjustment_ppm()
    }
}
#[cfg(test)]
mod tests {
    use core::convert::Infallible;
    use std::sync::Arc;

    use sonimesh_audio::{
        DecodeStep, DriftConfig, DriftConfigError, DriftDecision, DriftResyncDirection,
        FrameDecoder, realtime_pcm_queue,
    };
    use sonimesh_diagnostics::StreamCounters;
    use sonimesh_protocol::{MediaDirection, MediaKeyContext, MediaSealer, SessionSecret};

    use super::StreamDriftController;
    use crate::{DriftAdjustmentHandle, ReceiverStreamPipeline};

    const JITTER_CAPACITY: usize = 16;
    const MAX_ENCODED_BYTES: usize = 64;
    const PCM_SAMPLES: usize = 4;

    const CONFIG: DriftConfig = DriftConfig {
        target_blocks: 4,
        low_resync_blocks: 0,
        high_resync_blocks: 8,
        proportional_ppm_per_block: 100,
        integral_ppm_per_block_tick: 10,
        max_adjustment_ppm: 500,
        max_step_ppm: 50,
        max_integral_block_ticks: 16,
    };

    struct ByteDecoder;

    impl FrameDecoder for ByteDecoder {
        type Error = Infallible;

        fn decode(&mut self, encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
            output[0] = f32::from(encoded[0]);
            Ok(1)
        }

        fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
            output[0] = 0.0;
            Ok(1)
        }
    }

    fn context() -> MediaKeyContext {
        MediaKeyContext::new(
            0x0102_0304_0506_0708,
            9,
            1,
            MediaDirection::AndroidToDesktop,
        )
        .expect("fixed test context is valid")
    }

    type Pipeline =
        ReceiverStreamPipeline<ByteDecoder, JITTER_CAPACITY, MAX_ENCODED_BYTES, PCM_SAMPLES>;

    fn pipeline(counters: Arc<StreamCounters>) -> Pipeline {
        let secret = SessionSecret::new([0xA5; 32]);
        let (producer, _) =
            realtime_pcm_queue::<PCM_SAMPLES>(8).expect("bounded test queue is valid");
        ReceiverStreamPipeline::new(&secret, context(), ByteDecoder, producer, counters, 0)
            .expect("fixed test pipeline is valid")
    }

    fn feed_frames(pipeline: &mut Pipeline, frames: u32) {
        let mut sealer = MediaSealer::new(&SessionSecret::new([0xA5; 32]), context())
            .expect("fixed test sealer is valid");
        let mut plaintext = [0u8; MAX_ENCODED_BYTES];
        for sequence in 0..frames {
            let packet = sealer
                .seal(0, u64::from(sequence) * 960, &[7])
                .expect("fixed test packet seals");
            pipeline
                .receive(&packet.datagram, &mut plaintext)
                .expect("authenticated test frame is admitted");
            assert!(matches!(
                pipeline.decode_next().expect("test frame decodes"),
                DecodeStep::Decoded { .. }
            ));
        }
    }

    fn controller(counters: &Arc<StreamCounters>) -> StreamDriftController {
        StreamDriftController::new(
            CONFIG,
            DriftAdjustmentHandle::new(Some(Arc::clone(counters))),
        )
        .expect("fixed test config is valid")
    }

    #[test]
    fn new_rejects_invalid_controller_bounds() {
        let mut config = CONFIG;
        config.low_resync_blocks = config.target_blocks;

        assert!(matches!(
            StreamDriftController::new(config, DriftAdjustmentHandle::new(None)),
            Err(DriftConfigError::InvalidOccupancyThresholds)
        ));
    }

    #[test]
    fn update_publishes_a_positive_adjustment_to_the_shared_handle() {
        let counters = Arc::new(StreamCounters::default());
        let mut pipeline = pipeline(Arc::clone(&counters));
        let mut controller = controller(&counters);

        feed_frames(&mut pipeline, 6);
        assert_eq!(pipeline.queued_pcm_blocks(), 6);

        assert_eq!(
            controller.update(&mut pipeline).unwrap(),
            DriftDecision::Adjust { ppm: 50 }
        );
        assert_eq!(controller.adjustment_ppm(), 50);
        assert_eq!(counters.snapshot().drift_adjustment_ppm, 50);
        assert_eq!(pipeline.queued_pcm_blocks(), 6);
    }

    #[test]
    fn update_publishes_a_negative_adjustment_below_target() {
        let counters = Arc::new(StreamCounters::default());
        let mut pipeline = pipeline(Arc::clone(&counters));
        let mut controller = controller(&counters);

        feed_frames(&mut pipeline, 2);
        assert_eq!(
            controller.update(&mut pipeline).unwrap(),
            DriftDecision::Adjust { ppm: -50 }
        );
        assert_eq!(controller.adjustment_ppm(), -50);
        assert_eq!(counters.snapshot().drift_adjustment_ppm, -50);
    }

    #[test]
    fn update_high_side_hard_resync_retires_pipeline_state() {
        let counters = Arc::new(StreamCounters::default());
        let mut pipeline = pipeline(Arc::clone(&counters));
        let mut controller = controller(&counters);

        feed_frames(&mut pipeline, 8);
        assert_eq!(pipeline.queued_pcm_blocks(), 8);

        assert_eq!(
            controller.update(&mut pipeline).unwrap(),
            DriftDecision::HardResync {
                direction: DriftResyncDirection::High,
            }
        );
        assert_eq!(controller.adjustment_ppm(), 0);
        assert_eq!(pipeline.queued_pcm_blocks(), 0);
        let snapshot = counters.snapshot();
        assert_eq!(snapshot.stream_resets, 1);
        assert_eq!(snapshot.drift_hard_resyncs, 1);
        assert_eq!(snapshot.drift_adjustment_ppm, 0);
    }

    #[test]
    fn update_low_side_hard_resync_retires_an_empty_queue() {
        let counters = Arc::new(StreamCounters::default());
        let mut pipeline = pipeline(Arc::clone(&counters));
        let mut controller = controller(&counters);

        assert_eq!(pipeline.queued_pcm_blocks(), 0);
        assert_eq!(
            controller.update(&mut pipeline).unwrap(),
            DriftDecision::HardResync {
                direction: DriftResyncDirection::Low,
            }
        );
        assert_eq!(controller.adjustment_ppm(), 0);
        assert_eq!(pipeline.queued_pcm_blocks(), 0);
        assert_eq!(counters.snapshot().stream_resets, 1);
    }
}
