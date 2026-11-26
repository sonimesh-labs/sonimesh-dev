//! Shell-neutral owner of one desktop receiver scheduling loop.
//!
//! This boundary keeps receiver ownership, an optional framed control channel,
//! and low-rate media ticks together without introducing a UI or async runtime.
//! The caller still chooses the thread, timer, and platform output backend.

use crate::desktop_receiver_owner::MediaPrepare;
use std::io::{Read, Write};

use sonimesh_audio::{DecodeStep, DecodeWorkerError, FrameDecoder};
use sonimesh_network::{PinnedServerChannel, UdpReceiveError};
use sonimesh_network::{UntrustedControlStream, UntrustedControlWrite};
use sonimesh_platform::{AudioOutput, OutputHealth};

use crate::{
    ControlSessionError, ControlSessionPoll, ControlSessionToken, ControlStreamCoordinator,
    DesktopControlOwnerError, DesktopControlSession, DesktopReceiverOwner,
    ReceiverPcmProducerInstaller, ReconnectGraceObservation, SessionEvent, StreamStartRequest,
    TrustedControlChannel, UdpMediaTick, UdpPollOutcome, UdpReceiverError, UdpReceiverPipeline,
};

/// Maximum UDP polls performed by one clock-driven scheduler call.
pub const MAX_SCHEDULED_UDP_POLLS: usize = 16;

const MIN_SCHEDULED_STALL_MS: u64 = 100;
const SCHEDULED_STALL_FRAMES: u64 = 8;
const MAX_CONSECUTIVE_EMPTY_PLC: u8 = 3;

/// Media readiness observed by the clock-driven receiver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduledReceiverState {
    /// No control stream is active.
    Idle,
    /// An active control stream does not match the configured media pipeline.
    IdentityMismatch,
    /// The matching stream is active, but its output is unavailable.
    OutputUnavailable,
    /// Output was recreated, but the matching stream has not rejoined it.
    AwaitingStreamRecovery,
    /// Stale UDP and buffered audio are being retired after a scheduling stall.
    DrainingStaleMedia,
    /// The matching stream is admitted to a running output.
    Active,
}

/// Independent results from a clock-driven receiver scheduler call.
#[derive(Debug)]
pub struct ScheduledReceiverTick<DecoderError> {
    /// Readiness of the media pipeline and output for this call.
    pub state: ScheduledReceiverState,
    /// One control transport observation.
    pub control: Result<Option<ControlSessionPoll>, ControlSessionError>,
    /// Number of processed packets, or the first UDP error.
    pub packets: Result<usize, UdpReceiverError<DecoderError>>,
    /// One due decode decision, no decision if not due, or its failure.
    pub playout: Result<Option<DecodeStep>, ScheduledPlayoutError<DecoderError>>,
    /// PCM conversion result, independent of the other operations.
    pub pump: Result<(), crate::MixerStreamSessionError>,
    /// Reconnect-grace observation.
    pub reconnect: Result<ReconnectGraceObservation, DesktopControlOwnerError>,
    /// Output-health observation.
    pub output: Result<Option<OutputHealth>, DesktopControlOwnerError>,
}

/// A clock-driven playout decision could not be performed.
#[derive(Debug)]
pub enum ScheduledPlayoutError<DecoderError> {
    /// The supplied monotonic timestamp is earlier than the previous one.
    ClockRegression {
        /// Last accepted timestamp in milliseconds.
        previous_ms: u64,
        /// Regressed timestamp in milliseconds.
        now_ms: u64,
    },
    /// The stream-local decoder failed.
    Decode(DecodeWorkerError<DecoderError>),
}

/// Failure while attaching a control channel to a receiver scheduler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesktopReceiverSchedulerError {
    /// A control channel is already attached to this scheduler.
    ControlAlreadyAttached,
    /// The trusted channel could not be configured for non-blocking I/O.
    Socket(std::io::ErrorKind),
}

impl core::fmt::Display for DesktopReceiverSchedulerError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ControlAlreadyAttached => {
                formatter.write_str("desktop receiver already has a control channel")
            }
            Self::Socket(kind) => write!(formatter, "control socket setup failed: {kind:?}"),
        }
    }
}

impl std::error::Error for DesktopReceiverSchedulerError {}

/// Failure while replacing a lost control channel after reconnection.
#[derive(Debug)]
pub enum DesktopReceiverSchedulerReconnectError {
    /// A control channel is still attached to this scheduler.
    ControlAlreadyAttached,
    /// The receiver lifecycle could not return to a fresh ready session.
    Lifecycle(DesktopControlOwnerError),
    /// The trusted channel could not be configured for non-blocking I/O.
    Socket(std::io::ErrorKind),
}

impl core::fmt::Display for DesktopReceiverSchedulerReconnectError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ControlAlreadyAttached => {
                formatter.write_str("desktop receiver still has a control channel")
            }
            Self::Lifecycle(error) => {
                write!(formatter, "desktop receiver reconnect failed: {error}")
            }
            Self::Socket(kind) => write!(formatter, "control socket setup failed: {kind:?}"),
        }
    }
}

impl std::error::Error for DesktopReceiverSchedulerReconnectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ControlAlreadyAttached | Self::Socket(_) => None,
            Self::Lifecycle(error) => Some(error),
        }
    }
}

/// Results from one combined control and UDP media scheduler tick.
///
/// Control and media are intentionally reported independently. A transport
/// error retires only the control channel, while the media result still records
/// the same tick's UDP poll/decode outcome.
#[derive(Debug)]
pub struct DesktopReceiverSchedulerTick<DecoderError> {
    control: Result<Option<ControlSessionPoll>, ControlSessionError>,
    media: Result<UdpMediaTick, UdpReceiverError<DecoderError>>,
    pump: Result<(), crate::MixerStreamSessionError>,
}

impl<DecoderError> DesktopReceiverSchedulerTick<DecoderError> {
    /// Returns the control result for this tick.
    #[must_use = "inspect the control result"]
    pub const fn control(&self) -> &Result<Option<ControlSessionPoll>, ControlSessionError> {
        &self.control
    }

    /// Returns the media result for this tick.
    #[must_use = "inspect the media result"]
    pub const fn media(&self) -> &Result<UdpMediaTick, UdpReceiverError<DecoderError>> {
        &self.media
    }

    /// Returns the independent decoded-PCM conversion result for this tick.
    #[must_use = "inspect the mixer pump result"]
    pub const fn pump(&self) -> &Result<(), crate::MixerStreamSessionError> {
        &self.pump
    }

    /// Splits out control and media after inspecting the pump result.
    #[must_use = "handle both scheduler results"]
    pub fn into_parts(
        self,
    ) -> (
        Result<Option<ControlSessionPoll>, ControlSessionError>,
        Result<UdpMediaTick, UdpReceiverError<DecoderError>>,
    ) {
        (self.control, self.media)
    }
}

/// Results from one scheduler tick that also services reconnect grace.
#[derive(Debug)]
pub struct DesktopReceiverSchedulerMaintenanceTick<DecoderError> {
    control: Result<Option<ControlSessionPoll>, ControlSessionError>,
    media: Result<UdpMediaTick, UdpReceiverError<DecoderError>>,
    pump: Result<(), crate::MixerStreamSessionError>,
    reconnect: Result<ReconnectGraceObservation, DesktopControlOwnerError>,
    output: Result<Option<OutputHealth>, DesktopControlOwnerError>,
}

impl<DecoderError> DesktopReceiverSchedulerMaintenanceTick<DecoderError> {
    /// Returns the control result for this tick.
    #[must_use = "inspect the control result"]
    pub const fn control(&self) -> &Result<Option<ControlSessionPoll>, ControlSessionError> {
        &self.control
    }

    /// Returns the media result for this tick.
    #[must_use = "inspect the media result"]
    pub const fn media(&self) -> &Result<UdpMediaTick, UdpReceiverError<DecoderError>> {
        &self.media
    }

    /// Returns the independent decoded-PCM conversion result for this tick.
    #[must_use = "inspect the mixer pump result"]
    pub const fn pump(&self) -> &Result<(), crate::MixerStreamSessionError> {
        &self.pump
    }

    /// Returns the reconnect-grace observation for this tick.
    #[must_use = "inspect the reconnect-grace result"]
    pub const fn reconnect(&self) -> &Result<ReconnectGraceObservation, DesktopControlOwnerError> {
        &self.reconnect
    }

    /// Returns the output-health observation for this tick.
    #[must_use = "inspect the output-health result"]
    pub const fn output(&self) -> &Result<Option<OutputHealth>, DesktopControlOwnerError> {
        &self.output
    }

    /// Splits out the regular tick and reconnect result after inspecting output.
    #[must_use = "handle both scheduler results"]
    pub fn into_parts(
        self,
    ) -> (
        DesktopReceiverSchedulerTick<DecoderError>,
        Result<ReconnectGraceObservation, DesktopControlOwnerError>,
    ) {
        (
            DesktopReceiverSchedulerTick {
                control: self.control,
                media: self.media,
                pump: self.pump,
            },
            self.reconnect,
        )
    }
}

/// Owns one receiver and optionally one trusted framed control channel.
pub struct DesktopReceiverScheduler<
    O,
    P,
    const SLOTS: usize,
    const OUTPUT_SAMPLES_PER_BLOCK: usize,
    const MAX_SOURCES: usize,
    const PCM_SAMPLES_PER_BLOCK: usize,
    S,
> where
    O: AudioOutput,
    P: ReceiverPcmProducerInstaller<PCM_SAMPLES_PER_BLOCK>,
    S: Read + Write,
{
    receiver: DesktopReceiverOwner<
        O,
        P,
        SLOTS,
        OUTPUT_SAMPLES_PER_BLOCK,
        MAX_SOURCES,
        PCM_SAMPLES_PER_BLOCK,
    >,
    control: Option<(ControlSessionToken, UntrustedControlStream<S>)>,
    scheduled_clock_ms: Option<u64>,
    scheduled_playout: Option<(crate::StreamStartRequest, u128, u128)>,
    scheduled_drain: Option<crate::StreamStartRequest>,
    scheduled_concealment: u8,
    scheduled_startup_frames: u16,
}

impl<
    O,
    P,
    const SLOTS: usize,
    const OUTPUT_SAMPLES_PER_BLOCK: usize,
    const MAX_SOURCES: usize,
    const PCM_SAMPLES_PER_BLOCK: usize,
    S,
>
    DesktopReceiverScheduler<
        O,
        P,
        SLOTS,
        OUTPUT_SAMPLES_PER_BLOCK,
        MAX_SOURCES,
        PCM_SAMPLES_PER_BLOCK,
        S,
    >
where
    O: AudioOutput,
    P: ReceiverPcmProducerInstaller<PCM_SAMPLES_PER_BLOCK>,
    S: Read + Write,
{
    /// Creates a scheduler with no attached control channel.
    #[must_use]
    pub fn new(
        receiver: DesktopReceiverOwner<
            O,
            P,
            SLOTS,
            OUTPUT_SAMPLES_PER_BLOCK,
            MAX_SOURCES,
            PCM_SAMPLES_PER_BLOCK,
        >,
    ) -> Self {
        Self {
            receiver,
            control: None,
            scheduled_clock_ms: None,
            scheduled_playout: None,
            scheduled_drain: None,
            scheduled_concealment: 0,
            scheduled_startup_frames: 0,
        }
    }

    /// Borrows the composed receiver owner.
    #[must_use]
    pub const fn receiver(
        &self,
    ) -> &DesktopReceiverOwner<
        O,
        P,
        SLOTS,
        OUTPUT_SAMPLES_PER_BLOCK,
        MAX_SOURCES,
        PCM_SAMPLES_PER_BLOCK,
    > {
        &self.receiver
    }

    /// Mutably borrows the composed receiver owner.
    pub fn receiver_mut(
        &mut self,
    ) -> &mut DesktopReceiverOwner<
        O,
        P,
        SLOTS,
        OUTPUT_SAMPLES_PER_BLOCK,
        MAX_SOURCES,
        PCM_SAMPLES_PER_BLOCK,
    > {
        &mut self.receiver
    }

    /// Attaches one already-authenticated control channel.
    ///
    /// # Errors
    ///
    /// Returns [`DesktopReceiverSchedulerError::ControlAlreadyAttached`] when
    /// a previous channel has not been detached or closed.
    pub fn attach_control(
        &mut self,
        session: ControlSessionToken,
        channel: S,
    ) -> Result<(), DesktopReceiverSchedulerError> {
        if self.control.is_some() {
            return Err(DesktopReceiverSchedulerError::ControlAlreadyAttached);
        }
        self.control = Some((session, UntrustedControlStream::from_io(channel)));
        Ok(())
    }

    /// Detaches the current channel, discarding any partial frame or reply.
    pub fn detach_control(&mut self) -> Option<(ControlSessionToken, S)> {
        self.control
            .take()
            .map(|(session, framed)| (session, framed.into_inner()))
    }

    /// Retires the control transport and starts a bounded reconnect window.
    ///
    /// Call this after [`Self::poll_control_once`] reports `Closed` or an
    /// unrecoverable transport error. The media pipeline remains owned by the
    /// scheduler while the control owner moves to `Reconnecting`.
    ///
    /// # Errors
    ///
    /// Returns a lifecycle error when the owner cannot transition from its
    /// current state or arm the requested grace period.
    pub fn mark_control_lost(
        &mut self,
        now_ms: u64,
        grace_duration_ms: u64,
    ) -> Result<(), DesktopControlOwnerError> {
        self.control.take();
        let owner = self.receiver.owner_mut();
        owner.acknowledge_platform_event(SessionEvent::ConnectionLost)?;
        owner.begin_reconnect_grace(now_ms, grace_duration_ms)
    }

    /// Installs a fresh control session after the receiver has reconnected.
    ///
    /// The old transport must have been retired with [`Self::mark_control_lost`].
    /// This method returns the newly issued token so the caller can retain it
    /// for diagnostics or correlation, while the scheduler owns the channel.
    ///
    /// # Errors
    ///
    /// Returns an attachment error when a channel is still present, or a
    /// lifecycle error when the owner cannot re-enter a fresh ready session.
    pub fn attach_reconnected_control(
        &mut self,
        channel: S,
    ) -> Result<ControlSessionToken, DesktopReceiverSchedulerReconnectError> {
        if self.control.is_some() {
            return Err(DesktopReceiverSchedulerReconnectError::ControlAlreadyAttached);
        }
        let owner = self.receiver.owner_mut();
        owner
            .acknowledge_platform_event(SessionEvent::Reconnected)
            .map_err(DesktopReceiverSchedulerReconnectError::Lifecycle)?;
        let session = owner
            .install_authenticated_control_session()
            .map_err(DesktopReceiverSchedulerReconnectError::Lifecycle)?;
        self.control = Some((session, UntrustedControlStream::from_io(channel)));
        Ok(session)
    }

    /// Polls one control envelope and preserves the channel for the next tick.
    ///
    /// A closed channel is removed automatically. A transport or reply error
    /// also retires the channel so the caller can establish a fresh session.
    ///
    /// # Errors
    ///
    /// Returns [`ControlSessionError`] when the attached session cannot progress.
    pub fn poll_control_once(&mut self) -> Result<Option<ControlSessionPoll>, ControlSessionError> {
        self.poll_control_once_inner(None)
    }

    /// Polls one control envelope with host preparation for a new stream.
    ///
    /// # Errors
    ///
    /// Returns a control transport or reply error; preparation failures are
    /// returned to the peer as a correlated control error.
    pub fn poll_control_once_with_prepare(
        &mut self,
        prepare: &mut impl FnMut(
            ControlSessionToken,
            StreamStartRequest,
            &ControlStreamCoordinator,
            &mut P,
        ) -> Result<(), DesktopControlOwnerError>,
    ) -> Result<Option<ControlSessionPoll>, ControlSessionError> {
        self.poll_control_once_inner(Some(prepare))
    }

    fn poll_control_once_inner(
        &mut self,
        prepare: Option<&mut MediaPrepare<'_, P>>,
    ) -> Result<Option<ControlSessionPoll>, ControlSessionError> {
        let Some((session, framed)) = self.control.take() else {
            return Ok(None);
        };
        let mut control = DesktopControlSession::from_framed(&mut self.receiver, session, framed);

        // Finish a previously queued reply before accepting another command.
        // This keeps a transiently full socket from turning into WritePending.
        match control.flush()? {
            UntrustedControlWrite::Complete => {}
            UntrustedControlWrite::Pending => {
                let framed = control.into_framed();
                self.control = Some((session, framed));
                return Ok(Some(ControlSessionPoll::Empty));
            }
            UntrustedControlWrite::Closed => return Ok(Some(ControlSessionPoll::Closed)),
        }

        let result = control.poll_once_inner(prepare);
        let framed = control.into_framed();
        match result {
            Ok(ControlSessionPoll::Closed) => Ok(Some(ControlSessionPoll::Closed)),
            Ok(poll) => {
                self.control = Some((session, framed));
                Ok(Some(poll))
            }
            Err(error) => Err(error),
        }
    }
}

#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io;
    use std::net::SocketAddr;
    use std::sync::Arc;

    use sonimesh_audio::{FrameDecoder, RealtimePcmProducer, realtime_pcm_queue};
    use sonimesh_diagnostics::StreamCounters;
    use sonimesh_platform::{
        AudioOutput, OutputConfig, OutputDeviceId, OutputDeviceInfo, OutputError, OutputHealth,
        RenderSource, SampleFormat,
    };
    use sonimesh_protocol::{
        CONTROL_PROTOCOL_VERSION, Capability, Envelope, Hello, MediaDirection, MediaKeyContext,
        SessionSecret, StartStream, decode_control_frame, encode_control_frame, envelope,
    };

    use super::{DesktopReceiverScheduler, DesktopReceiverSchedulerError};
    use crate::{
        ControlToMixerBridge, DesktopControlOwner, DesktopReceiverOwner, OutputSelection,
        ReceiverPcmProducerInstaller, SessionState,
    };

    const SLOTS: usize = 1;
    const OUTPUT_SAMPLES: usize = 256;
    const SOURCE_SAMPLES: usize = 480;

    struct TestOutput {
        health: OutputHealth,
        source: Option<Box<dyn RenderSource>>,
    }

    impl Default for TestOutput {
        fn default() -> Self {
            Self {
                health: OutputHealth::Healthy,
                source: None,
            }
        }
    }

    impl AudioOutput for TestOutput {
        fn devices(&self) -> Result<Vec<OutputDeviceInfo>, OutputError> {
            Ok(Vec::new())
        }

        fn start(
            &mut self,
            _device: Option<&OutputDeviceId>,
            _config: OutputConfig,
            source: Box<dyn RenderSource>,
        ) -> Result<(), OutputError> {
            self.source = Some(source);
            Ok(())
        }

        fn stop(&mut self) {}

        fn health(&self) -> OutputHealth {
            self.health
        }
    }

    #[derive(Default)]
    struct TestPipeline;

    impl ReceiverPcmProducerInstaller<SOURCE_SAMPLES> for TestPipeline {
        fn replace_pcm_producer(&mut self, _producer: RealtimePcmProducer<SOURCE_SAMPLES>) {}
    }

    struct ByteDecoder;

    impl FrameDecoder for ByteDecoder {
        type Error = std::convert::Infallible;

        fn decode(&mut self, _encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
            output.fill(0.25);
            Ok(output.len())
        }

        fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
            output.fill(0.0);
            Ok(output.len())
        }
    }

    struct IdleIo;

    impl io::Read for IdleIo {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::ErrorKind::WouldBlock.into())
        }
    }

    impl io::Write for IdleIo {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct PartialWriteIo {
        input: VecDeque<u8>,
        writes: usize,
        output: Vec<u8>,
    }

    impl io::Read for PartialWriteIo {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.input.is_empty() {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let count = buffer.len().min(self.input.len());
            for slot in &mut buffer[..count] {
                *slot = self.input.pop_front().expect("count is bounded by input");
            }
            Ok(count)
        }
    }

    impl io::Write for PartialWriteIo {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            if self.writes == 2 {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let count = if self.writes == 1 {
                1.min(buffer.len())
            } else {
                buffer.len()
            };
            self.output.extend_from_slice(&buffer[..count]);
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn receiver()
    -> DesktopReceiverOwner<TestOutput, TestPipeline, SLOTS, OUTPUT_SAMPLES, 1, SOURCE_SAMPLES>
    {
        let bridge = ControlToMixerBridge::new(
            SessionState::READY,
            &[Capability::FrameDuration10Ms],
            TestOutput::default(),
            OutputSelection {
                device: None,
                config: OutputConfig {
                    sample_rate_hz: 48_000,
                    channels: 2,
                    sample_format: SampleFormat::Float32,
                    requested_callback_frames: Some(480),
                    maximum_callback_frames: 1_024,
                },
            },
            8,
            8,
        )
        .unwrap();
        DesktopReceiverOwner::new(
            DesktopControlOwner::new(bridge).unwrap(),
            TestPipeline,
            None,
        )
    }

    fn scheduler() -> DesktopReceiverScheduler<
        TestOutput,
        TestPipeline,
        SLOTS,
        OUTPUT_SAMPLES,
        1,
        SOURCE_SAMPLES,
        IdleIo,
    > {
        DesktopReceiverScheduler::new(receiver())
    }

    #[test]
    fn scheduler_preserves_an_idle_control_channel_until_detached() {
        let mut scheduler = scheduler();
        let session = scheduler
            .receiver_mut()
            .owner_mut()
            .install_authenticated_control_session()
            .unwrap();

        scheduler.attach_control(session, IdleIo).unwrap();
        assert_eq!(
            scheduler.attach_control(session, IdleIo),
            Err(DesktopReceiverSchedulerError::ControlAlreadyAttached)
        );
        assert!(matches!(
            scheduler.poll_control_once().unwrap(),
            Some(crate::ControlSessionPoll::Empty)
        ));
        assert!(scheduler.detach_control().is_some());
        assert_eq!(scheduler.poll_control_once().unwrap(), None);
    }

    #[test]
    fn scheduler_flushes_a_partial_reply_before_polling_the_next_command() {
        let mut scheduler = DesktopReceiverScheduler::new(receiver());
        let session = scheduler
            .receiver_mut()
            .owner_mut()
            .install_authenticated_control_session()
            .unwrap();

        let hello = Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 1,
            payload: Some(envelope::Payload::Hello(Hello {
                supported_versions: vec![CONTROL_PROTOCOL_VERSION],
                capabilities: vec![Capability::FrameDuration10Ms as i32],
                display_name: "test sender".to_owned(),
            })),
        };
        let start = Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 2,
            payload: Some(envelope::Payload::StartStream(StartStream {
                media_session_id: 7,
                stream_id: 1,
                sample_rate_hz: 48_000,
                channels: 1,
                frame_duration_ms: 10,
                media_key_epoch: 0,
            })),
        };
        let mut input = VecDeque::new();
        input.extend(encode_control_frame(&hello).unwrap());
        input.extend(encode_control_frame(&start).unwrap());
        scheduler
            .attach_control(
                session,
                PartialWriteIo {
                    input,
                    writes: 0,
                    output: Vec::new(),
                },
            )
            .unwrap();
        assert!(matches!(
            scheduler.poll_control_once().unwrap(),
            Some(crate::ControlSessionPoll::Processed {
                replied: true,
                write: Some(sonimesh_network::UntrustedControlWrite::Pending),
            })
        ));
        assert!(matches!(
            scheduler.poll_control_once().unwrap(),
            Some(crate::ControlSessionPoll::Empty)
        ));
        assert!(matches!(
            scheduler.poll_control_once().unwrap(),
            Some(crate::ControlSessionPoll::Processed {
                replied: true,
                write: Some(sonimesh_network::UntrustedControlWrite::Complete),
            })
        ));
        assert!(matches!(
            scheduler.poll_control_once().unwrap(),
            Some(crate::ControlSessionPoll::Empty)
        ));
        let (_, channel) = scheduler.detach_control().unwrap();
        let hello_len = u32::from_be_bytes(channel.output[..4].try_into().unwrap()) as usize + 4;
        let hello_reply = decode_control_frame(&channel.output[..hello_len]).unwrap();
        assert_eq!(hello_reply.request_id, 1);
        assert!(matches!(
            hello_reply.payload,
            Some(envelope::Payload::Hello(_))
        ));
        let reply = decode_control_frame(&channel.output[hello_len..]).unwrap();
        assert_eq!(reply.request_id, 2);
        assert!(matches!(
            reply.payload,
            Some(envelope::Payload::StreamStatus(_))
        ));
    }

    #[test]
    fn scheduler_preserves_fragmented_control_frame_across_polls() {
        struct FragmentedIo(VecDeque<u8>);
        impl io::Read for FragmentedIo {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                match self.0.pop_front() {
                    Some(byte) => {
                        buffer[0] = byte;
                        Ok(1)
                    }
                    None => Err(io::ErrorKind::WouldBlock.into()),
                }
            }
        }
        impl io::Write for FragmentedIo {
            fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
                Ok(buffer.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let hello = Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 1,
            payload: Some(envelope::Payload::Hello(Hello {
                supported_versions: vec![CONTROL_PROTOCOL_VERSION],
                capabilities: vec![Capability::FrameDuration10Ms as i32],
                display_name: "test sender".to_owned(),
            })),
        };
        let bytes = encode_control_frame(&hello).unwrap();
        let mut scheduler = DesktopReceiverScheduler::new(receiver());
        let session = scheduler
            .receiver_mut()
            .owner_mut()
            .install_authenticated_control_session()
            .unwrap();
        scheduler
            .attach_control(session, FragmentedIo(bytes.clone().into()))
            .unwrap();
        let mut processed = false;
        for _ in 0..=bytes.len() {
            match scheduler.poll_control_once().unwrap() {
                Some(crate::ControlSessionPoll::Processed { .. }) => {
                    processed = true;
                    break;
                }
                Some(crate::ControlSessionPoll::Empty) => {}
                other => panic!("unexpected poll: {other:?}"),
            }
        }
        assert!(processed, "fragmented hello must eventually be processed");
    }

    #[test]
    fn scheduler_marks_control_loss_and_arms_reconnect_grace() {
        let mut scheduler = scheduler();
        let session = scheduler
            .receiver_mut()
            .owner_mut()
            .install_authenticated_control_session()
            .unwrap();
        scheduler.attach_control(session, IdleIo).unwrap();

        scheduler.mark_control_lost(1_000, 30_000).unwrap();

        let owner = scheduler.receiver_mut().owner_mut();
        assert_eq!(
            owner.bridge().state().phase(),
            crate::SessionPhase::Reconnecting
        );
        assert!(matches!(
            owner.observe_reconnect_grace(1_000).unwrap(),
            crate::ReconnectGraceObservation::InGrace {
                remaining_ms: 30_000
            }
        ));
        assert!(scheduler.detach_control().is_none());
    }

    #[test]
    fn scheduler_reconnects_with_a_fresh_token_and_channel() {
        let mut scheduler = scheduler();
        let session = scheduler
            .receiver_mut()
            .owner_mut()
            .install_authenticated_control_session()
            .unwrap();
        scheduler.attach_control(session, IdleIo).unwrap();
        scheduler.mark_control_lost(1_000, 30_000).unwrap();

        let fresh = scheduler.attach_reconnected_control(IdleIo).unwrap();
        assert_ne!(fresh, session);
        assert_eq!(
            scheduler.receiver().owner().bridge().state().phase(),
            crate::SessionPhase::Ready
        );
        assert!(matches!(
            scheduler.poll_control_once().unwrap(),
            Some(crate::ControlSessionPoll::Empty)
        ));
    }

    #[test]
    #[allow(clippy::too_many_lines)] // Exercises control, UDP, decode, and output together.
    fn scheduler_drives_authenticated_udp_audio_to_the_output_source() {
        use sonimesh_audio::{JitterMode, JitterTargetConfig, TargetGatedPlayoutConfig};
        use sonimesh_network::MediaSessionSender;

        type Pipeline = crate::UdpReceiverPipeline<ByteDecoder, 8, 256, SOURCE_SAMPLES>;
        let (producer, _) = realtime_pcm_queue::<SOURCE_SAMPLES>(8).unwrap();
        let secret = SessionSecret::new([7; 32]);
        let context = MediaKeyContext::new(7, 1, 0, MediaDirection::AndroidToDesktop).unwrap();
        let counters = Arc::new(StreamCounters::default());
        let mut pipeline = Pipeline::bind_with_adaptive_target(
            "127.0.0.1:0".parse().unwrap(),
            &secret,
            context,
            ByteDecoder,
            producer,
            Arc::clone(&counters),
            0,
            crate::AdaptiveTargetConfig {
                target: JitterTargetConfig::preset(JitterMode::Balanced, 10, 8),
                arrival_ticks_per_second: 1_000,
                tick_source: Box::new(|| 0),
            },
        )
        .unwrap();
        pipeline
            .enable_target_gated_playout(TargetGatedPlayoutConfig { max_gate_ticks: 2 })
            .unwrap();
        let address = pipeline.local_addr().unwrap();
        let bridge = ControlToMixerBridge::new(
            SessionState::READY,
            &[Capability::FrameDuration10Ms],
            TestOutput::default(),
            OutputSelection {
                device: None,
                config: OutputConfig {
                    sample_rate_hz: 48_000,
                    channels: 2,
                    sample_format: SampleFormat::Float32,
                    requested_callback_frames: Some(480),
                    maximum_callback_frames: 1_024,
                },
            },
            8,
            8,
        )
        .unwrap();
        let owner = DesktopControlOwner::new(bridge).unwrap();
        let mut scheduler =
            DesktopReceiverScheduler::new(DesktopReceiverOwner::<
                _,
                _,
                SLOTS,
                OUTPUT_SAMPLES,
                1,
                SOURCE_SAMPLES,
            >::new(owner, pipeline, Some(counters)));
        let session = scheduler
            .receiver_mut()
            .owner_mut()
            .install_authenticated_control_session()
            .unwrap();
        let hello = Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 1,
            payload: Some(envelope::Payload::Hello(Hello {
                supported_versions: vec![CONTROL_PROTOCOL_VERSION],
                capabilities: vec![Capability::FrameDuration10Ms as i32],
                display_name: "test sender".to_owned(),
            })),
        };
        let start = Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 2,
            payload: Some(envelope::Payload::StartStream(StartStream {
                media_session_id: 7,
                stream_id: 1,
                sample_rate_hz: 48_000,
                channels: 1,
                frame_duration_ms: 10,
                media_key_epoch: 0,
            })),
        };
        let mut input = VecDeque::new();
        input.extend(encode_control_frame(&hello).unwrap());
        input.extend(encode_control_frame(&start).unwrap());
        scheduler
            .attach_control(
                session,
                PartialWriteIo {
                    input,
                    writes: 0,
                    output: Vec::new(),
                },
            )
            .unwrap();
        for _ in 0..3 {
            let tick = scheduler.tick();
            assert!(tick.control().is_ok());
            assert!(tick.media().is_ok());
            assert!(tick.pump().is_ok());
        }
        let mut sender =
            MediaSessionSender::bind("127.0.0.1:0".parse().unwrap(), &secret, context, address)
                .unwrap();
        sender.send_frame(0, 0, &[1]).unwrap();
        let mut decoded = false;
        for _ in 0..1_000 {
            let tick = scheduler.tick();
            assert!(tick.pump().is_ok());
            if matches!(tick.media(), Ok(media) if matches!(media.poll(), crate::UdpPollOutcome::Processed { .. }))
            {
                assert!(
                    matches!(tick.media(), Ok(media) if matches!(media.decode(), sonimesh_audio::DecodeStep::Decoded { .. }))
                );
                decoded = true;
                break;
            }
            std::thread::yield_now();
        }
        assert!(decoded, "loopback packet was not decoded");
        let output = scheduler
            .receiver_mut()
            .owner_mut()
            .bridge_mut()
            .mixer_mut()
            .output_mut();
        let mut samples = [0.0; OUTPUT_SAMPLES];
        output.source.as_mut().unwrap().render(&mut samples);
        assert!(
            samples.iter().any(|sample| *sample > 0.1),
            "decoded audio must reach the output"
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)] // Exercises timing, UDP, output recovery, and control lifecycle.
    fn scheduled_receiver_paces_playout_and_respects_stream_lifecycle() {
        use sonimesh_audio::DecodeStep;
        use sonimesh_network::MediaSessionSender;
        use sonimesh_protocol::StopStream;

        let (producer, _) = realtime_pcm_queue::<SOURCE_SAMPLES>(8).unwrap();
        let secret = SessionSecret::new([7; 32]);
        let context = MediaKeyContext::new(7, 1, 0, MediaDirection::AndroidToDesktop).unwrap();
        let counters = Arc::new(StreamCounters::default());
        let pipeline = crate::UdpReceiverPipeline::<ByteDecoder, 32, 256, SOURCE_SAMPLES>::bind(
            "127.0.0.1:0".parse().unwrap(),
            &secret,
            context,
            ByteDecoder,
            producer,
            Arc::clone(&counters),
            0,
        )
        .unwrap();
        let address = pipeline.local_addr().unwrap();
        let bridge = ControlToMixerBridge::new(
            SessionState::READY,
            &[Capability::FrameDuration10Ms],
            TestOutput::default(),
            OutputSelection {
                device: None,
                config: OutputConfig {
                    sample_rate_hz: 48_000,
                    channels: 2,
                    sample_format: SampleFormat::Float32,
                    requested_callback_frames: Some(480),
                    maximum_callback_frames: 1_024,
                },
            },
            8,
            8,
        )
        .unwrap();
        let owner = DesktopControlOwner::new(bridge).unwrap();
        let mut scheduler: DesktopReceiverScheduler<
            _,
            _,
            SLOTS,
            OUTPUT_SAMPLES,
            1,
            SOURCE_SAMPLES,
            IdleIo,
        > = DesktopReceiverScheduler::new(DesktopReceiverOwner::new(
            owner,
            pipeline,
            Some(counters),
        ));
        let session = scheduler
            .receiver_mut()
            .owner_mut()
            .install_authenticated_control_session()
            .unwrap();
        let hello = Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 1,
            payload: Some(envelope::Payload::Hello(Hello {
                supported_versions: vec![CONTROL_PROTOCOL_VERSION],
                capabilities: vec![Capability::FrameDuration10Ms as i32],
                display_name: "test sender".to_owned(),
            })),
        };
        let start = Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 2,
            payload: Some(envelope::Payload::StartStream(StartStream {
                media_session_id: 7,
                stream_id: 1,
                sample_rate_hz: 48_000,
                channels: 1,
                frame_duration_ms: 10,
                media_key_epoch: 0,
            })),
        };
        scheduler
            .receiver_mut()
            .accept_and_execute(session, &hello)
            .unwrap();
        let idle = scheduler.tick_scheduled_at(90);
        assert_eq!(idle.state, super::ScheduledReceiverState::Idle);
        assert!(matches!(idle.playout, Ok(None)));
        scheduler
            .receiver_mut()
            .accept_and_execute(session, &start)
            .unwrap();

        let mut sender =
            MediaSessionSender::bind("127.0.0.1:0".parse().unwrap(), &secret, context, address)
                .unwrap();
        for timestamp in [0, 480, 960] {
            sender.send_frame(0, timestamp, &[1]).unwrap();
        }
        let tick = scheduler.tick_scheduled_at(100);
        assert_eq!(tick.state, super::ScheduledReceiverState::Active);
        assert_eq!(tick.packets.unwrap(), 3);
        assert!(matches!(
            tick.playout,
            Ok(Some(DecodeStep::Decoded { sequence: 0, .. }))
        ));
        assert!(
            tick.control.is_ok()
                && tick.output.is_ok()
                && tick.reconnect.is_ok()
                && tick.pump.is_ok()
        );
        for now in [100, 101, 109] {
            assert!(matches!(scheduler.tick_scheduled_at(now).playout, Ok(None)));
        }
        assert!(matches!(
            scheduler.tick_scheduled_at(110).playout,
            Ok(Some(DecodeStep::Decoded { sequence: 1, .. }))
        ));
        assert!(matches!(
            scheduler.tick_scheduled_at(159).playout,
            Ok(Some(DecodeStep::Decoded { sequence: 2, .. }))
        ));
        assert!(matches!(scheduler.tick_scheduled_at(159).playout, Ok(None)));
        assert!(matches!(
            scheduler.tick_scheduled_at(158).playout,
            Err(super::ScheduledPlayoutError::ClockRegression {
                previous_ms: 159,
                now_ms: 158
            })
        ));
        assert!(matches!(scheduler.tick_scheduled_at(160).playout, Ok(None)));
        assert!(matches!(scheduler.tick_scheduled_at(163).playout, Ok(None)));
        assert!(matches!(
            scheduler.tick_scheduled_at(164).playout,
            Ok(Some(DecodeStep::Concealed { sequence: 3, .. }))
        ));

        let output = scheduler
            .receiver_mut()
            .owner_mut()
            .bridge_mut()
            .mixer_mut()
            .output_mut();
        let mut samples = [0.0; OUTPUT_SAMPLES];
        output.source.as_mut().unwrap().render(&mut samples);
        assert!(samples.iter().any(|sample| *sample > 0.1));

        // A busy socket is drained in bounded batches even between deadlines.
        for index in 4..24 {
            sender.send_frame(0, index * 480, &[1]).unwrap();
        }
        assert_eq!(scheduler.tick_scheduled_at(165).packets.unwrap(), 16);
        let tick = scheduler.tick_scheduled_at(165);
        assert_eq!(tick.packets.unwrap(), 4);
        assert!(matches!(tick.playout, Ok(None)));

        scheduler
            .receiver_mut()
            .owner_mut()
            .bridge_mut()
            .mixer_mut()
            .output_mut()
            .health = OutputHealth::StreamError;
        let fault = scheduler.tick_scheduled_at(170);
        assert_eq!(
            fault.state,
            super::ScheduledReceiverState::OutputUnavailable
        );
        assert!(matches!(fault.output, Ok(Some(OutputHealth::StreamError))));
        assert!(matches!(fault.playout, Ok(None)));
        for index in 24..42 {
            sender.send_frame(0, index * 480, &[1]).unwrap();
        }
        scheduler
            .receiver_mut()
            .owner_mut()
            .bridge_mut()
            .mixer_mut()
            .output_mut()
            .health = OutputHealth::Healthy;
        let identities = scheduler
            .receiver_mut()
            .owner_mut()
            .bridge_mut()
            .recover_output()
            .unwrap();
        let awaiting = scheduler.tick_scheduled_at(171);
        assert_eq!(
            awaiting.state,
            super::ScheduledReceiverState::AwaitingStreamRecovery
        );
        assert!(
            matches!(awaiting.playout, Ok(None)),
            "recovered output must wait for stream re-admission"
        );
        scheduler
            .receiver_mut()
            .recover_stream(identities[0])
            .unwrap();
        let drained = scheduler.tick_scheduled_at(171);
        assert_eq!(
            drained.state,
            super::ScheduledReceiverState::DrainingStaleMedia
        );
        assert_eq!(drained.packets.unwrap(), 16);
        assert!(matches!(drained.playout, Ok(None)));
        let drained = scheduler.tick_scheduled_at(171);
        assert_eq!(
            drained.state,
            super::ScheduledReceiverState::DrainingStaleMedia
        );
        assert_eq!(drained.packets.unwrap(), 2);
        assert!(matches!(drained.playout, Ok(None)));
        sender.send_frame(0, 42 * 480, &[1]).unwrap();
        assert!(matches!(
            scheduler.tick_scheduled_at(171).playout,
            Ok(Some(DecodeStep::Decoded { sequence: 41, .. }))
        ));
        // Alternating one-millisecond wakeup jitter must not shift the frame
        // clock permanently after every late call.
        for frame in 1..=100_u64 {
            let now = 171 + frame * 10 + frame % 2;
            sender.send_frame(0, (42 + frame) * 480, &[1]).unwrap();
            assert!(matches!(
                scheduler.tick_scheduled_at(now).playout,
                Ok(Some(_))
            ));
        }

        // A long scheduling pause retires queued audio and drains the socket
        // across bounded calls before admitting fresh playout.
        for index in 143..161 {
            sender.send_frame(0, index * 480, &[1]).unwrap();
        }
        let stalled = scheduler.tick_scheduled_at(1271);
        assert_eq!(
            stalled.state,
            super::ScheduledReceiverState::DrainingStaleMedia
        );
        assert_eq!(stalled.packets.unwrap(), 16);
        assert!(matches!(stalled.playout, Ok(None)));
        assert!(stalled.pump.is_ok());
        let identity = crate::MediaStreamIdentity {
            session_id: 7,
            stream_id: 1,
            key_epoch: 0,
        };
        assert_eq!(
            scheduler
                .receiver()
                .owner()
                .bridge()
                .mixer()
                .queued_output_blocks(identity)
                .unwrap(),
            0
        );
        let drained = scheduler.tick_scheduled_at(1200);
        assert_eq!(
            drained.state,
            super::ScheduledReceiverState::DrainingStaleMedia
        );
        assert_eq!(drained.packets.unwrap(), 2);
        assert!(matches!(
            drained.playout,
            Err(super::ScheduledPlayoutError::ClockRegression { .. })
        ));
        let invalid_sender = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        for _ in 0..2 {
            invalid_sender.send_to(&[0], address).unwrap();
        }
        sender.send_frame(0, 161 * 480, &[1]).unwrap();
        let rejected = scheduler.tick_scheduled_at(1201);
        assert!(rejected.packets.is_err());
        assert!(matches!(
            rejected.playout,
            Err(super::ScheduledPlayoutError::ClockRegression { .. })
        ));
        scheduler
            .receiver_mut()
            .owner_mut()
            .bridge_mut()
            .mixer_mut()
            .output_mut()
            .health = OutputHealth::StreamError;
        assert_eq!(
            scheduler.tick_scheduled_at(1271).state,
            super::ScheduledReceiverState::OutputUnavailable
        );
        sender.send_frame(0, 162 * 480, &[1]).unwrap();
        scheduler
            .receiver_mut()
            .owner_mut()
            .bridge_mut()
            .mixer_mut()
            .output_mut()
            .health = OutputHealth::Healthy;
        let identities = scheduler
            .receiver_mut()
            .owner_mut()
            .bridge_mut()
            .recover_output()
            .unwrap();
        scheduler
            .receiver_mut()
            .recover_stream(identities[0])
            .unwrap();
        let resumed = scheduler.tick_scheduled_at(1271);
        assert_eq!(
            resumed.state,
            super::ScheduledReceiverState::DrainingStaleMedia
        );
        assert_eq!(resumed.packets.unwrap(), 1);
        assert!(matches!(resumed.playout, Ok(None)));
        sender.send_frame(0, 163 * 480, &[1]).unwrap();
        let fresh = scheduler.tick_scheduled_at(1272);
        assert_eq!(fresh.state, super::ScheduledReceiverState::Active);
        assert!(
            matches!(
                fresh.playout,
                Ok(Some(DecodeStep::Decoded { sequence: 162, .. }))
            ),
            "fresh playout: {:?}; packets: {:?}",
            fresh.playout,
            fresh.packets
        );

        let stop = Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 3,
            payload: Some(envelope::Payload::StopStream(StopStream {
                media_session_id: 7,
                stream_id: 1,
                media_key_epoch: 0,
            })),
        };
        scheduler
            .receiver_mut()
            .accept_and_execute(session, &stop)
            .unwrap();
        assert!(matches!(
            scheduler.tick_scheduled_at(1280).playout,
            Ok(None)
        ));
        let mut other_start = start.clone();
        if let Some(envelope::Payload::StartStream(start)) = &mut other_start.payload {
            start.stream_id = 2;
        }
        other_start.request_id = 4;
        let rejected = scheduler
            .receiver_mut()
            .accept_and_execute(session, &other_start);
        assert!(matches!(
            rejected,
            Err(crate::DesktopControlOwnerError::MediaContextMismatch)
        ));
        assert_eq!(
            scheduler.tick_scheduled_at(1281).state,
            super::ScheduledReceiverState::Idle
        );
        scheduler
            .receiver_mut()
            .accept_and_execute(session, &start)
            .unwrap();
        scheduler.set_scheduled_startup_frames(2).unwrap();
        assert!(scheduler.set_scheduled_startup_frames(32).is_err());
        assert!(matches!(
            scheduler.tick_scheduled_at(1282).playout,
            Ok(None)
        ));
        sender.send_frame(0, 164 * 480, &[1]).unwrap();
        let buffering = scheduler.tick_scheduled_at(1283);
        assert_eq!(buffering.packets.unwrap(), 1);
        assert!(matches!(buffering.playout, Ok(None)));
        scheduler.set_scheduled_startup_frames(1).unwrap();
        assert!(matches!(
            scheduler.tick_scheduled_at(1302).playout,
            Ok(None)
        ));
        assert!(matches!(
            scheduler.tick_scheduled_at(1303).playout,
            Ok(Some(DecodeStep::Decoded { .. }))
        ));
        for now in [1313, 1323, 1333] {
            assert!(matches!(
                scheduler.tick_scheduled_at(now).playout,
                Ok(Some(DecodeStep::Concealed { .. }))
            ));
        }
        assert!(matches!(
            scheduler.tick_scheduled_at(1343).playout,
            Ok(None)
        ));
        sender.send_frame(0, 46 * 480, &[1]).unwrap();
        assert!(matches!(
            scheduler.tick_scheduled_at(1344).playout,
            Ok(None)
        ));
        assert!(matches!(
            scheduler.tick_scheduled_at(1353).playout,
            Ok(None)
        ));
        assert!(matches!(
            scheduler.tick_scheduled_at(1354).playout,
            Ok(Some(DecodeStep::Decoded { .. }))
        ));
        scheduler
            .receiver_mut()
            .owner_mut()
            .bridge_mut()
            .mixer_mut()
            .output_mut()
            .health = OutputHealth::StreamError;
        assert_eq!(
            scheduler.tick_scheduled_at(1355).state,
            super::ScheduledReceiverState::OutputUnavailable
        );
        scheduler
            .receiver_mut()
            .owner_mut()
            .bridge_mut()
            .mixer_mut()
            .output_mut()
            .health = OutputHealth::Healthy;
        let identities = scheduler
            .receiver_mut()
            .owner_mut()
            .bridge_mut()
            .recover_output()
            .unwrap();
        scheduler
            .receiver_mut()
            .recover_stream(identities[0])
            .unwrap();
        assert!(matches!(
            scheduler.tick_scheduled_at(1356).playout,
            Ok(None)
        ));
        sender.send_frame(0, 47 * 480, &[1]).unwrap();
        assert!(matches!(
            scheduler.tick_scheduled_at(1357).playout,
            Ok(None)
        ));
        assert!(matches!(
            scheduler.tick_scheduled_at(1366).playout,
            Ok(None)
        ));
        assert!(matches!(
            scheduler.tick_scheduled_at(1367).playout,
            Ok(Some(DecodeStep::Decoded { .. }))
        ));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn combined_tick_reports_control_and_media_results_together() {
        const JITTER_CAPACITY: usize = 8;
        const MAX_ENCODED_BYTES: usize = 256;
        type UdpPipeline = crate::UdpReceiverPipeline<
            ByteDecoder,
            JITTER_CAPACITY,
            MAX_ENCODED_BYTES,
            SOURCE_SAMPLES,
        >;

        let (producer, _consumer) = realtime_pcm_queue::<SOURCE_SAMPLES>(8).unwrap();
        let secret = SessionSecret::new([7; 32]);
        let context = MediaKeyContext::new(7, 1, 0, MediaDirection::AndroidToDesktop).unwrap();
        let counters = Arc::new(StreamCounters::default());
        let pipeline = UdpPipeline::bind(
            SocketAddr::from(([127, 0, 0, 1], 0)),
            &secret,
            context,
            ByteDecoder,
            producer,
            Arc::clone(&counters),
            0,
        )
        .unwrap();
        let bridge = ControlToMixerBridge::new(
            SessionState::READY,
            &[Capability::FrameDuration10Ms],
            TestOutput::default(),
            OutputSelection {
                device: None,
                config: OutputConfig {
                    sample_rate_hz: 48_000,
                    channels: 2,
                    sample_format: SampleFormat::Float32,
                    requested_callback_frames: Some(480),
                    maximum_callback_frames: 1_024,
                },
            },
            8,
            8,
        )
        .unwrap();
        let owner = DesktopControlOwner::new(bridge).unwrap();
        let mut scheduler: DesktopReceiverScheduler<
            TestOutput,
            UdpPipeline,
            SLOTS,
            OUTPUT_SAMPLES,
            1,
            SOURCE_SAMPLES,
            IdleIo,
        > = DesktopReceiverScheduler::new(DesktopReceiverOwner::new(
            owner,
            pipeline,
            Some(counters),
        ));
        let session = scheduler
            .receiver_mut()
            .owner_mut()
            .install_authenticated_control_session()
            .unwrap();
        scheduler.attach_control(session, IdleIo).unwrap();

        let tick = scheduler.tick();
        assert!(matches!(
            tick.control(),
            Ok(Some(crate::ControlSessionPoll::Empty))
        ));
        assert!(matches!(
            tick.media(),
            Ok(media) if matches!(media.poll(), crate::UdpPollOutcome::Empty)
        ));

        let hello = Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 1,
            payload: Some(envelope::Payload::Hello(Hello {
                supported_versions: vec![CONTROL_PROTOCOL_VERSION],
                capabilities: vec![Capability::FrameDuration10Ms as i32],
                display_name: "test sender".to_owned(),
            })),
        };
        let start = Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 2,
            payload: Some(envelope::Payload::StartStream(StartStream {
                media_session_id: 7,
                stream_id: 1,
                sample_rate_hz: 48_000,
                channels: 1,
                frame_duration_ms: 10,
                media_key_epoch: 0,
            })),
        };
        scheduler
            .receiver_mut()
            .accept_and_execute(session, &hello)
            .unwrap();
        scheduler
            .receiver_mut()
            .accept_and_execute(session, &start)
            .unwrap();
        scheduler
            .receiver_mut()
            .owner_mut()
            .bridge_mut()
            .mixer_mut()
            .output_mut()
            .health = OutputHealth::StreamError;
        let maintenance = scheduler.tick_at(999);
        assert!(matches!(
            maintenance.output(),
            Ok(Some(OutputHealth::StreamError))
        ));
        assert!(maintenance.media().is_ok());
        assert_eq!(
            scheduler.receiver().owner().bridge().state().phase(),
            crate::SessionPhase::OutputUnavailable
        );

        let maintenance = scheduler.tick_at(1_000);
        assert!(matches!(maintenance.output(), Ok(None)));
    }
}

impl<
    O,
    D,
    const SLOTS: usize,
    const OUTPUT_SAMPLES_PER_BLOCK: usize,
    const MAX_SOURCES: usize,
    const JITTER_CAPACITY: usize,
    const MAX_ENCODED_BYTES: usize,
    const PCM_SAMPLES_PER_BLOCK: usize,
    S,
>
    DesktopReceiverScheduler<
        O,
        UdpReceiverPipeline<D, JITTER_CAPACITY, MAX_ENCODED_BYTES, PCM_SAMPLES_PER_BLOCK>,
        SLOTS,
        OUTPUT_SAMPLES_PER_BLOCK,
        MAX_SOURCES,
        PCM_SAMPLES_PER_BLOCK,
        S,
    >
where
    O: AudioOutput,
    D: FrameDecoder,
    S: Read + Write,
{
    /// Sets the fixed wait before a newly buffered stream starts scheduled playout.
    ///
    /// Zero retains immediate playout. Changes apply at the next playout epoch;
    /// they do not shift a deadline already in progress.
    ///
    /// # Errors
    ///
    /// Returns an error if a nonzero wait is not strictly below jitter capacity.
    pub fn set_scheduled_startup_frames(&mut self, frames: u16) -> Result<(), &'static str> {
        if frames != 0 && usize::from(frames) >= JITTER_CAPACITY {
            return Err("startup wait must be smaller than jitter capacity");
        }
        self.scheduled_startup_frames = frames;
        Ok(())
    }

    /// Runs one control and UDP media scheduler tick outside the output callback.
    ///
    /// Control, media and PCM conversion results are independent; inspect each
    /// result even when another side reports an error. Each call makes one
    /// decode/PLC decision; the caller must supply the stream's playout cadence.
    pub fn tick(&mut self) -> DesktopReceiverSchedulerTick<D::Error> {
        let control = self.poll_control_once();
        let media = self.receiver.tick_media();
        let pump = self
            .receiver
            .owner_mut()
            .bridge_mut()
            .mixer_mut()
            .pump_available();
        DesktopReceiverSchedulerTick {
            control,
            media,
            pump,
        }
    }

    /// Runs one control/media tick and observes reconnect grace and output health.
    ///
    /// Reconnect observation runs before control and media work, so expiry can
    /// retire output and pending stream state before the rest of the tick.
    /// Results remain independent: a clock or output error does not suppress
    /// media polling, and a transport error does not suppress grace observation.
    pub fn tick_at(&mut self, now_ms: u64) -> DesktopReceiverSchedulerMaintenanceTick<D::Error> {
        let reconnect = self.receiver.owner_mut().observe_reconnect_grace(now_ms);
        let output = self.receiver.owner_mut().monitor();
        let DesktopReceiverSchedulerTick {
            control,
            media,
            pump,
        } = self.tick();
        DesktopReceiverSchedulerMaintenanceTick {
            control,
            media,
            pump,
            reconnect,
            output,
        }
    }

    /// Services control and UDP independently from the stream's playout cadence.
    ///
    /// Call frequently with a monotonic millisecond clock. Up to
    /// [`MAX_SCHEDULED_UDP_POLLS`] datagrams are polled per call; at most one
    /// decode is attempted per call. Late calls skip missed deadlines, preserve
    /// the frame clock phase, and space decisions at least half a frame apart.
    /// Playout uses immediate drain and starts only for a matching active media
    /// identity admitted to the running output. Three concealed frames followed
    /// by an empty buffer retire the playout epoch at the next deadline so that
    /// a later packet can re-establish its sequence. Do not interleave this method
    /// with the unpaced `tick`, `tick_at`, or `tick_media` methods for a stream.
    /// All results must be inspected separately.
    #[allow(clippy::too_many_lines)] // Keeps the ordered recovery and scheduling decisions together.
    pub fn tick_scheduled_at(&mut self, now_ms: u64) -> ScheduledReceiverTick<D::Error> {
        self.tick_scheduled_at_inner(now_ms, None)
    }

    /// Runs a clock-driven tick with host preparation for newly accepted starts.
    ///
    /// The callback obeys [`DesktopReceiverOwner::accept_and_execute_with_prepare`].
    /// Every returned result must be inspected independently.
    #[allow(clippy::too_many_lines)] // Keeps the ordered recovery and scheduling decisions together.
    pub fn tick_scheduled_at_with_prepare(
        &mut self,
        now_ms: u64,
        prepare: &mut impl FnMut(
            ControlSessionToken,
            StreamStartRequest,
            &ControlStreamCoordinator,
            &mut UdpReceiverPipeline<D, JITTER_CAPACITY, MAX_ENCODED_BYTES, PCM_SAMPLES_PER_BLOCK>,
        ) -> Result<(), DesktopControlOwnerError>,
    ) -> ScheduledReceiverTick<D::Error> {
        self.tick_scheduled_at_inner(now_ms, Some(prepare))
    }

    #[allow(clippy::too_many_lines)]
    fn tick_scheduled_at_inner(
        &mut self,
        now_ms: u64,
        prepare: Option<
            &mut MediaPrepare<
                '_,
                UdpReceiverPipeline<D, JITTER_CAPACITY, MAX_ENCODED_BYTES, PCM_SAMPLES_PER_BLOCK>,
            >,
        >,
    ) -> ScheduledReceiverTick<D::Error> {
        let reconnect = self.receiver.owner_mut().observe_reconnect_grace(now_ms);
        let output = self.receiver.owner_mut().monitor();
        let control = self.poll_control_once_inner(prepare);

        let context = self.receiver.pipeline_mut().context();
        let identity = crate::MediaStreamIdentity {
            session_id: context.session_id,
            stream_id: context.stream_id,
            key_epoch: context.key_epoch,
        };
        let bridge = self.receiver.owner().bridge();
        let matching = bridge.coordinator().active_request_exact(identity);
        let mut state = if matching.is_none() {
            if bridge.coordinator().active().is_some() {
                ScheduledReceiverState::IdentityMismatch
            } else {
                ScheduledReceiverState::Idle
            }
        } else if !bridge.output_active() {
            ScheduledReceiverState::OutputUnavailable
        } else if !bridge.mixer().removal_requires_membership_command(identity) {
            ScheduledReceiverState::AwaitingStreamRecovery
        } else {
            ScheduledReceiverState::Active
        };
        let request = if state == ScheduledReceiverState::Active {
            matching
        } else {
            None
        };
        if request.is_none()
            || self
                .scheduled_playout
                .is_some_and(|(previous, _, _)| Some(previous) != request)
        {
            self.scheduled_playout = None;
            self.scheduled_concealment = 0;
        }
        if self.scheduled_drain.is_some() && self.scheduled_drain != matching {
            self.scheduled_drain = None;
        }
        if matches!(
            state,
            ScheduledReceiverState::OutputUnavailable
                | ScheduledReceiverState::AwaitingStreamRecovery
        ) && matching.is_some()
            && self.scheduled_drain != matching
        {
            self.receiver.pipeline_mut().hard_resynchronize();
            self.scheduled_drain = matching;
        }

        let clock_error = self
            .scheduled_clock_ms
            .filter(|previous| now_ms < *previous);
        if let Some(request) = request {
            let stalled = clock_error.is_none()
                && self
                    .scheduled_playout
                    .is_some_and(|(previous, _, _)| previous == request)
                && self.scheduled_clock_ms.is_some_and(|previous| {
                    now_ms - previous
                        >= MIN_SCHEDULED_STALL_MS
                            .max(u64::from(request.frame_duration_ms) * SCHEDULED_STALL_FRAMES)
                });
            if stalled && self.scheduled_drain != Some(request) {
                self.receiver.pipeline_mut().hard_resynchronize();
                self.scheduled_playout = None;
                self.scheduled_drain = Some(request);
                self.scheduled_concealment = 0;
            }
            if self
                .scheduled_drain
                .is_some_and(|draining| draining != request)
            {
                self.scheduled_drain = None;
            }
        }

        let draining = request.is_some() && self.scheduled_drain == request;
        let due = self
            .scheduled_playout
            .is_some_and(|(previous, deadline, earliest)| {
                Some(previous) == request && u128::from(now_ms) >= deadline.max(earliest)
            });
        let interrupted = request.is_some()
            && !draining
            && clock_error.is_none()
            && due
            && self.scheduled_concealment >= MAX_CONSECUTIVE_EMPTY_PLC
            && self.receiver.pipeline_mut().jitter_len() == 0;
        let mut reset = if draining || interrupted {
            if draining {
                state = ScheduledReceiverState::DrainingStaleMedia;
            }
            Some(
                self.receiver
                    .owner_mut()
                    .bridge_mut()
                    .mixer_mut()
                    .begin_epoch(identity),
            )
        } else {
            None
        };
        if interrupted && reset.as_ref().is_some_and(Result::is_ok) {
            self.receiver.pipeline_mut().hard_resynchronize();
            self.scheduled_playout = None;
            self.scheduled_concealment = 0;
        }

        let mut socket_empty = false;
        let packets = if request.is_some() {
            let mut count = 0;
            let mut result = Ok(());
            for _ in 0..MAX_SCHEDULED_UDP_POLLS {
                match self.receiver.poll_media() {
                    Ok(UdpPollOutcome::Empty) => {
                        socket_empty = true;
                        break;
                    }
                    Ok(UdpPollOutcome::Processed { .. }) => count += 1,
                    Err(error) => {
                        let retry = draining
                            && !matches!(error, UdpReceiverError::Socket(UdpReceiveError::Io(_)));
                        if result.is_ok() {
                            result = Err(error);
                        }
                        if !retry {
                            break;
                        }
                    }
                }
            }
            result.map(|()| count)
        } else {
            Ok(0)
        };

        if draining
            && clock_error.is_none()
            && socket_empty
            && reset.as_ref().is_some_and(Result::is_ok)
        {
            self.receiver.pipeline_mut().hard_resynchronize();
            self.scheduled_drain = None;
            self.scheduled_playout = None;
        }
        let playout = if let Some(previous_ms) = clock_error {
            Err(ScheduledPlayoutError::ClockRegression {
                previous_ms,
                now_ms,
            })
        } else {
            self.scheduled_clock_ms = Some(now_ms);
            if draining {
                Ok(None)
            } else if let Some(request) = request {
                let now = u128::from(now_ms);
                if self.scheduled_startup_frames > 0
                    && self.scheduled_playout.is_none()
                    && self.receiver.pipeline_mut().jitter_len() > 0
                {
                    let due = now
                        + u128::from(self.scheduled_startup_frames)
                            * u128::from(request.frame_duration_ms);
                    self.scheduled_playout = Some((request, due, due));
                }
                if self.scheduled_startup_frames > 0 && self.scheduled_playout.is_none() {
                    Ok(None)
                } else {
                    let (deadline, earliest) = self
                        .scheduled_playout
                        .filter(|(previous, _, _)| *previous == request)
                        .map_or((now, now), |(_, deadline, earliest)| (deadline, earliest));
                    if now < deadline || now < earliest {
                        Ok(None)
                    } else {
                        let interval = u128::from(request.frame_duration_ms);
                        let next = deadline + ((now - deadline) / interval + 1) * interval;
                        self.scheduled_playout = Some((request, next, now + interval / 2));
                        self.receiver
                            .decode_media(false)
                            .map(Some)
                            .map_err(ScheduledPlayoutError::Decode)
                    }
                }
            } else {
                self.scheduled_playout = None;
                Ok(None)
            }
        };
        match &playout {
            Ok(Some(DecodeStep::Concealed { .. })) => {
                self.scheduled_concealment = self.scheduled_concealment.saturating_add(1);
            }
            Ok(Some(DecodeStep::Decoded { .. } | DecodeStep::NotReady)) => {
                self.scheduled_concealment = 0;
            }
            Err(ScheduledPlayoutError::Decode(_)) => {
                self.scheduled_concealment = 0;
                if self.scheduled_startup_frames > 0 {
                    self.scheduled_playout = None;
                }
                reset = Some(
                    self.receiver
                        .owner_mut()
                        .bridge_mut()
                        .mixer_mut()
                        .begin_epoch(identity),
                );
            }
            Ok(None) | Err(ScheduledPlayoutError::ClockRegression { .. }) => {}
        }
        let pump_result = self
            .receiver
            .owner_mut()
            .bridge_mut()
            .mixer_mut()
            .pump_available_except(draining.then_some(identity));
        let pump = reset.unwrap_or(Ok(())).and(pump_result);
        ScheduledReceiverTick {
            state,
            control,
            packets,
            playout,
            pump,
            reconnect,
            output,
        }
    }

    /// Runs one UDP poll/decode scheduler tick outside the output callback.
    ///
    /// # Errors
    ///
    /// Returns the underlying UDP or decoder failure.
    pub fn tick_media(&mut self) -> Result<UdpMediaTick, UdpReceiverError<D::Error>> {
        self.receiver.tick_media()
    }
}

impl<
    O,
    P,
    const SLOTS: usize,
    const OUTPUT_SAMPLES_PER_BLOCK: usize,
    const MAX_SOURCES: usize,
    const PCM_SAMPLES_PER_BLOCK: usize,
    S,
>
    DesktopReceiverScheduler<
        O,
        P,
        SLOTS,
        OUTPUT_SAMPLES_PER_BLOCK,
        MAX_SOURCES,
        PCM_SAMPLES_PER_BLOCK,
        S,
    >
where
    O: AudioOutput,
    P: ReceiverPcmProducerInstaller<PCM_SAMPLES_PER_BLOCK>,
    S: Read + Write,
{
    /// Attaches a trusted TLS channel returned by the pairing listener.
    ///
    /// # Errors
    ///
    /// Returns [`DesktopReceiverSchedulerError::ControlAlreadyAttached`] when
    /// another channel is active, or `Socket` if non-blocking setup fails.
    pub fn attach_trusted_control(
        &mut self,
        session: ControlSessionToken,
        trusted: TrustedControlChannel,
    ) -> Result<(), DesktopReceiverSchedulerError>
    where
        S: From<PinnedServerChannel>,
    {
        if self.control.is_some() {
            return Err(DesktopReceiverSchedulerError::ControlAlreadyAttached);
        }
        trusted
            .set_nonblocking()
            .map_err(|error| DesktopReceiverSchedulerError::Socket(error.kind()))?;
        let (channel, _exporter) = trusted.into_parts();
        self.attach_control(session, S::from(channel))
    }

    /// Replaces a lost control channel with a freshly trusted channel.
    ///
    /// # Errors
    ///
    /// Returns an attachment or lifecycle error when the previous transport
    /// was not retired, non-blocking setup fails, or the owner cannot complete reconnection.
    pub fn attach_trusted_reconnected_control(
        &mut self,
        trusted: TrustedControlChannel,
    ) -> Result<ControlSessionToken, DesktopReceiverSchedulerReconnectError>
    where
        S: From<PinnedServerChannel>,
    {
        if self.control.is_some() {
            return Err(DesktopReceiverSchedulerReconnectError::ControlAlreadyAttached);
        }
        trusted
            .set_nonblocking()
            .map_err(|error| DesktopReceiverSchedulerReconnectError::Socket(error.kind()))?;
        let (channel, _exporter) = trusted.into_parts();
        self.attach_reconnected_control(S::from(channel))
    }
}
