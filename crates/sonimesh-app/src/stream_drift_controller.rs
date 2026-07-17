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
