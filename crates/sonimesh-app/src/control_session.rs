//! Authenticated control-session adapter for the desktop receiver.
//!
//! Pairing produces a trusted TLS channel, while this module owns the next
//! small boundary: bounded control framing, owner dispatch, and correlated
//! lifecycle replies. It deliberately does not own pairing, media polling, or
//! a UI runtime.

use crate::desktop_receiver_owner::MediaPrepare;
use std::io::{Read, Write};

use sonimesh_network::{
    PinnedServerChannel, UntrustedControlError, UntrustedControlPoll, UntrustedControlStream,
    UntrustedControlWrite,
};
use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, ControlFrameError, Envelope, ErrorCode, Hello, StreamState,
    StreamStatus, envelope,
};

use crate::{
    ControlReply, ControlSessionToken, ControlStreamAction, ControlStreamCoordinator,
    ControlToMixerError, DesktopControlOwnerError, DesktopReceiverOwner, MediaStreamIdentity,
    ReceiverPcmProducerInstaller, StreamStartRequest, TrustedControlChannel,
};
use sonimesh_platform::AudioOutput;

/// Result of one non-blocking control-session tick.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlSessionPoll {
    /// No complete control frame or writable socket capacity was available.
    Empty,
    /// One command was accepted and executed; a reply may have been queued.
    Processed {
        /// Whether the command produced a wire reply.
        replied: bool,
        /// Write progress for the optional reply.
        write: Option<UntrustedControlWrite>,
    },
    /// One valid command was rejected by the owner and an error reply was sent.
    Rejected {
        /// Whether the rejection reached the transport.
        replied: bool,
        /// Write progress for the rejection reply.
        write: Option<UntrustedControlWrite>,
    },
    /// The peer closed the authenticated channel.
    Closed,
}

impl<
    'a,
    O,
    P,
    const SLOTS: usize,
    const OUTPUT_SAMPLES_PER_BLOCK: usize,
    const MAX_SOURCES: usize,
    const PCM_SAMPLES_PER_BLOCK: usize,
>
    DesktopControlSession<
        'a,
        O,
        P,
        SLOTS,
        OUTPUT_SAMPLES_PER_BLOCK,
        MAX_SOURCES,
        PCM_SAMPLES_PER_BLOCK,
        PinnedServerChannel,
    >
where
    O: AudioOutput,
    P: ReceiverPcmProducerInstaller<PCM_SAMPLES_PER_BLOCK>,
{
    /// Builds a control session directly from a completed pairing exchange.
    ///
    /// The pairing exporter is no longer needed after proof verification and is
    /// dropped here through its zeroizing wrapper.
    ///
    /// Call [`TrustedControlChannel::set_nonblocking`] first when polling on a
    /// scheduling thread, or use [`Self::try_from_trusted_channel`].
    #[must_use]
    pub fn from_trusted_channel(
        receiver: &'a mut DesktopReceiverOwner<
            O,
            P,
            SLOTS,
            OUTPUT_SAMPLES_PER_BLOCK,
            MAX_SOURCES,
            PCM_SAMPLES_PER_BLOCK,
        >,
        session: ControlSessionToken,
        trusted: TrustedControlChannel,
    ) -> Self {
        let (channel, _exporter) = trusted.into_parts();
        Self::new(receiver, session, channel)
    }

    /// Builds a control session with a non-blocking trusted socket.
    ///
    /// # Errors
    ///
    /// Returns the socket error if non-blocking mode cannot be enabled.
    pub fn try_from_trusted_channel(
        receiver: &'a mut DesktopReceiverOwner<
            O,
            P,
            SLOTS,
            OUTPUT_SAMPLES_PER_BLOCK,
            MAX_SOURCES,
            PCM_SAMPLES_PER_BLOCK,
        >,
        session: ControlSessionToken,
        trusted: TrustedControlChannel,
    ) -> std::io::Result<Self> {
        trusted.set_nonblocking()?;
        Ok(Self::from_trusted_channel(receiver, session, trusted))
    }
}

/// Failure while polling, dispatching, or replying on one control session.
#[derive(Debug)]
pub enum ControlSessionError {
    /// The bounded transport rejected or could not progress a frame.
    Transport(UntrustedControlError),
    /// A generated reply could not satisfy the protocol contract.
    Reply(ControlFrameError),
}

impl core::fmt::Display for ControlSessionError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Transport(error) => {
                write!(formatter, "control session transport failed: {error}")
            }
            Self::Reply(error) => write!(formatter, "control session reply failed: {error}"),
        }
    }
}

impl std::error::Error for ControlSessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::Reply(error) => Some(error),
        }
    }
}

/// One trusted, framed control session attached to a desktop receiver owner.
pub struct DesktopControlSession<
    'a,
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
    receiver: &'a mut DesktopReceiverOwner<
        O,
        P,
        SLOTS,
        OUTPUT_SAMPLES_PER_BLOCK,
        MAX_SOURCES,
        PCM_SAMPLES_PER_BLOCK,
    >,
    session: ControlSessionToken,
    framed: UntrustedControlStream<S>,
}

impl<
    'a,
    O,
    P,
    const SLOTS: usize,
    const OUTPUT_SAMPLES_PER_BLOCK: usize,
    const MAX_SOURCES: usize,
    const PCM_SAMPLES_PER_BLOCK: usize,
    S,
>
    DesktopControlSession<
        'a,
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
    /// Attaches a trusted channel to an already-installed owner session.
    #[must_use]
    pub fn new(
        receiver: &'a mut DesktopReceiverOwner<
            O,
            P,
            SLOTS,
            OUTPUT_SAMPLES_PER_BLOCK,
            MAX_SOURCES,
            PCM_SAMPLES_PER_BLOCK,
        >,
        session: ControlSessionToken,
        channel: S,
    ) -> Self {
        Self::from_framed(receiver, session, UntrustedControlStream::from_io(channel))
    }

    /// Attaches an existing framing owner, preserving partial reads and writes.
    #[must_use]
    pub(crate) fn from_framed(
        receiver: &'a mut DesktopReceiverOwner<
            O,
            P,
            SLOTS,
            OUTPUT_SAMPLES_PER_BLOCK,
            MAX_SOURCES,
            PCM_SAMPLES_PER_BLOCK,
        >,
        session: ControlSessionToken,
        framed: UntrustedControlStream<S>,
    ) -> Self {
        Self {
            receiver,
            session,
            framed,
        }
    }

    /// Polls and processes at most one control envelope.
    ///
    /// Hello and lifecycle replies, including duplicate commands, are
    /// correlated with the request ID. Incoming replies are ignored.
    ///
    /// # Errors
    ///
    /// Returns [`ControlSessionError`] when framing, owner execution, or reply
    /// encoding cannot progress.
    pub fn poll_once(&mut self) -> Result<ControlSessionPoll, ControlSessionError> {
        self.poll_once_inner(None)
    }

    /// Polls one envelope with host preparation for a newly accepted Start.
    ///
    /// The callback follows [`DesktopReceiverOwner::accept_and_execute_with_prepare`]
    /// and runs outside the realtime output callback.
    ///
    /// # Errors
    ///
    /// Returns a framing or reply error; preparation errors produce a control reply.
    pub fn poll_once_with_prepare(
        &mut self,
        prepare: &mut impl FnMut(
            ControlSessionToken,
            StreamStartRequest,
            &ControlStreamCoordinator,
            &mut P,
        ) -> Result<(), DesktopControlOwnerError>,
    ) -> Result<ControlSessionPoll, ControlSessionError> {
        self.poll_once_inner(Some(prepare))
    }

    pub(crate) fn poll_once_inner(
        &mut self,
        prepare: Option<&mut MediaPrepare<'_, P>>,
    ) -> Result<ControlSessionPoll, ControlSessionError> {
        match self.framed.poll().map_err(ControlSessionError::Transport)? {
            UntrustedControlPoll::Empty => Ok(ControlSessionPoll::Empty),
            UntrustedControlPoll::Closed => Ok(ControlSessionPoll::Closed),
            UntrustedControlPoll::Frame(envelope) => self.process(&envelope, prepare),
        }
    }

    /// Flushes a previously queued reply without waiting for socket capacity.
    ///
    /// # Errors
    ///
    /// Returns [`ControlSessionError::Transport`] when the channel rejects the
    /// pending write.
    pub fn flush(&mut self) -> Result<UntrustedControlWrite, ControlSessionError> {
        self.framed
            .flush_write()
            .map_err(ControlSessionError::Transport)
    }

    /// Returns the underlying channel after the session loop is finished.
    #[must_use]
    pub fn into_inner(self) -> S {
        self.framed.into_inner()
    }

    /// Returns the framing owner, retaining any partial read or pending reply.
    #[must_use]
    pub(crate) fn into_framed(self) -> UntrustedControlStream<S> {
        self.framed
    }

    fn process(
        &mut self,
        envelope: &Envelope,
        prepare: Option<&mut MediaPrepare<'_, P>>,
    ) -> Result<ControlSessionPoll, ControlSessionError> {
        let request_id = envelope.request_id;
        match self
            .receiver
            .accept_and_execute_inner(self.session, envelope, prepare)
        {
            Ok(action) => {
                let reply = if matches!(action, ControlStreamAction::Noop) {
                    let bridge = self.receiver.owner().bridge();
                    let stream_ready = match envelope.payload.as_ref() {
                        Some(envelope::Payload::StartStream(start)) => {
                            u16::try_from(start.media_key_epoch)
                                .ok()
                                .is_some_and(|key_epoch| {
                                    bridge.state().phase() == crate::SessionPhase::Streaming
                                        && bridge.mixer().removal_requires_membership_command(
                                            MediaStreamIdentity {
                                                session_id: start.media_session_id,
                                                stream_id: start.stream_id,
                                                key_epoch,
                                            },
                                        )
                                })
                        }
                        _ => false,
                    };
                    reply_for_noop(
                        envelope,
                        bridge.coordinator().has_pending_change(),
                        stream_ready,
                    )
                } else {
                    reply_for_action(action, request_id)
                };
                let Some(reply) = reply.map_err(ControlSessionError::Reply)? else {
                    return Ok(ControlSessionPoll::Processed {
                        replied: false,
                        write: None,
                    });
                };
                let write = self
                    .framed
                    .send(&reply)
                    .map_err(ControlSessionError::Transport)?;
                Ok(ControlSessionPoll::Processed {
                    replied: true,
                    write: Some(write),
                })
            }
            Err(error) => {
                let reply =
                    owner_error_reply(request_id, &error).map_err(ControlSessionError::Reply)?;
                let write = self
                    .framed
                    .send(&reply)
                    .map_err(ControlSessionError::Transport)?;
                Ok(ControlSessionPoll::Rejected {
                    replied: true,
                    write: Some(write),
                })
            }
        }
    }
}

fn reply_for_noop(
    request: &Envelope,
    pending: bool,
    stream_ready: bool,
) -> Result<Option<Envelope>, ControlFrameError> {
    let (session_id, stream_id, state, detail) = match request.payload.as_ref() {
        Some(envelope::Payload::StartStream(start)) if !pending && stream_ready => (
            start.media_session_id,
            start.stream_id,
            StreamState::Streaming,
            "stream already started",
        ),
        Some(envelope::Payload::StopStream(stop)) if !pending => (
            stop.media_session_id,
            stop.stream_id,
            StreamState::Stopped,
            "stream already stopped",
        ),
        Some(envelope::Payload::StartStream(_) | envelope::Payload::StopStream(_)) => {
            return ControlReply::error(ErrorCode::RetryLater, true, "stream operation not ready")?
                .into_envelope(request.request_id)
                .map(Some);
        }
        _ => return Ok(None),
    };
    Ok(Some(Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: request.request_id,
        payload: Some(envelope::Payload::StreamStatus(StreamStatus {
            media_session_id: session_id,
            stream_id,
            state: state as i32,
            detail: detail.to_owned(),
        })),
    }))
}

fn reply_for_action(
    action: ControlStreamAction,
    request_id: u64,
) -> Result<Option<Envelope>, ControlFrameError> {
    let (identity, state, detail) = match action {
        ControlStreamAction::Negotiated(negotiated) => {
            return Ok(Some(Envelope {
                protocol_version: negotiated.version(),
                request_id,
                payload: Some(envelope::Payload::Hello(Hello {
                    supported_versions: vec![negotiated.version()],
                    capabilities: negotiated
                        .capabilities()
                        .iter()
                        .map(|cap| *cap as i32)
                        .collect(),
                    display_name: "SoniMesh".to_owned(),
                })),
            }));
        }
        ControlStreamAction::Start { request, .. } => {
            (request.identity, StreamState::Streaming, "stream started")
        }
        ControlStreamAction::Rotate { next, .. } => (
            next.identity,
            StreamState::Streaming,
            "stream key epoch rotated",
        ),
        ControlStreamAction::Stop { identity, .. } => {
            (identity, StreamState::Stopped, "stream stopped")
        }
        ControlStreamAction::Noop | ControlStreamAction::Ignored => return Ok(None),
    };
    ControlReply::stream_status(identity, state, detail)?
        .into_envelope(request_id)
        .map(Some)
}

fn owner_error_reply(
    request_id: u64,
    error: &DesktopControlOwnerError,
) -> Result<Envelope, ControlFrameError> {
    let envelope = match error {
        DesktopControlOwnerError::MediaContextMismatch => ControlReply::error(
            ErrorCode::RetryLater,
            true,
            "receiver media pipeline is not configured for the requested stream",
        )?
        .into_envelope(request_id)?,
        DesktopControlOwnerError::MediaFormatMismatch => ControlReply::error(
            ErrorCode::RetryLater,
            true,
            "receiver Opus format does not match the requested stream",
        )?
        .into_envelope(request_id)?,
        DesktopControlOwnerError::Mixer(ControlToMixerError::Control(control)) => {
            ControlReply::for_rejection(request_id, control)?
        }
        _ => ControlReply::error(
            ErrorCode::RetryLater,
            true,
            "desktop control operation failed",
        )?
        .into_envelope(request_id)?,
    };
    Ok(envelope)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io;

    use sonimesh_audio::RealtimePcmProducer;
    use sonimesh_platform::{
        AudioOutput, OutputConfig, OutputDeviceId, OutputDeviceInfo, OutputError, OutputHealth,
        RenderSource, SampleFormat,
    };
    use sonimesh_protocol::{
        CONTROL_PROTOCOL_VERSION, Capability, ControlError, Envelope, ErrorCode, Hello,
        StartStream, StopStream, StreamState, decode_control_frame, encode_control_frame, envelope,
    };

    use super::{ControlSessionPoll, DesktopControlSession};
    use crate::{
        ControlSessionToken, ControlToMixerBridge, DesktopControlOwner, DesktopReceiverOwner,
        OutputSelection, ReceiverPcmProducerInstaller, SessionEvent, SessionState,
    };
    use sonimesh_network::UntrustedControlWrite;

    const SLOTS: usize = 2;
    const OUTPUT_SAMPLES: usize = 256;
    const SOURCE_SAMPLES: usize = 480;

    #[derive(Default)]
    struct TestOutput;

    impl AudioOutput for TestOutput {
        fn devices(&self) -> Result<Vec<OutputDeviceInfo>, OutputError> {
            Ok(Vec::new())
        }

        fn start(
            &mut self,
            _device: Option<&OutputDeviceId>,
            _config: OutputConfig,
            _source: Box<dyn RenderSource>,
        ) -> Result<(), OutputError> {
            Ok(())
        }

        fn stop(&mut self) {}

        fn health(&self) -> OutputHealth {
            OutputHealth::Healthy
        }
    }

    #[derive(Default)]
    struct TestPipeline {
        replacements: usize,
        context: Option<sonimesh_protocol::MediaKeyContext>,
        opus_frame: Option<sonimesh_audio::OpusFrameConfig>,
    }

    impl<const N: usize> ReceiverPcmProducerInstaller<N> for TestPipeline {
        fn replace_pcm_producer(&mut self, _producer: RealtimePcmProducer<N>) {
            self.replacements += 1;
        }

        fn media_context(&self) -> Option<sonimesh_protocol::MediaKeyContext> {
            self.context
        }

        fn opus_frame_config(&self) -> Option<sonimesh_audio::OpusFrameConfig> {
            self.opus_frame
        }
    }

    struct MemoryIo {
        input: VecDeque<u8>,
        output: Vec<u8>,
    }

    impl MemoryIo {
        fn from_frames(frames: &[Envelope]) -> Self {
            let mut input = VecDeque::new();
            for frame in frames {
                input.extend(encode_control_frame(frame).unwrap());
            }
            Self {
                input,
                output: Vec::new(),
            }
        }
    }

    impl io::Read for MemoryIo {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let count = buffer.len().min(self.input.len());
            for slot in &mut buffer[..count] {
                *slot = self.input.pop_front().expect("count is bounded by input");
            }
            Ok(count)
        }
    }

    impl io::Write for MemoryIo {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.output.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn receiver()
    -> DesktopReceiverOwner<TestOutput, TestPipeline, SLOTS, OUTPUT_SAMPLES, 1, SOURCE_SAMPLES>
    {
        receiver_with_block_size()
    }

    fn receiver_with_block_size<const N: usize>()
    -> DesktopReceiverOwner<TestOutput, TestPipeline, SLOTS, OUTPUT_SAMPLES, 1, N> {
        let bridge = ControlToMixerBridge::new(
            SessionState::READY,
            &[Capability::FrameDuration10Ms],
            TestOutput,
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
            TestPipeline::default(),
            None,
        )
    }

    fn hello() -> Envelope {
        Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 1,
            payload: Some(envelope::Payload::Hello(Hello {
                supported_versions: vec![CONTROL_PROTOCOL_VERSION],
                capabilities: vec![
                    Capability::FrameDuration10Ms as i32,
                    Capability::FrameDuration20Ms as i32,
                ],
                display_name: "test sender".to_owned(),
            })),
        }
    }

    fn start() -> Envelope {
        Envelope {
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
        }
    }

    fn stop(request_id: u64) -> Envelope {
        Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id,
            payload: Some(envelope::Payload::StopStream(StopStream {
                media_session_id: 7,
                stream_id: 1,
                media_key_epoch: 0,
            })),
        }
    }

    fn command_reply(
        receiver: &mut DesktopReceiverOwner<
            TestOutput,
            TestPipeline,
            SLOTS,
            OUTPUT_SAMPLES,
            1,
            SOURCE_SAMPLES,
        >,
        session: ControlSessionToken,
        request: Envelope,
    ) -> Envelope {
        let mut control =
            DesktopControlSession::new(receiver, session, MemoryIo::from_frames(&[request]));
        assert!(matches!(
            control.poll_once().unwrap(),
            ControlSessionPoll::Processed { replied: true, .. }
                | ControlSessionPoll::Rejected { replied: true, .. }
        ));
        decode_control_frame(&control.into_inner().output).unwrap()
    }

    #[test]
    fn mismatched_opus_format_can_be_retried_on_the_same_session() {
        let mut receiver = receiver_with_block_size::<960>();
        receiver.pipeline_mut().opus_frame =
            Some(sonimesh_audio::OpusFrameConfig::new(48_000, 2, 10).unwrap());
        let session = receiver
            .owner_mut()
            .install_authenticated_control_session()
            .unwrap();
        receiver.accept_and_execute(session, &hello()).unwrap();
        let mut valid = start();
        valid.request_id = 3;
        if let Some(envelope::Payload::StartStream(start)) = &mut valid.payload {
            start.channels = 2;
        }
        let mut control = DesktopControlSession::new(
            &mut receiver,
            session,
            MemoryIo::from_frames(&[start(), valid]),
        );
        assert!(matches!(
            control.poll_once().unwrap(),
            ControlSessionPoll::Rejected { replied: true, .. }
        ));
        control.poll_once().unwrap();
        let io = control.into_inner();
        let first_len = u32::from_be_bytes(io.output[..4].try_into().unwrap()) as usize + 4;
        let first = decode_control_frame(&io.output[..first_len]).unwrap();
        assert_eq!(first.request_id, 2);
        assert!(
            matches!(first.payload, Some(envelope::Payload::Error(error)) if error.code == ErrorCode::RetryLater as i32 && error.retryable)
        );
        let second = decode_control_frame(&io.output[first_len..]).unwrap();
        assert_eq!(second.request_id, 3);
        assert!(
            matches!(&second.payload, Some(envelope::Payload::StreamStatus(status)) if status.state == StreamState::Streaming as i32),
            "{second:?}"
        );
        assert_eq!(receiver.pipeline_mut().replacements, 1);
        assert!(!receiver.owner().bridge().coordinator().has_pending_change());
    }

    #[test]
    fn mismatched_pipeline_replies_retryably_and_preserves_active_stream() {
        use sonimesh_protocol::{MediaDirection, MediaKeyContext};

        let mut receiver = receiver();
        receiver.pipeline_mut().context =
            Some(MediaKeyContext::new(7, 2, 0, MediaDirection::AndroidToDesktop).unwrap());
        let session = receiver
            .owner_mut()
            .install_authenticated_control_session()
            .unwrap();
        receiver.accept_and_execute(session, &hello()).unwrap();
        let reply = command_reply(&mut receiver, session, start());
        assert_eq!(reply.request_id, 2);
        assert!(
            matches!(reply.payload, Some(envelope::Payload::Error(error)) if error.code == ErrorCode::RetryLater as i32 && error.retryable)
        );
        assert_eq!(receiver.pipeline_mut().replacements, 0);
        assert!(!receiver.owner().bridge().coordinator().has_pending_change());

        receiver.pipeline_mut().context =
            Some(MediaKeyContext::new(7, 1, 0, MediaDirection::AndroidToDesktop).unwrap());
        let reply = command_reply(&mut receiver, session, start());
        assert!(
            matches!(reply.payload, Some(envelope::Payload::StreamStatus(status)) if status.state == StreamState::Streaming as i32)
        );
        let mut rotate = start();
        rotate.request_id = 3;
        if let Some(envelope::Payload::StartStream(start)) = &mut rotate.payload {
            start.media_key_epoch = 1;
        }
        let reply = command_reply(&mut receiver, session, rotate);
        assert_eq!(reply.request_id, 3);
        assert!(matches!(reply.payload, Some(envelope::Payload::Error(error)) if error.retryable));
        assert!(!receiver.owner().bridge().coordinator().has_pending_change());
        let reply = command_reply(&mut receiver, session, start());
        assert!(
            matches!(reply.payload, Some(envelope::Payload::StreamStatus(status)) if status.state == StreamState::Streaming as i32)
        );
        receiver.pipeline_mut().context =
            Some(MediaKeyContext::new(7, 1, 0, MediaDirection::DesktopToAndroid).unwrap());
        let reply = command_reply(&mut receiver, session, start());
        assert!(matches!(reply.payload, Some(envelope::Payload::Error(error)) if error.retryable));
    }

    #[test]
    fn duplicate_start_and_stop_receive_correlated_status_without_repeating_work() {
        let mut receiver = receiver();
        let session = receiver
            .owner_mut()
            .install_authenticated_control_session()
            .unwrap();
        receiver.accept_and_execute(session, &hello()).unwrap();
        receiver.accept_and_execute(session, &start()).unwrap();

        let mut duplicate = start();
        duplicate.request_id = 20;
        let reply = command_reply(&mut receiver, session, duplicate);
        assert_eq!(reply.request_id, 20);
        assert!(
            matches!(reply.payload, Some(envelope::Payload::StreamStatus(status))
            if status.media_session_id == 7 && status.stream_id == 1
                && status.state == StreamState::Streaming as i32)
        );
        assert_eq!(receiver.pipeline_mut().replacements, 1);

        let first_stop = command_reply(&mut receiver, session, stop(21));
        let second_stop = command_reply(&mut receiver, session, stop(22));
        for (reply, request_id) in [(first_stop, 21), (second_stop, 22)] {
            assert_eq!(reply.request_id, request_id);
            assert!(
                matches!(reply.payload, Some(envelope::Payload::StreamStatus(status))
                if status.media_session_id == 7 && status.stream_id == 1
                    && status.state == StreamState::Stopped as i32)
            );
        }
        assert_eq!(receiver.pipeline_mut().replacements, 1);
    }

    #[test]
    fn pending_duplicate_and_unavailable_output_return_retryable_errors() {
        let mut receiver = receiver();
        let session = receiver
            .owner_mut()
            .install_authenticated_control_session()
            .unwrap();
        receiver.accept_and_execute(session, &hello()).unwrap();
        let action = receiver
            .owner_mut()
            .accept_control(session, &start())
            .unwrap();
        let reply = command_reply(&mut receiver, session, start());
        assert_eq!(reply.request_id, 2);
        assert!(
            matches!(reply.payload, Some(envelope::Payload::Error(error))
            if error.code == ErrorCode::RetryLater as i32 && error.retryable)
        );
        let crate::ControlStreamAction::Start { token, .. } = action else {
            panic!("expected pending start");
        };
        receiver
            .owner_mut()
            .bridge_mut()
            .coordinator_mut()
            .abort_pending(token)
            .unwrap();
        receiver.accept_and_execute(session, &start()).unwrap();
        let action = receiver
            .owner_mut()
            .accept_control(session, &stop(30))
            .unwrap();
        let reply = command_reply(&mut receiver, session, stop(31));
        assert_eq!(reply.request_id, 31);
        assert!(
            matches!(reply.payload, Some(envelope::Payload::Error(error))
            if error.code == ErrorCode::RetryLater as i32 && error.retryable)
        );
        let crate::ControlStreamAction::Stop { token, .. } = action else {
            panic!("expected pending stop");
        };
        receiver
            .owner_mut()
            .bridge_mut()
            .coordinator_mut()
            .abort_pending(token)
            .unwrap();
        receiver
            .owner_mut()
            .acknowledge_platform_event(SessionEvent::OutputLost)
            .unwrap();
        let reply = command_reply(&mut receiver, session, start());
        assert!(
            matches!(reply.payload, Some(envelope::Payload::Error(error))
            if error.code == ErrorCode::RetryLater as i32 && error.retryable)
        );
        let identities = receiver.owner_mut().recover_output().unwrap();
        assert_eq!(identities.len(), 1);
        let reply = command_reply(&mut receiver, session, start());
        assert!(
            matches!(reply.payload, Some(envelope::Payload::Error(error))
            if error.code == ErrorCode::RetryLater as i32 && error.retryable)
        );
        receiver.recover_stream(identities[0]).unwrap();
        assert_eq!(receiver.pipeline_mut().replacements, 2);
        let reply = command_reply(&mut receiver, session, start());
        assert!(
            matches!(reply.payload, Some(envelope::Payload::StreamStatus(status))
            if status.state == StreamState::Streaming as i32)
        );
    }

    #[test]
    fn owner_rejection_stays_on_the_session_and_returns_a_correlated_error() {
        let mut receiver = receiver();
        let session = receiver
            .owner_mut()
            .install_authenticated_control_session()
            .unwrap();
        let mut control = DesktopControlSession::new(
            &mut receiver,
            session,
            MemoryIo::from_frames(std::slice::from_ref(&start())),
        );

        assert_eq!(
            control.poll_once().unwrap(),
            ControlSessionPoll::Rejected {
                replied: true,
                write: Some(UntrustedControlWrite::Complete),
            }
        );
        let io = control.into_inner();
        let reply = decode_control_frame(&io.output).unwrap();
        let Some(envelope::Payload::Error(ControlError { code, .. })) = reply.payload else {
            panic!("expected a control error reply");
        };
        assert_eq!(reply.request_id, 2);
        assert_ne!(code, 0);
    }

    #[test]
    fn polls_commands_executes_owner_and_replies_with_correlated_status() {
        let mut receiver = receiver();
        let session = receiver
            .owner_mut()
            .install_authenticated_control_session()
            .unwrap();
        let mut control = DesktopControlSession::new(
            &mut receiver,
            session,
            MemoryIo::from_frames(&[hello(), start()]),
        );

        assert_eq!(
            control.poll_once().unwrap(),
            ControlSessionPoll::Processed {
                replied: true,
                write: Some(UntrustedControlWrite::Complete),
            }
        );
        assert_eq!(
            control.poll_once().unwrap(),
            ControlSessionPoll::Processed {
                replied: true,
                write: Some(UntrustedControlWrite::Complete),
            }
        );
        let io = control.into_inner();
        let hello_len = u32::from_be_bytes(io.output[..4].try_into().unwrap()) as usize + 4;
        let hello_reply = decode_control_frame(&io.output[..hello_len]).unwrap();
        assert_eq!(hello_reply.request_id, 1);
        let Some(envelope::Payload::Hello(hello)) = hello_reply.payload else {
            panic!("expected a hello reply");
        };
        assert_eq!(hello.supported_versions, [CONTROL_PROTOCOL_VERSION]);
        assert_eq!(hello.capabilities, [Capability::FrameDuration10Ms as i32]);
        let reply = decode_control_frame(&io.output[hello_len..]).unwrap();
        let Some(envelope::Payload::StreamStatus(status)) = reply.payload else {
            panic!("expected a stream status reply");
        };
        assert_eq!(reply.request_id, 2);
        assert_eq!(status.state, StreamState::Streaming as i32);
        assert_eq!(status.media_session_id, 7);
        assert_eq!(status.stream_id, 1);
        assert_eq!(receiver.pipeline_mut().replacements, 1);
    }
}
