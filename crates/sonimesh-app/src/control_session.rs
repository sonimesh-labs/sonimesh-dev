//! Authenticated control-session adapter for the desktop receiver.
//!
//! Pairing produces a trusted TLS channel, while this module owns the next
//! small boundary: bounded control framing, owner dispatch, and correlated
//! lifecycle replies. It deliberately does not own pairing, media polling, or
//! a UI runtime.

use std::io::{Read, Write};

use sonimesh_network::{
    UntrustedControlError, UntrustedControlPoll, UntrustedControlStream, UntrustedControlWrite,
};
use sonimesh_protocol::{ControlFrameError, Envelope, ErrorCode, StreamState};

use crate::{
    ControlReply, ControlSessionToken, ControlStreamAction, ControlToMixerError,
    DesktopControlOwnerError, DesktopReceiverOwner, ReceiverPcmProducerInstaller,
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
        Self {
            receiver,
            session,
            framed: UntrustedControlStream::from_io(channel),
        }
    }

    /// Polls and processes at most one control envelope.
    ///
    /// Lifecycle replies are correlated with the request ID. Hello, no-op, and
    /// ignored envelopes produce no reply because version one has no dedicated
    /// negotiation acknowledgement payload.
    ///
    /// # Errors
    ///
    /// Returns [`ControlSessionError`] when framing, owner execution, or reply
    /// encoding cannot progress.
    pub fn poll_once(&mut self) -> Result<ControlSessionPoll, ControlSessionError> {
        match self.framed.poll().map_err(ControlSessionError::Transport)? {
            UntrustedControlPoll::Empty => Ok(ControlSessionPoll::Empty),
            UntrustedControlPoll::Closed => Ok(ControlSessionPoll::Closed),
            UntrustedControlPoll::Frame(envelope) => self.process(&envelope),
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

    fn process(&mut self, envelope: &Envelope) -> Result<ControlSessionPoll, ControlSessionError> {
        let request_id = envelope.request_id;
        match self.receiver.accept_and_execute(self.session, envelope) {
            Ok(action) => {
                let Some(reply) =
                    reply_for_action(action, request_id).map_err(ControlSessionError::Reply)?
                else {
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

fn reply_for_action(
    action: ControlStreamAction,
    request_id: u64,
) -> Result<Option<Envelope>, ControlFrameError> {
    let (identity, state, detail) = match action {
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
        ControlStreamAction::Negotiated(_)
        | ControlStreamAction::Noop
        | ControlStreamAction::Ignored => return Ok(None),
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
        CONTROL_PROTOCOL_VERSION, Capability, ControlError, Envelope, Hello, StartStream,
        StreamState, decode_control_frame, encode_control_frame, envelope,
    };

    use super::{ControlSessionPoll, DesktopControlSession};
    use crate::{
        ControlToMixerBridge, DesktopControlOwner, DesktopReceiverOwner, OutputSelection,
        ReceiverPcmProducerInstaller, SessionState,
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
    }

    impl ReceiverPcmProducerInstaller<SOURCE_SAMPLES> for TestPipeline {
        fn replace_pcm_producer(&mut self, _producer: RealtimePcmProducer<SOURCE_SAMPLES>) {
            self.replacements += 1;
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
                capabilities: vec![Capability::FrameDuration10Ms as i32],
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
                replied: false,
                write: None,
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
        let reply = decode_control_frame(&io.output).unwrap();
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
