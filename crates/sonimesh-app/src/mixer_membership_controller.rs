//! Control-domain membership controller for the Phase 2 fixed-slot mixer.
//!
//! ADR 0015 puts membership transitions in the control domain: activating or
//! deactivating a slot publishes a bounded command that the callback applies at
//! the start of its next render. This controller is the app-side admission
//! boundary. It owns a [`MixerMembershipHandle`], keeps a deterministic
//! projection of the callback's fixed slot table (mirroring the
//! lowest-free-slot FIFO policy the callback applies), and fails closed when
//! the table or the command queue is full.

use core::fmt;
use std::sync::Arc;

use sonimesh_audio::{
    MAX_PCM_QUEUE_BLOCKS, MixerMembershipHandle, RealtimePcmProducer, RealtimeQueueConfigError,
    realtime_pcm_queue,
};
use sonimesh_diagnostics::StreamCounters;

use crate::{GainControlHandle, SourcePcmFormat};

/// One admitted stream: the slot it occupies after the next render and the
/// producer the stream pipeline feeds.
pub struct AdmittedStream<const PCM_SAMPLES_PER_BLOCK: usize> {
    /// Fixed slot the stream occupies after pending commands are applied.
    pub slot: usize,
    /// Decoder-side producer paired with the consumer installed in the slot.
    pub producer: RealtimePcmProducer<PCM_SAMPLES_PER_BLOCK>,
}

/// Failure while admitting or removing a stream at the mixer boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MixerAdmissionError {
    /// The stream's source format differs from the mixer's output format.
    FormatMismatch,
    /// Every slot is occupied; admission fails closed in the control domain.
    SlotTableFull,
    /// The bounded command queue is full; retry after the next render.
    CommandQueueFull,
    /// The slot index is outside the fixed slot table.
    SlotOutOfRange,
    /// The bounded PCM queue configuration was rejected.
    Queue(RealtimeQueueConfigError),
}

impl fmt::Display for MixerAdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FormatMismatch => {
                formatter.write_str("stream format differs from the mixer output format")
            }
            Self::SlotTableFull => formatter.write_str("mixer slot table is full"),
            Self::CommandQueueFull => formatter.write_str("mixer membership command queue is full"),
            Self::SlotOutOfRange => formatter.write_str("mixer slot index is out of range"),
            Self::Queue(error) => write!(formatter, "mixer PCM queue setup failed: {error}"),
        }
    }
}

impl std::error::Error for MixerAdmissionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Queue(error) => Some(error),
            Self::FormatMismatch
            | Self::SlotTableFull
            | Self::CommandQueueFull
            | Self::SlotOutOfRange => None,
        }
    }
}

/// Control-domain endpoint publishing membership changes for one mixer.
///
/// The controller and its [`MixerRenderSource`] must be created from the same
/// mixer so the command queue is shared. The controller's active-slot
/// projection is authoritative because it is the sole publisher and mirrors the
/// callback's lowest-free-slot FIFO application.
pub struct MixerMembershipController<const SLOTS: usize, const PCM_SAMPLES_PER_BLOCK: usize> {
    membership: MixerMembershipHandle<PCM_SAMPLES_PER_BLOCK>,
    queue_capacity: usize,
    output_format: SourcePcmFormat,
    active: [bool; SLOTS],
}

impl<const SLOTS: usize, const PCM_SAMPLES_PER_BLOCK: usize>
    MixerMembershipController<SLOTS, PCM_SAMPLES_PER_BLOCK>
{
    /// Creates a controller sharing `membership`'s mixer and accepting the
    /// given output-format blocks and queue capacity.
    ///
    /// # Errors
    ///
    /// Returns a queue error when `queue_capacity` is outside the bounded PCM
    /// queue range.
    pub fn new(
        membership: MixerMembershipHandle<PCM_SAMPLES_PER_BLOCK>,
        queue_capacity: usize,
        output_format: SourcePcmFormat,
    ) -> Result<Self, MixerAdmissionError> {
        if queue_capacity == 0 {
            return Err(MixerAdmissionError::Queue(
                RealtimeQueueConfigError::ZeroCapacity,
            ));
        }
        if queue_capacity > MAX_PCM_QUEUE_BLOCKS {
            return Err(MixerAdmissionError::Queue(
                RealtimeQueueConfigError::CapacityTooLarge,
            ));
        }
        Ok(Self {
            membership,
            queue_capacity,
            output_format,
            active: [false; SLOTS],
        })
    }

    /// Publishes one stream activation at the lowest free slot.
    ///
    /// The Phase 2 mixer is a plain summing bus: each slot's queue must already
    /// hold output-format interleaved PCM, so `source_format` must equal the
    /// controller's output format. On success the returned producer is
    /// installed into the stream's decode pipeline, and the stream occupies the
    /// returned slot after the next render.
    ///
    /// # Errors
    ///
    /// Returns an error before publishing when the source format differs or
    /// the table is full, or a retryable error when the bounded command queue
    /// is full.
    pub fn admit_stream(
        &mut self,
        source_format: SourcePcmFormat,
        gain: GainControlHandle,
        counters: Option<Arc<StreamCounters>>,
    ) -> Result<AdmittedStream<PCM_SAMPLES_PER_BLOCK>, MixerAdmissionError> {
        if source_format != self.output_format {
            return Err(MixerAdmissionError::FormatMismatch);
        }
        let slot = (0..SLOTS)
            .find(|&index| !self.active[index])
            .ok_or(MixerAdmissionError::SlotTableFull)?;
        let (producer, consumer) = realtime_pcm_queue::<PCM_SAMPLES_PER_BLOCK>(self.queue_capacity)
            .map_err(MixerAdmissionError::Queue)?;
        self.membership
            .insert(consumer, gain, counters)
            .map_err(|_| MixerAdmissionError::CommandQueueFull)?;
        self.active[slot] = true;
        Ok(AdmittedStream { slot, producer })
    }

    /// Publishes one stream deactivation for `slot`.
    ///
    /// A stale remove for an already-inactive slot is tolerated, matching the
    /// callback. The control domain drops the stream's producer after this
    /// call; the callback drops the slot's consumer on its next render.
    ///
    /// # Errors
    ///
    /// Returns an error for an out-of-range slot, or a retryable error when
    /// the bounded command queue is full.
    pub fn remove_stream(&mut self, slot: usize) -> Result<(), MixerAdmissionError> {
        if slot >= SLOTS {
            return Err(MixerAdmissionError::SlotOutOfRange);
        }
        self.membership
            .remove(slot)
            .map_err(|_| MixerAdmissionError::CommandQueueFull)?;
        self.active[slot] = false;
        Ok(())
    }

    /// Number of membership commands waiting to be applied by the callback.
    #[must_use]
    pub fn pending_commands(&self) -> usize {
        self.membership.pending_commands()
    }

    /// Number of slots the controller projects as active after pending drains.
    #[must_use]
    pub fn active(&self) -> usize {
        self.active.iter().filter(|present| **present).count()
    }

    /// Returns whether the controller projects `slot` as active.
    #[must_use]
    pub fn is_active(&self, slot: usize) -> bool {
        matches!(self.active.get(slot), Some(true))
    }
}
