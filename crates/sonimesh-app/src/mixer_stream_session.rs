//! Control-domain stream session driving adapted mixer admission.
//!
//! `MixerStreamSession` composes the output-lifetime owner
//! ([`MixerOutputController`]) with the membership boundary
//! ([`MixerMembershipController`]) and per-stream adapter pumps. The control
//! domain starts the platform output once, admits decoded streams through the
//! general adapted path (`admit_adapted_stream`), installs each stream's
//! source producer into its caller-owned pipeline, and pumps the per-stream
//! `StreamFormatAdapter` after every decode step.
//!
//! Stream records survive output-device recreation: when the backend faults,
//! [`Self::monitor`] retires the membership boundary but keeps every stream
//! record, and [`Self::recover_output`] recreates the source with a fresh
//! empty slot table and returns the identities the caller must re-admit
//! through [`Self::recover_stream`] so each pipeline receives a fresh
//! producer paired with the new mixer. A failure in one stream's adapter pump
//! is reported without blocking the other streams' pumps.
//! Per-stream key rotation ([`Self::rotate_stream`]) replaces a stream's
//! identity in place: the slot, gain handle, and diagnostics counters stay,
//! partial conversion state is retired, and the audio configuration (frame
//! budget and source format) must be unchanged because the decode pipeline
//! and adapter are compiled for those parameters.

use core::fmt;
use std::sync::Arc;

use sonimesh_audio::{MAX_PCM_QUEUE_BLOCKS, RealtimeQueueConfigError, ResamplerError};
use sonimesh_diagnostics::{StreamCounters, StreamMetricsSnapshot};
use sonimesh_platform::{AudioOutput, OutputHealth};

use crate::{
    GainControlHandle, MediaStreamIdentity, MixerAdmissionError, MixerMembershipController,
    MixerOutputController, MixerOutputLifecycleError, OutputSelection, OutputSnapshot, PumpOutcome,
    ReceiverPcmProducerInstaller, SourcePcmFormat, SourcePcmFormatError, StreamFormatAdapter,
    StreamFormatAdapterError, StreamStartRequest,
};

/// Decode-domain pump boundary for one admitted mixer stream.
///
/// The source block size is a compile-time constant per stream, so the session
/// stores each stream's adapter behind this erase-allocation boundary to keep
/// one homogeneous record table across streams with different source formats.
pub(crate) trait AdaptedStreamPump {
    /// Drains decoded source blocks and pushes one output block to the slot queue.
    fn pump(&mut self) -> Result<PumpOutcome, StreamFormatAdapterError>;
    /// Retires partial conversion state and queued output blocks.
    fn begin_epoch(&mut self);
    /// Applies one bounded drift correction.
    fn set_rate_adjustment_ppm(&mut self, ppm: i32) -> Result<(), ResamplerError>;
    /// Approximate source-format blocks waiting for conversion.
    fn queued_source_blocks(&self) -> usize;
    /// Approximate output-format blocks waiting for the mixer.
    fn queued_output_blocks(&self) -> usize;
}

impl<const SOURCE_SAMPLES_PER_BLOCK: usize, const OUTPUT_SAMPLES_PER_BLOCK: usize> AdaptedStreamPump
    for StreamFormatAdapter<SOURCE_SAMPLES_PER_BLOCK, OUTPUT_SAMPLES_PER_BLOCK>
{
    fn pump(&mut self) -> Result<PumpOutcome, StreamFormatAdapterError> {
        StreamFormatAdapter::pump(self)
    }

    fn begin_epoch(&mut self) {
        StreamFormatAdapter::begin_epoch(self);
    }

    fn set_rate_adjustment_ppm(&mut self, ppm: i32) -> Result<(), ResamplerError> {
        StreamFormatAdapter::set_rate_adjustment_ppm(self, ppm)
    }

    fn queued_source_blocks(&self) -> usize {
        StreamFormatAdapter::queued_source_blocks(self)
    }

    fn queued_output_blocks(&self) -> usize {
        StreamFormatAdapter::queued_output_blocks(self)
    }
}

/// One admitted mixer stream owned across output lifetimes.
struct StreamRecord {
    identity: MediaStreamIdentity,
    /// Slot occupied in the current output lifetime, if this persisted record
    /// has already rejoined its fresh mixer membership. Record-table position is
    /// intentionally independent so recovery order cannot overwrite another
    /// persisted stream.
    membership_slot: Option<usize>,
    source_format: SourcePcmFormat,
    source_queue_capacity: usize,
    /// Frames per source block the pump was compiled and admitted for.
    samples_per_block: usize,
    gain: GainControlHandle,
    counters: Option<Arc<StreamCounters>>,
    pump: Box<dyn AdaptedStreamPump>,
}

/// A stream admitted into the current mixer lifetime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StartedMixerStream {
    /// Fixed slot the stream occupies after pending commands are applied.
    pub slot: usize,
}

/// Control-domain session owning one mixer output and its admitted streams.
pub struct MixerStreamSession<O, const SLOTS: usize, const OUTPUT_SAMPLES_PER_BLOCK: usize> {
    output: MixerOutputController<O, SLOTS, OUTPUT_SAMPLES_PER_BLOCK>,
    queue_capacity: usize,
    membership: Option<MixerMembershipController<SLOTS, OUTPUT_SAMPLES_PER_BLOCK>>,
    records: [Option<StreamRecord>; SLOTS],
}

impl<O, const SLOTS: usize, const OUTPUT_SAMPLES_PER_BLOCK: usize>
    MixerStreamSession<O, SLOTS, OUTPUT_SAMPLES_PER_BLOCK>
where
    O: AudioOutput,
{
    /// Creates a stopped session with a bounded mixer slot queue capacity.
    ///
    /// # Errors
    ///
    /// Returns a queue error when `queue_capacity` is outside the bounded PCM
    /// queue range.
    pub fn new(output: O, queue_capacity: usize) -> Result<Self, MixerStreamSessionError> {
        if queue_capacity == 0 {
            return Err(MixerStreamSessionError::Membership(
                MixerAdmissionError::Queue(RealtimeQueueConfigError::ZeroCapacity),
            ));
        }
        if queue_capacity > MAX_PCM_QUEUE_BLOCKS {
            return Err(MixerStreamSessionError::Membership(
                MixerAdmissionError::Queue(RealtimeQueueConfigError::CapacityTooLarge),
            ));
        }
        Ok(Self {
            output: MixerOutputController::new(output),
            queue_capacity,
            membership: None,
            records: [const { None }; SLOTS],
        })
    }

    /// Starts the platform output and installs a fresh membership boundary.
    ///
    /// # Errors
    ///
    /// Returns [`MixerStreamSessionError::Output`] when the output is already
    /// active or source or backend creation fails.
    pub fn start_output(
        &mut self,
        selection: OutputSelection,
    ) -> Result<(), MixerStreamSessionError> {
        self.output
            .start(selection)
            .map_err(MixerStreamSessionError::Output)?;
        let membership = self
            .output
            .membership()
            .ok_or(MixerStreamSessionError::OutputNotActive)?;
        let output_format = self
            .output
            .output_format()
            .ok_or(MixerStreamSessionError::OutputNotActive)?;
        self.membership = Some(
            MixerMembershipController::new(membership, self.queue_capacity, output_format)
                .map_err(MixerStreamSessionError::Membership)?,
        );
        Ok(())
    }

    /// Admits one decoded stream through per-stream format adaptation.
    ///
    /// The stream's source block size `S` must equal the negotiated
    /// frame-duration and sample-rate budget. On success the returned source
    /// producer is installed into `pipeline`, the adapter pump is owned by the
    /// session and advanced by [`Self::pump_all`], and the stream occupies the
    /// returned slot after the next render.
    ///
    /// # Errors
    ///
    /// Returns an error when the output is not active, the identity is already
    /// admitted, the source block size or source format is invalid, or the
    /// mixer admission boundary fails closed.
    pub fn start_stream<P, const S: usize>(
        &mut self,
        request: StreamStartRequest,
        source_queue_capacity: usize,
        counters: Option<Arc<StreamCounters>>,
        pipeline: &mut P,
    ) -> Result<StartedMixerStream, MixerStreamSessionError>
    where
        P: ReceiverPcmProducerInstaller<S>,
    {
        let expected = frames_per_block(request)?;
        if expected != S {
            return Err(MixerStreamSessionError::SourceBlockMismatch { expected });
        }
        let source_format = source_format_of(request)?;
        if self.slot_for(request.identity).is_some() {
            return Err(MixerStreamSessionError::DuplicateStream(request.identity));
        }
        let membership = self
            .membership
            .as_mut()
            .ok_or(MixerStreamSessionError::OutputNotActive)?;
        let gain = GainControlHandle::new();
        let admitted = membership
            .admit_adapted_stream::<S>(
                source_format,
                source_queue_capacity,
                gain.clone(),
                counters.clone(),
            )
            .map_err(MixerStreamSessionError::Membership)?;
        let slot = admitted.slot;
        pipeline.replace_pcm_producer(admitted.source_producer);
        self.records[slot] = Some(StreamRecord {
            identity: request.identity,
            membership_slot: Some(slot),
            source_format,
            source_queue_capacity,
            samples_per_block: expected,
            gain: gain.clone(),
            counters,
            pump: Box::new(admitted.adapter),
        });
        let _ = gain;
        Ok(StartedMixerStream { slot })
    }

    /// Returns whether removing `identity` would publish a realtime membership
    /// command for the current output lifetime.
    ///
    /// During output recovery the stream record persists while the membership
    /// boundary is retired, so removing it only updates control-domain state and
    /// does not consume command-queue capacity.
    #[must_use]
    pub fn removal_requires_membership_command(&self, identity: MediaStreamIdentity) -> bool {
        self.membership.is_some()
            && self
                .slot_for(identity)
                .and_then(|record_slot| self.records[record_slot].as_ref())
                .is_some_and(|record| record.membership_slot.is_some())
    }

    /// Removes one admitted stream and retires its slot.
    ///
    /// When the membership boundary is retired (output faulted or stopped) the
    /// record is still dropped so the control domain can stop the stream
    /// during an output-unavailable period.
    ///
    /// # Errors
    ///
    /// Returns [`MixerStreamSessionError::UnknownStream`] when no admitted
    /// stream carries `identity`, or a retryable membership error.
    pub fn remove_stream(
        &mut self,
        identity: MediaStreamIdentity,
    ) -> Result<(), MixerStreamSessionError> {
        let record_slot = self
            .slot_for(identity)
            .ok_or(MixerStreamSessionError::UnknownStream(identity))?;
        let membership_slot = self.records[record_slot]
            .as_ref()
            .and_then(|record| record.membership_slot);
        if let (Some(membership), Some(membership_slot)) =
            (self.membership.as_mut(), membership_slot)
        {
            membership
                .remove_stream(membership_slot)
                .map_err(MixerStreamSessionError::Membership)?;
        }
        self.records[record_slot] = None;
        Ok(())
    }

    /// Advances every admitted stream's adapter pump by one step.
    ///
    /// All streams are pumped even when one pump fails, so a stream-local
    /// conversion failure cannot starve other streams; the first observed
    /// error is returned after the pass.
    ///
    /// # Errors
    ///
    /// Returns the first adapter pump error observed during the pass.
    pub fn pump_all(&mut self) -> Result<(), MixerStreamSessionError> {
        let mut first_error = None;
        for record in self.records.iter_mut().flatten() {
            if let Err(error) = record.pump.pump() {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        match first_error {
            Some(error) => Err(MixerStreamSessionError::Adapter(error)),
            None => Ok(()),
        }
    }

    /// Retires one stream's partial conversion state after a stream-local reset.
    ///
    /// # Errors
    ///
    /// Returns [`MixerStreamSessionError::UnknownStream`] when no admitted
    /// stream carries `identity`.
    pub fn begin_epoch(
        &mut self,
        identity: MediaStreamIdentity,
    ) -> Result<(), MixerStreamSessionError> {
        let slot = self
            .slot_for(identity)
            .ok_or(MixerStreamSessionError::UnknownStream(identity))?;
        let Some(record) = self.records[slot].as_mut() else {
            return Err(MixerStreamSessionError::UnknownStream(identity));
        };
        record.pump.begin_epoch();
        Ok(())
    }

    /// Replaces one stream's identity and key epoch without changing its slot.
    ///
    /// Key rotation keeps the same mixer slot, gain handle, and diagnostics
    /// counters. The `next` request must keep the stream's audio configuration:
    /// the source block budget (frame duration and sample rate) and the source
    /// channel and format layout must match, because the decode pipeline and
    /// the adapter pump are compiled for those parameters. Partial conversion
    /// state is retired so the new epoch starts clean.
    ///
    /// # Errors
    ///
    /// Returns [`MixerStreamSessionError::UnknownStream`] when `previous` is
    /// not admitted, [`MixerStreamSessionError::DuplicateStream`] when `next`
    /// is already admitted elsewhere, or
    /// [`MixerStreamSessionError::RotationFormatChange`] when `next` changes
    /// the frame budget or source format.
    pub fn rotate_stream(
        &mut self,
        previous: MediaStreamIdentity,
        next: StreamStartRequest,
    ) -> Result<StartedMixerStream, MixerStreamSessionError> {
        let next_budget = frames_per_block(next)?;
        let next_format = source_format_of(next)?;
        let record_slot = self
            .slot_for(previous)
            .ok_or(MixerStreamSessionError::UnknownStream(previous))?;
        if self.slot_for(next.identity).is_some() {
            return Err(MixerStreamSessionError::DuplicateStream(next.identity));
        }
        let Some(record) = self.records[record_slot].as_mut() else {
            return Err(MixerStreamSessionError::UnknownStream(previous));
        };
        if next_budget != record.samples_per_block || next_format != record.source_format {
            return Err(MixerStreamSessionError::RotationFormatChange);
        }
        // A recovered record can occupy a different fresh mixer slot from its
        // stable record-table position. Preserve the latter for lookup while
        // reporting the slot the stream currently occupies to the caller.
        let membership_slot = record.membership_slot.unwrap_or(record_slot);
        record.identity = next.identity;
        record.pump.begin_epoch();
        Ok(StartedMixerStream {
            slot: membership_slot,
        })
    }

    /// Applies one bounded drift correction to one stream's adapter.
    ///
    /// # Errors
    ///
    /// Returns [`MixerStreamSessionError::UnknownStream`] for an unknown
    /// identity or [`MixerStreamSessionError::Resampler`] when the correction
    /// is outside the shared drift bound.
    pub fn set_rate_adjustment_ppm(
        &mut self,
        identity: MediaStreamIdentity,
        ppm: i32,
    ) -> Result<(), MixerStreamSessionError> {
        let slot = self
            .slot_for(identity)
            .ok_or(MixerStreamSessionError::UnknownStream(identity))?;
        let Some(record) = self.records[slot].as_mut() else {
            return Err(MixerStreamSessionError::UnknownStream(identity));
        };
        record
            .pump
            .set_rate_adjustment_ppm(ppm)
            .map_err(MixerStreamSessionError::Resampler)
    }

    /// Detects a platform fault and retires the membership boundary.
    ///
    /// Stream records persist; [`Self::recover_output`] re-admits them.
    pub fn monitor(&mut self) -> Option<OutputHealth> {
        let health = self.output.monitor();
        if health.is_some() {
            self.membership = None;
            self.clear_membership_slots();
        }
        health
    }

    /// Recreates the output backend with a fresh empty slot table.
    ///
    /// Returns the identities of every persisted stream so the caller can
    /// re-admit each one through [`Self::recover_stream`] and reinstall its
    /// fresh source producer into the matching pipeline.
    ///
    /// # Errors
    ///
    /// Returns [`MixerStreamSessionError::Output`] when recreation fails or
    /// the fresh source cannot be reached.
    pub fn recover_output(&mut self) -> Result<Vec<MediaStreamIdentity>, MixerStreamSessionError> {
        self.output
            .recover()
            .map_err(MixerStreamSessionError::Output)?;
        let membership = self
            .output
            .membership()
            .ok_or(MixerStreamSessionError::OutputNotActive)?;
        let output_format = self
            .output
            .output_format()
            .ok_or(MixerStreamSessionError::OutputNotActive)?;
        self.membership = Some(
            MixerMembershipController::new(membership, self.queue_capacity, output_format)
                .map_err(MixerStreamSessionError::Membership)?,
        );
        Ok(self
            .records
            .iter()
            .flatten()
            .map(|record| record.identity)
            .collect())
    }

    /// Re-admits one persisted stream against the recovered output.
    ///
    /// The caller calls this once per identity returned by
    /// [`Self::recover_output`], passing the stream's pipeline so the fresh
    /// source producer replaces the retired one.
    ///
    /// # Errors
    ///
    /// Returns an error when the output is not active, the identity is not
    /// persisted, or admission fails closed.
    pub fn recover_stream<P, const S: usize>(
        &mut self,
        identity: MediaStreamIdentity,
        pipeline: &mut P,
    ) -> Result<StartedMixerStream, MixerStreamSessionError>
    where
        P: ReceiverPcmProducerInstaller<S>,
    {
        let record_slot = self
            .slot_for(identity)
            .ok_or(MixerStreamSessionError::UnknownStream(identity))?;
        let Some(record) = self.records[record_slot].as_ref() else {
            return Err(MixerStreamSessionError::UnknownStream(identity));
        };
        if record.membership_slot.is_some() {
            return Err(MixerStreamSessionError::DuplicateStream(identity));
        }
        let source_format = record.source_format;
        let source_queue_capacity = record.source_queue_capacity;
        let gain = record.gain.clone();
        let counters = record.counters.clone();
        let membership = self
            .membership
            .as_mut()
            .ok_or(MixerStreamSessionError::OutputNotActive)?;
        let admitted = membership
            .admit_adapted_stream::<S>(source_format, source_queue_capacity, gain, counters)
            .map_err(MixerStreamSessionError::Membership)?;
        let membership_slot = admitted.slot;
        pipeline.replace_pcm_producer(admitted.source_producer);
        let Some(record) = self.records[record_slot].as_mut() else {
            return Err(MixerStreamSessionError::UnknownStream(identity));
        };
        record.pump = Box::new(admitted.adapter);
        record.membership_slot = Some(membership_slot);
        Ok(StartedMixerStream {
            slot: membership_slot,
        })
    }

    /// Stops the output and retires every stream record.
    pub fn stop(&mut self) {
        self.output.stop();
        self.membership = None;
        self.records = [const { None }; SLOTS];
    }

    /// Stops the output and retires its realtime membership boundary while
    /// preserving stream records for a later output recovery.
    pub fn retire_output_preserving_streams(&mut self) {
        self.output.stop();
        self.membership = None;
        self.clear_membership_slots();
    }

    /// Returns the clone of one stream's gain and mute handle.
    ///
    /// # Errors
    ///
    /// Returns [`MixerStreamSessionError::UnknownStream`] when no admitted
    /// stream carries `identity`.
    pub fn gain(
        &self,
        identity: MediaStreamIdentity,
    ) -> Result<GainControlHandle, MixerStreamSessionError> {
        let slot = self
            .slot_for(identity)
            .ok_or(MixerStreamSessionError::UnknownStream(identity))?;
        let Some(record) = self.records[slot].as_ref() else {
            return Err(MixerStreamSessionError::UnknownStream(identity));
        };
        Ok(record.gain.clone())
    }

    /// Approximate source-format blocks waiting for conversion for one stream.
    ///
    /// # Errors
    ///
    /// Returns [`MixerStreamSessionError::UnknownStream`] when no admitted
    /// stream carries `identity`.
    pub fn queued_source_blocks(
        &self,
        identity: MediaStreamIdentity,
    ) -> Result<usize, MixerStreamSessionError> {
        let slot = self
            .slot_for(identity)
            .ok_or(MixerStreamSessionError::UnknownStream(identity))?;
        let Some(record) = self.records[slot].as_ref() else {
            return Err(MixerStreamSessionError::UnknownStream(identity));
        };
        Ok(record.pump.queued_source_blocks())
    }

    /// Approximate output-format blocks waiting for the mixer for one stream.
    ///
    /// # Errors
    ///
    /// Returns [`MixerStreamSessionError::UnknownStream`] when no admitted
    /// stream carries `identity`.
    pub fn queued_output_blocks(
        &self,
        identity: MediaStreamIdentity,
    ) -> Result<usize, MixerStreamSessionError> {
        let slot = self
            .slot_for(identity)
            .ok_or(MixerStreamSessionError::UnknownStream(identity))?;
        let Some(record) = self.records[slot].as_ref() else {
            return Err(MixerStreamSessionError::UnknownStream(identity));
        };
        Ok(record.pump.queued_output_blocks())
    }

    /// Number of streams admitted into the current or last output lifetime.
    #[must_use]
    pub fn stream_count(&self) -> usize {
        self.records.iter().flatten().count()
    }

    /// Membership commands waiting to be applied by the callback.
    #[must_use]
    pub fn pending_commands(&self) -> usize {
        self.membership
            .as_ref()
            .map_or(0, MixerMembershipController::pending_commands)
    }

    /// Returns a redacted low-rate output lifecycle snapshot.
    #[must_use]
    pub fn snapshot(&self) -> OutputSnapshot {
        self.output.snapshot()
    }

    /// Collects one redacted metrics snapshot per active stream, in slot order.
    ///
    /// A stream admitted without a counters endpoint exports a zeroed entry so
    /// bundle ordinals stay stable. Reads relaxed atomic counters; a concurrent
    /// event may appear in the next snapshot.
    #[must_use]
    pub fn stream_metrics(&self) -> Vec<StreamMetricsSnapshot> {
        self.records
            .iter()
            .flatten()
            .map(|record| {
                record.counters.as_ref().map_or_else(
                    || StreamCounters::default().snapshot(),
                    |counters| counters.snapshot(),
                )
            })
            .collect()
    }

    /// Borrows the platform adapter for control-domain device operations.
    #[must_use]
    pub const fn output(&self) -> &O {
        self.output.output()
    }

    /// Mutably borrows the platform adapter outside the realtime callback.
    pub fn output_mut(&mut self) -> &mut O {
        self.output.output_mut()
    }

    fn clear_membership_slots(&mut self) {
        for record in self.records.iter_mut().flatten() {
            record.membership_slot = None;
        }
    }

    fn slot_for(&self, identity: MediaStreamIdentity) -> Option<usize> {
        self.records.iter().position(|record| {
            record
                .as_ref()
                .is_some_and(|record| record.identity == identity)
        })
    }
}

/// Failure while starting, admitting, removing, or recovering a mixer stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MixerStreamSessionError {
    /// Output start or recovery failed at the platform boundary.
    Output(MixerOutputLifecycleError),
    /// The stream could not be admitted or removed at the mixer boundary.
    Membership(MixerAdmissionError),
    /// The negotiated source format is outside the shared audio bounds.
    SourceFormat(SourcePcmFormatError),
    /// The pipeline's source block size differs from the frame budget.
    SourceBlockMismatch {
        /// Frames per block required by the negotiated frame duration and rate.
        expected: usize,
    },
    /// A stream with this identity is already admitted.
    DuplicateStream(MediaStreamIdentity),
    /// No admitted stream carries this identity.
    UnknownStream(MediaStreamIdentity),
    /// A stream operation was requested before the output started.
    OutputNotActive,
    /// The frame-duration and sample-rate budget cannot be represented.
    FrameBudgetOverflow,
    /// The frame-duration and sample-rate budget does not divide into whole frames.
    NonIntegralFrameBlock,
    /// A key rotation changed the stream's frame budget or source format.
    RotationFormatChange,
    /// A decode-domain adapter pump or reset failed.
    Adapter(StreamFormatAdapterError),
    /// A drift correction was rejected by the shared bound.
    Resampler(ResamplerError),
}

impl fmt::Display for MixerStreamSessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Output(error) => write!(formatter, "mixer session output failed: {error}"),
            Self::Membership(error) => {
                write!(formatter, "mixer session membership failed: {error}")
            }
            Self::SourceFormat(error) => {
                write!(formatter, "mixer session source format failed: {error}")
            }
            Self::SourceBlockMismatch { expected } => write!(
                formatter,
                "mixer session pipeline block size must be {expected} frames"
            ),
            Self::DuplicateStream(identity) => {
                write!(
                    formatter,
                    "mixer session stream {identity:?} is already admitted"
                )
            }
            Self::UnknownStream(identity) => {
                write!(
                    formatter,
                    "mixer session stream {identity:?} is not admitted"
                )
            }
            Self::OutputNotActive => formatter.write_str("mixer session output is not active"),
            Self::FrameBudgetOverflow => {
                formatter.write_str("mixer session frame budget overflowed")
            }
            Self::NonIntegralFrameBlock => {
                formatter.write_str("mixer session frame budget is not whole frames")
            }
            Self::RotationFormatChange => {
                formatter.write_str("key rotation changed the stream's audio configuration")
            }
            Self::Adapter(error) => write!(formatter, "mixer session adapter failed: {error}"),
            Self::Resampler(error) => {
                write!(formatter, "mixer session drift correction failed: {error}")
            }
        }
    }
}

impl std::error::Error for MixerStreamSessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Output(error) => Some(error),
            Self::Membership(error) => Some(error),
            Self::SourceFormat(error) => Some(error),
            Self::Adapter(error) => Some(error),
            Self::Resampler(error) => Some(error),
            Self::SourceBlockMismatch { .. }
            | Self::DuplicateStream(_)
            | Self::UnknownStream(_)
            | Self::OutputNotActive
            | Self::FrameBudgetOverflow
            | Self::NonIntegralFrameBlock
            | Self::RotationFormatChange => None,
        }
    }
}

fn source_format_of(
    request: StreamStartRequest,
) -> Result<SourcePcmFormat, MixerStreamSessionError> {
    let channels = u16::try_from(request.channels).map_err(|_| {
        MixerStreamSessionError::SourceFormat(SourcePcmFormatError::InvalidChannelCount)
    })?;
    SourcePcmFormat::new(request.sample_rate_hz, channels)
        .map_err(MixerStreamSessionError::SourceFormat)
}

fn frames_per_block(request: StreamStartRequest) -> Result<usize, MixerStreamSessionError> {
    let numerator = u64::from(request.frame_duration_ms)
        .checked_mul(u64::from(request.sample_rate_hz))
        .ok_or(MixerStreamSessionError::FrameBudgetOverflow)?;
    if numerator % 1_000 != 0 {
        return Err(MixerStreamSessionError::NonIntegralFrameBlock);
    }
    usize::try_from(numerator / 1_000).map_err(|_| MixerStreamSessionError::FrameBudgetOverflow)
}
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sonimesh_audio::{
        MAX_DRIFT_ADJUSTMENT_PPM, MAX_PCM_QUEUE_BLOCKS, RealtimePcmProducer,
        RealtimePcmPushOutcome, RealtimeQueueConfigError,
    };
    use sonimesh_diagnostics::StreamCounters;
    use sonimesh_platform::{
        AudioOutput, OutputConfig, OutputDeviceId, OutputDeviceInfo, OutputError, OutputHealth,
        RenderSource, SampleFormat,
    };

    use super::{MixerStreamSession, MixerStreamSessionError};
    use crate::{
        GainControlHandle, MediaStreamIdentity, MixerAdmissionError, MixerOutputLifecycleError,
        OutputSelection, ReceiverPcmProducerInstaller, SourcePcmFormatError, StreamStartRequest,
    };

    const SLOTS: usize = 2;
    const OUTPUT_SAMPLES: usize = 256;
    const SOURCE_SAMPLES: usize = 480;

    struct FakeOutput {
        started: bool,
        stopped: bool,
        health: OutputHealth,
        fail_start: bool,
    }

    impl Default for FakeOutput {
        fn default() -> Self {
            Self {
                started: false,
                stopped: false,
                health: OutputHealth::Healthy,
                fail_start: false,
            }
        }
    }

    impl AudioOutput for FakeOutput {
        fn devices(&self) -> Result<Vec<OutputDeviceInfo>, OutputError> {
            Ok(Vec::new())
        }

        fn start(
            &mut self,
            _device: Option<&OutputDeviceId>,
            _config: OutputConfig,
            _source: Box<dyn RenderSource>,
        ) -> Result<(), OutputError> {
            if self.fail_start {
                return Err(OutputError::DeviceUnavailable);
            }
            self.started = true;
            self.stopped = false;
            Ok(())
        }

        fn stop(&mut self) {
            self.started = false;
            self.stopped = true;
        }

        fn health(&self) -> OutputHealth {
            self.health
        }
    }

    struct FakePipeline<const S: usize> {
        replaced: usize,
        producer: Option<RealtimePcmProducer<S>>,
    }

    impl<const S: usize> Default for FakePipeline<S> {
        fn default() -> Self {
            Self {
                replaced: 0,
                producer: None,
            }
        }
    }

    impl<const S: usize> ReceiverPcmProducerInstaller<S> for FakePipeline<S> {
        fn replace_pcm_producer(&mut self, producer: RealtimePcmProducer<S>) {
            self.replaced += 1;
            self.producer = Some(producer);
        }
    }

    fn selection(device: Option<OutputDeviceId>) -> OutputSelection {
        OutputSelection {
            device,
            config: OutputConfig {
                sample_rate_hz: 48_000,
                channels: 2,
                sample_format: SampleFormat::Float32,
                requested_callback_frames: Some(480),
                maximum_callback_frames: 1_024,
            },
        }
    }

    fn identity(stream_id: u32) -> MediaStreamIdentity {
        MediaStreamIdentity {
            session_id: 0x0102_0304,
            stream_id,
            key_epoch: 0,
        }
    }

    fn request(identity: MediaStreamIdentity) -> StreamStartRequest {
        StreamStartRequest {
            identity,
            sample_rate_hz: 48_000,
            channels: 1,
            frame_duration_ms: 10,
        }
    }

    fn session(output: FakeOutput) -> MixerStreamSession<FakeOutput, SLOTS, OUTPUT_SAMPLES> {
        MixerStreamSession::new(output, 8).expect("fixed test session is valid")
    }

    fn started(
        session: &mut MixerStreamSession<FakeOutput, SLOTS, OUTPUT_SAMPLES>,
        identity: MediaStreamIdentity,
        pipeline: &mut FakePipeline<SOURCE_SAMPLES>,
    ) {
        session
            .start_stream(request(identity), 8, None, pipeline)
            .unwrap();
    }

    #[test]
    fn new_rejects_zero_and_oversized_queue_capacity() {
        assert!(matches!(
            MixerStreamSession::<FakeOutput, SLOTS, OUTPUT_SAMPLES>::new(FakeOutput::default(), 0),
            Err(MixerStreamSessionError::Membership(
                MixerAdmissionError::Queue(RealtimeQueueConfigError::ZeroCapacity)
            ))
        ));
        assert!(matches!(
            MixerStreamSession::<FakeOutput, SLOTS, OUTPUT_SAMPLES>::new(
                FakeOutput::default(),
                MAX_PCM_QUEUE_BLOCKS + 1
            ),
            Err(MixerStreamSessionError::Membership(
                MixerAdmissionError::Queue(RealtimeQueueConfigError::CapacityTooLarge)
            ))
        ));
    }

    #[test]
    fn start_stream_requires_an_active_output() {
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        let mut session = session(FakeOutput::default());

        assert_eq!(
            session.start_stream(request(identity(1)), 8, None, &mut pipeline),
            Err(MixerStreamSessionError::OutputNotActive)
        );
        assert_eq!(pipeline.replaced, 0);
        assert_eq!(session.stream_count(), 0);
    }

    #[test]
    fn start_output_rejects_an_already_active_stream() {
        let mut session = session(FakeOutput::default());
        session.start_output(selection(None)).unwrap();

        assert_eq!(
            session.start_output(selection(None)),
            Err(MixerStreamSessionError::Output(
                MixerOutputLifecycleError::StreamAlreadyActive
            ))
        );
    }

    #[test]
    fn start_stream_admits_into_the_lowest_slot_and_installs_the_producer() {
        let mut session = session(FakeOutput::default());
        session.start_output(selection(None)).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();

        let started = session
            .start_stream(request(identity(1)), 8, None, &mut pipeline)
            .unwrap();
        assert_eq!(started.slot, 0);
        assert_eq!(pipeline.replaced, 1);
        assert!(pipeline.producer.is_some());
        assert_eq!(session.stream_count(), 1);
        assert!(session.pending_commands() >= 1);
        assert!(session.snapshot().stream_active());
        assert!(session.gain(identity(1)).is_ok());
    }

    #[test]
    fn start_stream_rejects_a_duplicate_identity() {
        let mut session = session(FakeOutput::default());
        session.start_output(selection(None)).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        started(&mut session, identity(1), &mut pipeline);

        assert_eq!(
            session.start_stream(request(identity(1)), 8, None, &mut pipeline),
            Err(MixerStreamSessionError::DuplicateStream(identity(1)))
        );
        assert_eq!(session.stream_count(), 1);
    }

    #[test]
    fn start_stream_rejects_a_mismatched_source_block_size() {
        let mut session = session(FakeOutput::default());
        session.start_output(selection(None)).unwrap();
        let mut pipeline = FakePipeline::<256>::default();

        assert_eq!(
            session.start_stream(request(identity(1)), 8, None, &mut pipeline),
            Err(MixerStreamSessionError::SourceBlockMismatch {
                expected: SOURCE_SAMPLES
            })
        );
        assert_eq!(session.stream_count(), 0);
    }

    #[test]
    fn start_stream_rejects_an_invalid_source_format() {
        let mut session = session(FakeOutput::default());
        session.start_output(selection(None)).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        let mut invalid = request(identity(1));
        invalid.channels = 0;

        assert!(matches!(
            session.start_stream(invalid, 8, None, &mut pipeline),
            Err(MixerStreamSessionError::SourceFormat(
                SourcePcmFormatError::InvalidChannelCount
            ))
        ));
        assert_eq!(session.stream_count(), 0);
    }

    #[test]
    fn pump_all_underflows_without_source_data() {
        let mut session = session(FakeOutput::default());
        session.start_output(selection(None)).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        started(&mut session, identity(1), &mut pipeline);

        assert_eq!(session.pump_all(), Ok(()));
        assert_eq!(session.queued_source_blocks(identity(1)).unwrap(), 0);
    }

    #[test]
    fn pump_all_converts_queued_source_blocks_into_the_slot_queue() {
        let mut session = session(FakeOutput::default());
        session.start_output(selection(None)).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        started(&mut session, identity(1), &mut pipeline);

        let producer = pipeline.producer.as_mut().unwrap();
        assert_eq!(
            producer.push(&[0.5; SOURCE_SAMPLES]),
            RealtimePcmPushOutcome::Inserted
        );
        assert_eq!(session.queued_source_blocks(identity(1)).unwrap(), 1);

        session.pump_all().unwrap();
        assert_eq!(session.queued_source_blocks(identity(1)).unwrap(), 0);
        assert_eq!(session.queued_output_blocks(identity(1)).unwrap(), 1);
    }

    #[test]
    fn begin_epoch_retires_partial_conversion_state() {
        let mut session = session(FakeOutput::default());
        session.start_output(selection(None)).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        started(&mut session, identity(1), &mut pipeline);

        let producer = pipeline.producer.as_mut().unwrap();
        assert_eq!(
            producer.push(&[0.5; SOURCE_SAMPLES]),
            RealtimePcmPushOutcome::Inserted
        );
        session.pump_all().unwrap();
        assert_eq!(session.queued_output_blocks(identity(1)).unwrap(), 1);

        session.begin_epoch(identity(1)).unwrap();
        assert_eq!(session.queued_output_blocks(identity(1)).unwrap(), 0);
        assert_eq!(
            session.begin_epoch(identity(9)),
            Err(MixerStreamSessionError::UnknownStream(identity(9)))
        );
    }

    #[test]
    fn remove_stream_retires_the_slot_for_reuse() {
        let mut session = session(FakeOutput::default());
        session.start_output(selection(None)).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        started(&mut session, identity(1), &mut pipeline);
        started(&mut session, identity(2), &mut pipeline);
        assert_eq!(session.stream_count(), 2);

        session.remove_stream(identity(1)).unwrap();
        assert_eq!(session.stream_count(), 1);
        assert!(matches!(
            session.gain(identity(1)),
            Err(MixerStreamSessionError::UnknownStream(found))
                if found == identity(1)
        ));

        let readmitted = session
            .start_stream(request(identity(3)), 8, None, &mut pipeline)
            .unwrap();
        assert_eq!(readmitted.slot, 0);
        assert_eq!(
            session.remove_stream(identity(9)),
            Err(MixerStreamSessionError::UnknownStream(identity(9)))
        );
    }

    #[test]
    fn rotate_stream_replaces_an_identity_in_place() {
        let mut session = session(FakeOutput::default());
        session.start_output(selection(None)).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        started(&mut session, identity(1), &mut pipeline);

        let rotated = session
            .rotate_stream(identity(1), request(identity(2)))
            .unwrap();
        assert_eq!(rotated.slot, 0);
        assert_eq!(session.stream_count(), 1);
        assert!(session.gain(identity(2)).is_ok());
        assert!(matches!(
            session.gain(identity(1)),
            Err(MixerStreamSessionError::UnknownStream(found))
                if found == identity(1)
        ));
    }

    #[test]
    fn rotate_stream_rejects_unknown_format_changes_and_duplicates() {
        let mut session = session(FakeOutput::default());
        session.start_output(selection(None)).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        started(&mut session, identity(1), &mut pipeline);
        started(&mut session, identity(2), &mut pipeline);

        assert_eq!(
            session.rotate_stream(identity(9), request(identity(3))),
            Err(MixerStreamSessionError::UnknownStream(identity(9)))
        );
        assert_eq!(
            session.rotate_stream(identity(1), request(identity(2))),
            Err(MixerStreamSessionError::DuplicateStream(identity(2)))
        );

        let mut changed = request(identity(3));
        changed.frame_duration_ms = 20;
        assert_eq!(
            session.rotate_stream(identity(1), changed),
            Err(MixerStreamSessionError::RotationFormatChange)
        );
        assert_eq!(session.stream_count(), 2);
    }

    #[test]
    fn set_rate_adjustment_ppm_applies_bounded_corrections() {
        let mut session = session(FakeOutput::default());
        session.start_output(selection(None)).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        started(&mut session, identity(1), &mut pipeline);

        session.set_rate_adjustment_ppm(identity(1), 100).unwrap();
        assert!(matches!(
            session.set_rate_adjustment_ppm(identity(1), MAX_DRIFT_ADJUSTMENT_PPM + 1),
            Err(MixerStreamSessionError::Resampler(_))
        ));
        assert_eq!(
            session.set_rate_adjustment_ppm(identity(9), 0),
            Err(MixerStreamSessionError::UnknownStream(identity(9)))
        );
    }

    #[test]
    fn gain_returns_the_shared_control_handle() {
        let mut session = session(FakeOutput::default());
        session.start_output(selection(None)).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        started(&mut session, identity(1), &mut pipeline);

        let handle: GainControlHandle = session.gain(identity(1)).unwrap();
        handle.set_gain(0.5).unwrap();
        let shared = session.gain(identity(1)).unwrap();
        assert!((shared.gain() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn monitor_retires_membership_but_keeps_stream_records() {
        let output = FakeOutput {
            health: OutputHealth::StreamError,
            ..Default::default()
        };
        let mut session = session(output);
        session.start_output(selection(None)).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        started(&mut session, identity(1), &mut pipeline);

        assert_eq!(session.monitor(), Some(OutputHealth::StreamError));
        assert_eq!(session.monitor(), None);
        assert_eq!(session.stream_count(), 1);
        assert_eq!(session.pending_commands(), 0);
        assert!(!session.snapshot().stream_active());
        assert!(session.snapshot().fault_reported());
        assert_eq!(
            session.start_stream(request(identity(2)), 8, None, &mut pipeline),
            Err(MixerStreamSessionError::OutputNotActive)
        );
    }

    #[test]
    fn recover_output_readmits_persisted_streams() {
        let output = FakeOutput {
            health: OutputHealth::CallbackCapacityExceeded,
            ..Default::default()
        };
        let mut session = session(output);
        session.start_output(selection(None)).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        started(&mut session, identity(1), &mut pipeline);
        started(&mut session, identity(2), &mut pipeline);
        assert_eq!(
            session.monitor(),
            Some(OutputHealth::CallbackCapacityExceeded)
        );

        let identities = session.recover_output().unwrap();
        assert_eq!(identities, vec![identity(1), identity(2)]);
        assert!(session.snapshot().stream_active());

        let recovered = session.recover_stream(identity(1), &mut pipeline).unwrap();
        assert_eq!(recovered.slot, 0);
        assert_eq!(pipeline.replaced, 3);
        assert_eq!(
            session.recover_stream(identity(9), &mut pipeline),
            Err(MixerStreamSessionError::UnknownStream(identity(9)))
        );
    }

    #[test]
    fn recovery_keeps_records_when_streams_rejoin_out_of_record_order() {
        let output = FakeOutput {
            health: OutputHealth::CallbackCapacityExceeded,
            ..Default::default()
        };
        let mut session = session(output);
        session.start_output(selection(None)).unwrap();
        let mut first_pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        let mut second_pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        started(&mut session, identity(1), &mut first_pipeline);
        started(&mut session, identity(2), &mut second_pipeline);
        assert_eq!(
            session.monitor(),
            Some(OutputHealth::CallbackCapacityExceeded)
        );

        assert_eq!(
            session.recover_output().unwrap(),
            vec![identity(1), identity(2)]
        );
        let second = session
            .recover_stream(identity(2), &mut second_pipeline)
            .unwrap();
        assert_eq!(second.slot, 0);
        let first = session
            .recover_stream(identity(1), &mut first_pipeline)
            .unwrap();
        assert_eq!(first.slot, 1);

        assert_eq!(session.stream_count(), 2);
        assert_eq!(session.pending_commands(), 2);
        assert_eq!(
            session.recover_stream(identity(2), &mut second_pipeline),
            Err(MixerStreamSessionError::DuplicateStream(identity(2)))
        );
        assert_eq!(session.pending_commands(), 2);

        // Record-table positions stay stable while recovery assigns fresh mixer
        // slots, so public operations must report and target the current
        // membership slot rather than the persisted record position.
        let mut rotated_identity = identity(1);
        rotated_identity.key_epoch = 1;
        let rotated = session
            .rotate_stream(identity(1), request(rotated_identity))
            .unwrap();
        assert_eq!(rotated.slot, 1);

        session.remove_stream(identity(2)).unwrap();
        assert_eq!(session.pending_commands(), 3);
        session.remove_stream(rotated_identity).unwrap();
        assert_eq!(session.pending_commands(), 4);
        assert_eq!(session.stream_count(), 0);
    }

    #[test]
    fn stop_retires_every_stream_record() {
        let mut session = session(FakeOutput::default());
        session.start_output(selection(None)).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        started(&mut session, identity(1), &mut pipeline);
        started(&mut session, identity(2), &mut pipeline);

        session.stop();
        assert_eq!(session.stream_count(), 0);
        assert_eq!(session.pending_commands(), 0);
        assert!(session.output().stopped);
        assert!(!session.snapshot().stream_active());
        assert!(matches!(
            session.gain(identity(1)),
            Err(MixerStreamSessionError::UnknownStream(found))
                if found == identity(1)
        ));
    }

    #[test]
    fn stream_metrics_reports_one_redacted_entry_per_stream() {
        let mut session = session(FakeOutput::default());
        session.start_output(selection(None)).unwrap();
        let mut pipeline = FakePipeline::<SOURCE_SAMPLES>::default();
        session
            .start_stream(
                request(identity(1)),
                8,
                Some(Arc::new(StreamCounters::default())),
                &mut pipeline,
            )
            .unwrap();
        started(&mut session, identity(2), &mut pipeline);

        let metrics = session.stream_metrics();
        assert_eq!(metrics.len(), 2);
        assert_eq!(session.stream_metrics().len(), session.stream_count());
    }
}
