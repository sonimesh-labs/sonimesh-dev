//! Shell-neutral owner of one desktop receiver scheduling loop.
//!
//! This boundary keeps receiver ownership, an optional framed control channel,
//! and low-rate media ticks together without introducing a UI or async runtime.
//! The caller still chooses the thread, timer, and platform output backend.

use std::io::{Read, Write};

use sonimesh_audio::FrameDecoder;
use sonimesh_network::PinnedServerChannel;
use sonimesh_network::{UntrustedControlStream, UntrustedControlWrite};
use sonimesh_platform::{AudioOutput, OutputHealth};

use crate::{
    ControlSessionError, ControlSessionPoll, ControlSessionToken, DesktopControlOwnerError,
    DesktopControlSession, DesktopReceiverOwner, ReceiverPcmProducerInstaller,
    ReconnectGraceObservation, SessionEvent, TrustedControlChannel, UdpMediaTick, UdpReceiverError,
    UdpReceiverPipeline,
};

/// Failure while attaching a control channel to a receiver scheduler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DesktopReceiverSchedulerError {
    /// A control channel is already attached to this scheduler.
    ControlAlreadyAttached,
}

impl core::fmt::Display for DesktopReceiverSchedulerError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ControlAlreadyAttached => {
                formatter.write_str("desktop receiver already has a control channel")
            }
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
        }
    }
}

impl std::error::Error for DesktopReceiverSchedulerReconnectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ControlAlreadyAttached => None,
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

        let result = control.poll_once();
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
        use sonimesh_network::MediaSessionSender;

        type Pipeline = crate::UdpReceiverPipeline<ByteDecoder, 8, 256, SOURCE_SAMPLES>;
        let (producer, _) = realtime_pcm_queue::<SOURCE_SAMPLES>(8).unwrap();
        let secret = SessionSecret::new([7; 32]);
        let context = MediaKeyContext::new(7, 1, 0, MediaDirection::AndroidToDesktop).unwrap();
        let counters = Arc::new(StreamCounters::default());
        let pipeline = Pipeline::bind(
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
    /// another channel is active.
    pub fn attach_trusted_control(
        &mut self,
        session: ControlSessionToken,
        trusted: TrustedControlChannel,
    ) -> Result<(), DesktopReceiverSchedulerError>
    where
        S: From<PinnedServerChannel>,
    {
        let (channel, _exporter) = trusted.into_parts();
        self.attach_control(session, S::from(channel))
    }

    /// Replaces a lost control channel with a freshly trusted channel.
    ///
    /// # Errors
    ///
    /// Returns an attachment or lifecycle error when the previous transport
    /// was not retired or the owner cannot complete reconnection.
    pub fn attach_trusted_reconnected_control(
        &mut self,
        trusted: TrustedControlChannel,
    ) -> Result<ControlSessionToken, DesktopReceiverSchedulerReconnectError>
    where
        S: From<PinnedServerChannel>,
    {
        let (channel, _exporter) = trusted.into_parts();
        self.attach_reconnected_control(S::from(channel))
    }
}
