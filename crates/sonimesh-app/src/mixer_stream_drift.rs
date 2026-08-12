//! Control-domain per-stream drift for the mixer session path.
//!
//! `MixerStreamDriftController` mirrors [`crate::StreamDriftController`] for
//! the Phase 2 mixer path: it samples only the bounded adapter source queue
//! (the queue the decode-domain `StreamFormatAdapter` reads), publishes
//! ordinary ppm changes to that stream's adapter resampler, and retires the
//! stream's partial conversion state when occupancy reaches an emergency
//! threshold. The mixer callback is never touched; the controller is driven at
//! a low fixed cadence by the same control loop that decodes and pumps. Call
//! [`Self::reset`] after [`MixerStreamSession::recover_stream`] installs a
//! fresh nominal adapter so a stale pre-recovery correction is not re-applied.

use sonimesh_audio::{DriftConfig, DriftConfigError, DriftController, DriftDecision};

use crate::{MediaStreamIdentity, MixerStreamSession, MixerStreamSessionError};

/// Control-domain owner that applies one stream's bounded drift decisions.
pub struct MixerStreamDriftController {
    identity: MediaStreamIdentity,
    controller: DriftController,
}

impl MixerStreamDriftController {
    /// Creates a controller bound to one admitted mixer stream.
    ///
    /// # Errors
    ///
    /// Returns [`DriftConfigError`] when the supplied controller bounds are
    /// invalid or cannot preserve a safe occupancy range.
    pub fn new(
        identity: MediaStreamIdentity,
        config: DriftConfig,
    ) -> Result<Self, DriftConfigError> {
        Ok(Self {
            identity,
            controller: DriftController::new(config)?,
        })
    }

    /// Samples the stream's adapter source occupancy and applies the decision.
    ///
    /// Ordinary adjustments publish to the stream's format adapter through the
    /// session; an emergency threshold retires the adapter's partial
    /// conversion state so stale samples cannot cross the resync boundary.
    ///
    /// # Errors
    ///
    /// Returns [`MixerStreamSessionError::UnknownStream`] when the stream was
    /// removed, or a session error when an adjustment or reset cannot be
    /// applied. A valid [`DriftConfig`] cannot produce an out-of-bound
    /// adjustment.
    pub fn update<O, const SLOTS: usize, const OUTPUT_SAMPLES_PER_BLOCK: usize>(
        &mut self,
        session: &mut MixerStreamSession<O, SLOTS, OUTPUT_SAMPLES_PER_BLOCK>,
    ) -> Result<DriftDecision, MixerStreamSessionError>
    where
        O: sonimesh_platform::AudioOutput,
    {
        let occupancy_blocks =
            u16::try_from(session.queued_source_blocks(self.identity)?).unwrap_or(u16::MAX);
        let decision = self.controller.update(occupancy_blocks);
        match decision {
            DriftDecision::Adjust { ppm } => {
                session.set_rate_adjustment_ppm(self.identity, ppm)?;
            }
            DriftDecision::HardResync { .. } => {
                session.begin_epoch(self.identity)?;
            }
        }
        Ok(decision)
    }

    /// Returns the latest bounded correction requested by the controller.
    #[must_use]
    pub const fn adjustment_ppm(&self) -> i32 {
        self.controller.adjustment_ppm()
    }

    /// Clears controller history after an external stream reset.
    ///
    /// Call this after [`MixerStreamSession::recover_stream`] installs a fresh
    /// nominal adapter so the stale pre-recovery correction and integral
    /// history are not re-applied by the next update.
    pub fn reset(&mut self) {
        self.controller.reset();
    }

    /// Returns the stream identity this controller addresses.
    #[must_use]
    pub const fn identity(&self) -> MediaStreamIdentity {
        self.identity
    }
}
