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
    MAX_PCM_QUEUE_BLOCKS, MixerMembershipHandle, RealtimePcmConsumer, RealtimePcmProducer,
    RealtimeQueueConfigError, realtime_pcm_queue,
};
use sonimesh_diagnostics::StreamCounters;

use crate::{GainControlHandle, SourcePcmFormat, StreamFormatAdapter, StreamFormatAdapterError};

/// One admitted stream: the slot it occupies after the next render and the
/// producer the stream pipeline feeds.
pub struct AdmittedStream<const PCM_SAMPLES_PER_BLOCK: usize> {
    /// Fixed slot the stream occupies after pending commands are applied.
    pub slot: usize,
    /// Decoder-side producer paired with the consumer installed in the slot.
    pub producer: RealtimePcmProducer<PCM_SAMPLES_PER_BLOCK>,
}

/// One admitted stream with per-stream format adaptation before the mixer
/// queue.
pub struct AdmittedAdaptedStream<
    const SOURCE_SAMPLES_PER_BLOCK: usize,
    const OUTPUT_SAMPLES_PER_BLOCK: usize,
> {
    /// Fixed slot the stream occupies after pending commands are applied.
    pub slot: usize,
    /// Source-format producer installed into the stream's decode pipeline.
    pub source_producer: RealtimePcmProducer<SOURCE_SAMPLES_PER_BLOCK>,
    /// Decode-domain adapter converting source blocks to output-format blocks.
    pub adapter: StreamFormatAdapter<SOURCE_SAMPLES_PER_BLOCK, OUTPUT_SAMPLES_PER_BLOCK>,
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
    /// Per-stream format adaptation could not be constructed.
    Adapter(StreamFormatAdapterError),
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
            Self::Adapter(error) => write!(formatter, "mixer stream adaptation failed: {error}"),
        }
    }
}

impl std::error::Error for MixerAdmissionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Queue(error) => Some(error),
            Self::Adapter(error) => Some(error),
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
    /// This fast path is for streams that already produce output-format
    /// interleaved PCM in the mixer's block size: `source_format` must equal
    /// the controller's output format. Streams in any other source format use
    /// [`Self::admit_adapted_stream`]. On success the returned producer is
    /// installed into the stream's decode pipeline, and the stream occupies
    /// the returned slot after the next render.
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
        let (slot, producer, consumer) = self.reserve_slot()?;
        self.membership
            .insert(consumer, gain, counters)
            .map_err(|_| MixerAdmissionError::CommandQueueFull)?;
        self.active[slot] = true;
        Ok(AdmittedStream { slot, producer })
    }

    /// Publishes one stream activation with per-stream format adaptation.
    ///
    /// This is the general admission path: the stream's decoded source-format
    /// blocks are converted to the mixer's output format by a
    /// [`StreamFormatAdapter`] owned by the decode/control domain before they
    /// enter the slot queue. On success the returned source producer is
    /// installed into the stream's decode pipeline, the adapter is pumped
    /// alongside decode, and the stream occupies the returned slot after the
    /// next render. The slot queue is only published after adapter
    /// construction succeeds, so an invalid source format never reserves a
    /// slot.
    ///
    /// # Errors
    ///
    /// Returns an error before publishing when the source queue capacity is
    /// outside the bounded range, the table is full, or format adaptation
    /// cannot be constructed, or a retryable error when the bounded command
    /// queue is full.
    pub fn admit_adapted_stream<const SOURCE_SAMPLES_PER_BLOCK: usize>(
        &mut self,
        source_format: SourcePcmFormat,
        source_queue_capacity: usize,
        gain: GainControlHandle,
        counters: Option<Arc<StreamCounters>>,
    ) -> Result<
        AdmittedAdaptedStream<SOURCE_SAMPLES_PER_BLOCK, PCM_SAMPLES_PER_BLOCK>,
        MixerAdmissionError,
    > {
        if source_queue_capacity == 0 {
            return Err(MixerAdmissionError::Queue(
                RealtimeQueueConfigError::ZeroCapacity,
            ));
        }
        if source_queue_capacity > MAX_PCM_QUEUE_BLOCKS {
            return Err(MixerAdmissionError::Queue(
                RealtimeQueueConfigError::CapacityTooLarge,
            ));
        }
        let (source_producer, source_consumer) =
            realtime_pcm_queue::<SOURCE_SAMPLES_PER_BLOCK>(source_queue_capacity)
                .map_err(MixerAdmissionError::Queue)?;
        let (slot, slot_producer, slot_consumer) = self.reserve_slot()?;
        let adapter = StreamFormatAdapter::new(
            source_consumer,
            slot_producer,
            source_format,
            self.output_format,
        )
        .map_err(MixerAdmissionError::Adapter)?;
        self.membership
            .insert(slot_consumer, gain, counters)
            .map_err(|_| MixerAdmissionError::CommandQueueFull)?;
        self.active[slot] = true;
        Ok(AdmittedAdaptedStream {
            slot,
            source_producer,
            adapter,
        })
    }

    /// Reserves the lowest free slot and its bounded output-format queue pair.
    ///
    /// The returned consumer is installed by the caller after any per-stream
    /// adapter has been constructed, so admission fails closed without
    /// publishing a command when a later construction step fails.
    fn reserve_slot(
        &mut self,
    ) -> Result<
        (
            usize,
            RealtimePcmProducer<PCM_SAMPLES_PER_BLOCK>,
            RealtimePcmConsumer<PCM_SAMPLES_PER_BLOCK>,
        ),
        MixerAdmissionError,
    > {
        let slot = (0..SLOTS)
            .find(|&index| !self.active[index])
            .ok_or(MixerAdmissionError::SlotTableFull)?;
        let (producer, consumer) = realtime_pcm_queue::<PCM_SAMPLES_PER_BLOCK>(self.queue_capacity)
            .map_err(MixerAdmissionError::Queue)?;
        Ok((slot, producer, consumer))
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

#[cfg(test)]
mod tests {
    use sonimesh_audio::{
        CommandDrivenMixer, GainControlHandle, MAX_PCM_QUEUE_BLOCKS, RealtimeQueueConfigError,
    };

    use super::{MixerAdmissionError, MixerMembershipController, SourcePcmFormat};

    const SLOTS: usize = 2;
    const SAMPLES: usize = 256;

    fn output_format() -> SourcePcmFormat {
        SourcePcmFormat::new(48_000, 2).expect("fixed test output format is valid")
    }

    fn mixer() -> CommandDrivenMixer<SLOTS, SAMPLES> {
        CommandDrivenMixer::new().expect("fixed test mixer configuration is valid")
    }

    fn controller(
        membership: sonimesh_audio::MixerMembershipHandle<SAMPLES>,
    ) -> MixerMembershipController<SLOTS, SAMPLES> {
        MixerMembershipController::new(membership, 8, output_format())
            .expect("fixed test controller configuration is valid")
    }

    #[test]
    fn new_rejects_zero_and_oversized_queue_capacity() {
        let mixer = mixer();
        let membership = mixer.membership();
        assert!(matches!(
            MixerMembershipController::<SLOTS, SAMPLES>::new(
                membership.clone(),
                0,
                output_format()
            ),
            Err(MixerAdmissionError::Queue(
                RealtimeQueueConfigError::ZeroCapacity
            ))
        ));
        assert!(matches!(
            MixerMembershipController::<SLOTS, SAMPLES>::new(
                membership,
                MAX_PCM_QUEUE_BLOCKS + 1,
                output_format()
            ),
            Err(MixerAdmissionError::Queue(
                RealtimeQueueConfigError::CapacityTooLarge
            ))
        ));
    }

    #[test]
    fn admit_stream_rejects_a_format_mismatch() {
        let mixer = mixer();
        let mut controller = controller(mixer.membership());
        let mismatched = SourcePcmFormat::new(44_100, 2).expect("fixed mismatch format is valid");

        assert!(matches!(
            controller.admit_stream(mismatched, GainControlHandle::new(), None),
            Err(MixerAdmissionError::FormatMismatch)
        ));
        assert_eq!(controller.active(), 0);
        assert_eq!(controller.pending_commands(), 0);
    }

    #[test]
    fn admit_stream_occupies_the_lowest_free_slot() {
        let mut mixer = mixer();
        let mut controller = controller(mixer.membership());

        let first = controller
            .admit_stream(output_format(), GainControlHandle::new(), None)
            .unwrap();
        assert_eq!(first.slot, 0);
        assert_eq!(controller.active(), 1);
        assert!(controller.is_active(0));
        assert!(!controller.is_active(1));
        assert_eq!(controller.pending_commands(), 1);

        let second = controller
            .admit_stream(output_format(), GainControlHandle::new(), None)
            .unwrap();
        assert_eq!(second.slot, 1);
        assert_eq!(controller.active(), 2);
        assert_eq!(controller.pending_commands(), 2);

        // The callback applies both queued commands when it drains.
        mixer.drain_pending();
        assert_eq!(controller.pending_commands(), 0);
    }

    #[test]
    fn admit_stream_fails_closed_when_the_slot_table_is_full() {
        let mixer = mixer();
        let mut controller = controller(mixer.membership());
        controller
            .admit_stream(output_format(), GainControlHandle::new(), None)
            .unwrap();
        controller
            .admit_stream(output_format(), GainControlHandle::new(), None)
            .unwrap();

        assert!(matches!(
            controller.admit_stream(output_format(), GainControlHandle::new(), None),
            Err(MixerAdmissionError::SlotTableFull)
        ));
        assert_eq!(controller.active(), 2);
        assert_eq!(controller.pending_commands(), 2);
    }

    #[test]
    fn remove_stream_rejects_an_out_of_range_slot() {
        let mixer = mixer();
        let mut controller = controller(mixer.membership());

        assert_eq!(
            controller.remove_stream(SLOTS),
            Err(MixerAdmissionError::SlotOutOfRange)
        );
    }

    #[test]
    fn remove_stream_releases_the_slot_for_reuse() {
        let mixer = mixer();
        let mut controller = controller(mixer.membership());
        controller
            .admit_stream(output_format(), GainControlHandle::new(), None)
            .unwrap();
        controller
            .admit_stream(output_format(), GainControlHandle::new(), None)
            .unwrap();

        controller.remove_stream(0).unwrap();
        assert!(!controller.is_active(0));
        assert_eq!(controller.active(), 1);

        // The next admission reuses the lowest free slot.
        let readmitted = controller
            .admit_stream(output_format(), GainControlHandle::new(), None)
            .unwrap();
        assert_eq!(readmitted.slot, 0);
        assert_eq!(controller.active(), 2);
    }

    #[test]
    fn remove_stream_tolerates_a_stale_inactive_slot() {
        let mixer = mixer();
        let mut controller = controller(mixer.membership());

        controller.remove_stream(1).unwrap();
        assert!(!controller.is_active(1));
        assert_eq!(controller.active(), 0);
    }

    #[test]
    fn admit_adapted_stream_rejects_invalid_source_queue_capacity() {
        let mixer = mixer();
        let mut controller = controller(mixer.membership());

        assert!(matches!(
            controller.admit_adapted_stream::<SAMPLES>(
                output_format(),
                0,
                GainControlHandle::new(),
                None
            ),
            Err(MixerAdmissionError::Queue(
                RealtimeQueueConfigError::ZeroCapacity
            ))
        ));
        assert!(matches!(
            controller.admit_adapted_stream::<SAMPLES>(
                output_format(),
                MAX_PCM_QUEUE_BLOCKS + 1,
                GainControlHandle::new(),
                None
            ),
            Err(MixerAdmissionError::Queue(
                RealtimeQueueConfigError::CapacityTooLarge
            ))
        ));
        assert_eq!(controller.active(), 0);
        assert_eq!(controller.pending_commands(), 0);
    }

    #[test]
    fn admit_adapted_stream_installs_an_adapter_at_the_lowest_free_slot() {
        let mixer = mixer();
        let mut controller = controller(mixer.membership());
        let source_format = SourcePcmFormat::new(48_000, 1).expect("mono source format is valid");

        let admitted = controller
            .admit_adapted_stream::<128>(source_format, 4, GainControlHandle::new(), None)
            .unwrap();
        assert_eq!(admitted.slot, 0);
        assert_eq!(controller.active(), 1);
        assert_eq!(controller.pending_commands(), 1);
        assert_eq!(admitted.adapter.source_channels(), 1);
        assert_eq!(admitted.adapter.output_channels(), 2);
    }

    #[test]
    fn admit_adapted_stream_fails_closed_when_adaptation_cannot_be_constructed() {
        let mixer = mixer();
        let mut controller = controller(mixer.membership());
        let invalid = SourcePcmFormat {
            sample_rate_hz: 48_000,
            channels: 0,
        };

        assert!(matches!(
            controller.admit_adapted_stream::<128>(invalid, 4, GainControlHandle::new(), None),
            Err(MixerAdmissionError::Adapter(_))
        ));
        assert_eq!(controller.active(), 0);
        assert_eq!(controller.pending_commands(), 0);
    }

    #[test]
    fn error_display_is_actionable() {
        assert_eq!(
            MixerAdmissionError::SlotTableFull.to_string(),
            "mixer slot table is full"
        );
        assert_eq!(
            MixerAdmissionError::CommandQueueFull.to_string(),
            "mixer membership command queue is full"
        );
        assert_eq!(
            MixerAdmissionError::SlotOutOfRange.to_string(),
            "mixer slot index is out of range"
        );
    }
}
