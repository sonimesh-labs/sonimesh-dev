use core::fmt;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use sonimesh_audio::{
    DecodeStep, DecodeWorkerError, FrameDecoder, InsertOutcome, RealtimePcmProducer,
};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_network::{ProtectedIngressError, UdpMediaReceiver, UdpReceiveError};
use sonimesh_protocol::{MAX_MEDIA_PLAINTEXT_LEN, MediaKeyContext, SessionSecret};

use crate::{ReceiverPipelineError, ReceiverStreamPipeline};

/// Error while constructing a UDP-backed receiver pipeline.
#[derive(Debug)]
pub enum UdpReceiverSetupError {
    /// The local media socket could not be bound or configured.
    Socket(io::Error),
    /// The protected ingress could not derive its initial media key.
    Pipeline(ProtectedIngressError),
}

impl fmt::Display for UdpReceiverSetupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Socket(error) => write!(formatter, "UDP receiver setup failed: {error}"),
            Self::Pipeline(error) => write!(formatter, "receiver pipeline setup failed: {error}"),
        }
    }
}

impl std::error::Error for UdpReceiverSetupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Socket(error) => Some(error),
            Self::Pipeline(error) => Some(error),
        }
    }
}

/// Error while polling or admitting one UDP media datagram.
#[derive(Debug)]
pub enum UdpReceiverError<DecoderError> {
    /// The socket could not provide a datagram.
    Socket(UdpReceiveError),
    /// The datagram reached the protected receiver but failed its bounded path.
    Pipeline(ReceiverPipelineError<DecoderError>),
}

impl<DecoderError: fmt::Display> fmt::Display for UdpReceiverError<DecoderError> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Socket(error) => write!(formatter, "UDP receiver failed: {error}"),
            Self::Pipeline(error) => write!(formatter, "UDP media pipeline failed: {error}"),
        }
    }
}

impl<DecoderError> std::error::Error for UdpReceiverError<DecoderError> where
    DecoderError: std::error::Error + 'static
{
}

/// Result of one non-blocking UDP media poll.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UdpPollOutcome {
    /// No datagram was ready and no work was performed.
    Empty,
    /// One datagram reached the bounded jitter admission boundary.
    Processed {
        /// Untrusted source endpoint observed by the socket.
        source: SocketAddr,
        /// Jitter admission result for the authenticated packet.
        admission: InsertOutcome,
    },
}

/// Application-domain composition of UDP polling, protected ingress, and decode.
///
/// This type owns one stream's fixed plaintext scratch buffer and pipeline. It
/// does not own pairing, TLS, session lifecycle, or the realtime output
/// consumer; those remain control-domain responsibilities.
pub struct UdpReceiverPipeline<
    D,
    const JITTER_CAPACITY: usize,
    const MAX_ENCODED_BYTES: usize,
    const PCM_SAMPLES_PER_BLOCK: usize,
> {
    receiver: UdpMediaReceiver,
    pipeline: ReceiverStreamPipeline<D, JITTER_CAPACITY, MAX_ENCODED_BYTES, PCM_SAMPLES_PER_BLOCK>,
    plaintext: [u8; MAX_MEDIA_PLAINTEXT_LEN],
}

impl<
    D,
    const JITTER_CAPACITY: usize,
    const MAX_ENCODED_BYTES: usize,
    const PCM_SAMPLES_PER_BLOCK: usize,
> UdpReceiverPipeline<D, JITTER_CAPACITY, MAX_ENCODED_BYTES, PCM_SAMPLES_PER_BLOCK>
where
    D: FrameDecoder,
{
    /// Binds a UDP socket and creates one protected stream pipeline.
    ///
    /// The caller supplies a root secret that has already been authenticated by
    /// the control plane. This constructor does not establish trust.
    ///
    /// # Errors
    ///
    /// Returns [`UdpReceiverSetupError`] when socket setup or media key setup
    /// fails.
    pub fn bind(
        address: SocketAddr,
        secret: &SessionSecret,
        context: MediaKeyContext,
        decoder: D,
        producer: RealtimePcmProducer<PCM_SAMPLES_PER_BLOCK>,
        counters: Arc<StreamCounters>,
        initial_sequence: u32,
    ) -> Result<Self, UdpReceiverSetupError> {
        let receiver = UdpMediaReceiver::bind(address).map_err(UdpReceiverSetupError::Socket)?;
        let pipeline = ReceiverStreamPipeline::new(
            secret,
            context,
            decoder,
            producer,
            counters,
            initial_sequence,
        )
        .map_err(UdpReceiverSetupError::Pipeline)?;
        Ok(Self {
            receiver,
            pipeline,
            plaintext: [0; MAX_MEDIA_PLAINTEXT_LEN],
        })
    }

    /// Returns the local endpoint assigned to the media socket.
    ///
    /// # Errors
    ///
    /// Returns an operating-system error when the endpoint cannot be queried.
    pub fn local_addr(&self) -> Result<SocketAddr, io::Error> {
        self.receiver.local_addr()
    }

    /// Polls one datagram and admits it into the protected jitter boundary.
    ///
    /// Oversized datagrams are counted as rejected packets before returning the
    /// socket error. Other socket errors are not packet-level rejections.
    ///
    /// # Errors
    ///
    /// Returns [`UdpReceiverError`] for socket or protected pipeline failures.
    pub fn poll(&mut self) -> Result<UdpPollOutcome, UdpReceiverError<D::Error>> {
        let Some(datagram) = (match self.receiver.poll() {
            Ok(datagram) => datagram,
            Err(error) => {
                if matches!(error, UdpReceiveError::DatagramTooLarge) {
                    self.pipeline.counters().rejected();
                }
                return Err(UdpReceiverError::Socket(error));
            }
        }) else {
            return Ok(UdpPollOutcome::Empty);
        };
        let source = datagram.source;
        let admission = self
            .pipeline
            .receive(datagram.payload, &mut self.plaintext)
            .map_err(UdpReceiverError::Pipeline)?;
        Ok(UdpPollOutcome::Processed { source, admission })
    }

    /// Runs one decode or PLC decision outside the output callback.
    ///
    /// # Errors
    ///
    /// Returns the stream-local decoder error.
    pub fn decode_next(&mut self) -> Result<DecodeStep, DecodeWorkerError<D::Error>> {
        self.pipeline.decode_next()
    }

    /// Rotates this stream to a newer authenticated media epoch.
    ///
    /// # Errors
    ///
    /// Returns [`ProtectedIngressError`] without changing the pipeline when the
    /// replacement context is invalid.
    pub fn rotate(
        &mut self,
        secret: &SessionSecret,
        next: MediaKeyContext,
        initial_sequence: u32,
    ) -> Result<(), ProtectedIngressError> {
        self.pipeline.rotate(secret, next, initial_sequence)
    }

    /// Borrows the stream's atomic diagnostics.
    #[must_use]
    pub fn counters(&self) -> &StreamCounters {
        self.pipeline.counters()
    }
}
