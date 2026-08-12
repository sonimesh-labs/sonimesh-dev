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

use core::fmt;
use std::sync::Arc;

use sonimesh_audio::{MAX_PCM_QUEUE_BLOCKS, RealtimeQueueConfigError, ResamplerError};
use sonimesh_diagnostics::StreamCounters;
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
    source_format: SourcePcmFormat,
    source_queue_capacity: usize,
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
        let channels = u16::try_from(request.channels).map_err(|_| {
            MixerStreamSessionError::SourceFormat(SourcePcmFormatError::InvalidChannelCount)
        })?;
        let source_format = SourcePcmFormat::new(request.sample_rate_hz, channels)
            .map_err(MixerStreamSessionError::SourceFormat)?;
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
            source_format,
            source_queue_capacity,
            gain: gain.clone(),
            counters,
            pump: Box::new(admitted.adapter),
        });
        let _ = gain;
        Ok(StartedMixerStream { slot })
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
        let slot = self
            .slot_for(identity)
            .ok_or(MixerStreamSessionError::UnknownStream(identity))?;
        if let Some(membership) = self.membership.as_mut() {
            membership
                .remove_stream(slot)
                .map_err(MixerStreamSessionError::Membership)?;
        }
        self.records[slot] = None;
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
        let slot = self
            .slot_for(identity)
            .ok_or(MixerStreamSessionError::UnknownStream(identity))?;
        let membership = self
            .membership
            .as_mut()
            .ok_or(MixerStreamSessionError::OutputNotActive)?;
        let Some(record) = self.records[slot].as_ref() else {
            return Err(MixerStreamSessionError::UnknownStream(identity));
        };
        let admitted = membership
            .admit_adapted_stream::<S>(
                record.source_format,
                record.source_queue_capacity,
                record.gain.clone(),
                record.counters.clone(),
            )
            .map_err(MixerStreamSessionError::Membership)?;
        let new_slot = admitted.slot;
        pipeline.replace_pcm_producer(admitted.source_producer);
        let Some(record) = self.records[slot].as_mut() else {
            return Err(MixerStreamSessionError::UnknownStream(identity));
        };
        record.pump = Box::new(admitted.adapter);
        if new_slot != slot {
            self.records[new_slot] = self.records[slot].take();
        }
        Ok(StartedMixerStream { slot: new_slot })
    }

    /// Stops the output and retires every stream record.
    pub fn stop(&mut self) {
        self.output.stop();
        self.membership = None;
        self.records = [const { None }; SLOTS];
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

    /// Borrows the platform adapter for control-domain device operations.
    #[must_use]
    pub const fn output(&self) -> &O {
        self.output.output()
    }

    /// Mutably borrows the platform adapter outside the realtime callback.
    pub fn output_mut(&mut self) -> &mut O {
        self.output.output_mut()
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
            | Self::NonIntegralFrameBlock => None,
        }
    }
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
