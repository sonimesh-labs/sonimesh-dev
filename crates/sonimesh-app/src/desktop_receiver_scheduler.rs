//! Shell-neutral owner of one desktop receiver scheduling loop.
//!
//! This boundary keeps receiver ownership, an optional framed control channel,
//! and low-rate media ticks together without introducing a UI or async runtime.
//! The caller still chooses the thread, timer, and platform output backend.

use std::io::{Read, Write};

use sonimesh_audio::FrameDecoder;
use sonimesh_network::PinnedServerChannel;
use sonimesh_platform::AudioOutput;

use crate::{
    ControlSessionError, ControlSessionPoll, ControlSessionToken, DesktopControlSession,
    DesktopReceiverOwner, ReceiverPcmProducerInstaller, TrustedControlChannel, UdpMediaTick,
    UdpReceiverError, UdpReceiverPipeline,
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
    control: Option<(ControlSessionToken, S)>,
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
        self.control = Some((session, channel));
        Ok(())
    }

    /// Detaches the current channel, if one is attached.
    pub fn detach_control(&mut self) -> Option<(ControlSessionToken, S)> {
        self.control.take()
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
        let Some((session, channel)) = self.control.take() else {
            return Ok(None);
        };
        let mut control = DesktopControlSession::new(&mut self.receiver, session, channel);
        let result = control.poll_once();
        let channel = control.into_inner();
        match result {
            Ok(ControlSessionPoll::Closed) => Ok(Some(ControlSessionPoll::Closed)),
            Ok(poll) => {
                self.control = Some((session, channel));
                Ok(Some(poll))
            }
            Err(error) => Err(error),
        }
    }
}

#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod tests {
    use std::io;

    use sonimesh_audio::RealtimePcmProducer;
    use sonimesh_platform::{
        AudioOutput, OutputConfig, OutputDeviceId, OutputDeviceInfo, OutputError, OutputHealth,
        RenderSource, SampleFormat,
    };
    use sonimesh_protocol::Capability;

    use super::{DesktopReceiverScheduler, DesktopReceiverSchedulerError};
    use crate::{
        ControlToMixerBridge, DesktopControlOwner, DesktopReceiverOwner, OutputSelection,
        ReceiverPcmProducerInstaller, SessionState,
    };

    const SLOTS: usize = 1;
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
    struct TestPipeline;

    impl ReceiverPcmProducerInstaller<SOURCE_SAMPLES> for TestPipeline {
        fn replace_pcm_producer(&mut self, _producer: RealtimePcmProducer<SOURCE_SAMPLES>) {}
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

    fn scheduler() -> DesktopReceiverScheduler<
        TestOutput,
        TestPipeline,
        SLOTS,
        OUTPUT_SAMPLES,
        1,
        SOURCE_SAMPLES,
        IdleIo,
    > {
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
        DesktopReceiverScheduler::new(DesktopReceiverOwner::new(
            DesktopControlOwner::new(bridge).unwrap(),
            TestPipeline,
            None,
        ))
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
}
