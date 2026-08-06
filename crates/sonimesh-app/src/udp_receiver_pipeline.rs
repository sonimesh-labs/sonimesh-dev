use core::fmt;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use sonimesh_audio::{
    DecodeStep, DecodeWorkerError, FrameDecoder, InsertOutcome, RealtimePcmProducer,
};
use sonimesh_diagnostics::{StreamCounters, redact_endpoint};
use sonimesh_network::{
    ProtectedIngressError, SourceEndpointAdmissionError, SourceEndpointPin, UdpMediaReceiver,
    UdpReceiveError,
};
use sonimesh_protocol::{MAX_MEDIA_PLAINTEXT_LEN, MediaKeyContext, SessionSecret};

use crate::{FreshSessionInstallError, ReceiverPipelineError, ReceiverStreamPipeline};

/// Error while constructing a UDP-backed receiver pipeline.
pub enum UdpReceiverSetupError {
    /// The local media socket could not be bound or configured.
    Socket(io::Error),
    /// The protected ingress could not derive its initial media key.
    Pipeline(ProtectedIngressError),
}

impl fmt::Debug for UdpReceiverSetupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Socket(error) => formatter
                .debug_struct("Socket")
                .field("kind", &error.kind())
                .field("raw_os_error", &error.raw_os_error())
                .finish(),
            Self::Pipeline(error) => formatter.debug_tuple("Pipeline").field(error).finish(),
        }
    }
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
    /// The datagram came from a different endpoint than the pinned source.
    SourceEndpointChanged,
    /// The datagram reached the protected receiver but failed its bounded path.
    Pipeline(ReceiverPipelineError<DecoderError>),
}

impl<DecoderError: fmt::Display> fmt::Display for UdpReceiverError<DecoderError> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Socket(error) => write!(formatter, "UDP receiver failed: {error}"),
            Self::SourceEndpointChanged => formatter.write_str(
                "UDP media source endpoint changed before a fresh session was installed",
            ),
            Self::Pipeline(error) => write!(formatter, "UDP media pipeline failed: {error}"),
        }
    }
}

impl<DecoderError> std::error::Error for UdpReceiverError<DecoderError>
where
    DecoderError: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Socket(error) => Some(error),
            Self::SourceEndpointChanged => None,
            Self::Pipeline(error) => Some(error),
        }
    }
}

/// Result of one non-blocking UDP media poll.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum UdpPollOutcome {
    /// No datagram was ready and no work was performed.
    Empty,
    /// One datagram reached the bounded jitter admission boundary.
    Processed {
        /// Source endpoint observed for the admitted packet.
        ///
        /// This successful outcome establishes the pin when the session was
        /// previously unbound; the endpoint itself is not part of the AEAD data.
        source: SocketAddr,
        /// Jitter admission result for the authenticated packet.
        admission: InsertOutcome,
    },
}

impl fmt::Debug for UdpPollOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("Empty"),
            Self::Processed { source, admission } => {
                let redacted_source = redact_endpoint(*source);
                formatter
                    .debug_struct("Processed")
                    .field("source", &redacted_source)
                    .field("admission", admission)
                    .finish()
            }
        }
    }
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
    source_pin: SourceEndpointPin,
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
            source_pin: SourceEndpointPin::new(),
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

    /// Returns the source endpoint pinned by the first protected packet outcome.
    ///
    /// A fresh pipeline or successfully installed fresh session is unbound
    /// until protected ingress accepts a packet and bounded jitter handling
    /// returns an outcome.
    #[must_use]
    pub const fn pinned_source(&self) -> Option<SocketAddr> {
        self.source_pin.pinned()
    }

    /// Polls one datagram and admits it into the protected jitter boundary.
    ///
    /// Oversized datagrams and packets from a changed source endpoint are
    /// counted as rejected packets. Once the first protected packet returns
    /// an outcome, another source cannot reach parsing or authentication until a
    /// fresh authenticated session is installed. Other socket errors are not
    /// packet-level rejections.
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
        let Self {
            source_pin,
            pipeline,
            plaintext,
            ..
        } = self;
        let admission =
            match source_pin.process(source, || pipeline.receive(datagram.payload, plaintext)) {
                Ok(admission) => admission,
                Err(SourceEndpointAdmissionError::SourceEndpointChanged) => {
                    plaintext.fill(0);
                    pipeline.counters().rejected();
                    return Err(UdpReceiverError::SourceEndpointChanged);
                }
                Err(SourceEndpointAdmissionError::Processing(error)) => {
                    return Err(UdpReceiverError::Pipeline(error));
                }
            };
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

    /// Installs authenticated media state for a newly established session.
    ///
    /// # Errors
    ///
    /// Returns without changing the active pipeline when media key derivation
    /// for the replacement context fails.
    pub fn install_fresh_session(
        &mut self,
        secret: &SessionSecret,
        context: MediaKeyContext,
        initial_sequence: u32,
    ) -> Result<(), FreshSessionInstallError> {
        self.pipeline
            .install_fresh_session(secret, context, initial_sequence)?;
        self.plaintext.fill(0);
        self.source_pin.clear_for_fresh_session();
        Ok(())
    }

    /// Clears this stream's playout state after a drift-controller hard resync.
    pub fn hard_resynchronize(&mut self) {
        self.pipeline.hard_resynchronize();
    }

    /// Installs a producer paired with a freshly created output source.
    pub fn replace_pcm_producer(&mut self, producer: RealtimePcmProducer<PCM_SAMPLES_PER_BLOCK>) {
        self.pipeline.replace_pcm_producer(producer);
    }

    /// Borrows the stream's atomic diagnostics.
    #[must_use]
    pub fn counters(&self) -> &StreamCounters {
        self.pipeline.counters()
    }
}

#[cfg(test)]
mod error_source_tests {
    use std::error::Error;
    use std::io;

    use sonimesh_audio::{DecodeWorkerError, FrameError};
    use sonimesh_network::{IngressError, ProtectedIngressError, UdpReceiveError};
    use sonimesh_protocol::{MediaCryptoError, PacketParseError};

    use super::UdpReceiverError;
    use crate::ReceiverPipelineError;

    fn source<'a>(error: &'a (dyn Error + 'static)) -> &'a (dyn Error + 'static) {
        error
            .source()
            .expect("composite error must retain its cause")
    }

    #[test]
    fn socket_errors_retain_the_operating_system_cause() {
        let error: UdpReceiverError<MediaCryptoError> =
            UdpReceiverError::Socket(UdpReceiveError::Io(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "test receive failure",
            )));

        let receive = source(&error);
        assert!(receive.downcast_ref::<UdpReceiveError>().is_some());
        let operating_system = source(receive);
        assert!(operating_system.downcast_ref::<io::Error>().is_some());
        assert!(operating_system.source().is_none());
    }

    #[test]
    fn ingress_errors_retain_parse_and_authentication_causes() {
        let parse: UdpReceiverError<MediaCryptoError> = UdpReceiverError::Pipeline(
            ReceiverPipelineError::Ingress(IngressError::Parse(PacketParseError::Truncated)),
        );
        let receiver = source(&parse);
        assert!(
            receiver
                .downcast_ref::<ReceiverPipelineError<MediaCryptoError>>()
                .is_some()
        );
        let ingress = source(receiver);
        assert!(
            ingress
                .downcast_ref::<IngressError<MediaCryptoError>>()
                .is_some()
        );
        let packet = source(ingress);
        assert!(packet.downcast_ref::<PacketParseError>().is_some());
        assert!(packet.source().is_none());

        let authentication: UdpReceiverError<MediaCryptoError> =
            UdpReceiverError::Pipeline(ReceiverPipelineError::Ingress(
                IngressError::Authentication(MediaCryptoError::AuthenticationFailed),
            ));
        let crypto = source(source(source(&authentication)));
        assert!(crypto.downcast_ref::<MediaCryptoError>().is_some());
        assert!(crypto.source().is_none());
    }

    #[test]
    fn decode_errors_retain_frame_and_decoder_causes() {
        let frame: UdpReceiverError<MediaCryptoError> = UdpReceiverError::Pipeline(
            ReceiverPipelineError::Decode(DecodeWorkerError::Frame(FrameError::PayloadTooLarge)),
        );
        let frame_error = source(source(source(&frame)));
        assert!(frame_error.downcast_ref::<FrameError>().is_some());
        assert!(frame_error.source().is_none());

        let decoder: UdpReceiverError<MediaCryptoError> =
            UdpReceiverError::Pipeline(ReceiverPipelineError::Decode(DecodeWorkerError::Decoder(
                MediaCryptoError::PayloadTooLarge,
            )));
        let decoder_error = source(source(source(&decoder)));
        assert!(decoder_error.downcast_ref::<MediaCryptoError>().is_some());
        assert!(decoder_error.source().is_none());
    }

    #[test]
    fn rotation_errors_retain_crypto_causes_without_synthetic_policy_causes() {
        let error: UdpReceiverError<MediaCryptoError> =
            UdpReceiverError::Pipeline(ReceiverPipelineError::Rotation(
                ProtectedIngressError::Crypto(MediaCryptoError::InvalidContext),
            ));

        let crypto = source(source(source(&error)));
        assert!(crypto.downcast_ref::<MediaCryptoError>().is_some());
        assert!(crypto.source().is_none());
        assert!(Error::source(&ProtectedIngressError::NonIncreasingEpoch).is_none());
        assert!(
            Error::source(&UdpReceiverError::<MediaCryptoError>::SourceEndpointChanged).is_none()
        );
    }
}

#[cfg(test)]
mod setup_debug_tests {
    use std::io;

    use super::UdpReceiverSetupError;

    #[test]
    fn socket_failure_debug_omits_the_free_form_error_message() {
        let error = UdpReceiverSetupError::Socket(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "bind failed for 192.168.10.42:49321",
        ));

        let debug = format!("{error:?}");

        assert!(debug.contains("AddrNotAvailable"));
        assert!(!debug.contains("192.168.10.42"));
        assert!(!debug.contains("bind failed"));
    }
}

#[cfg(test)]
mod poll_debug_tests {
    use sonimesh_audio::InsertOutcome;

    use super::UdpPollOutcome;

    #[test]
    fn processed_poll_debug_redacts_the_source_address() {
        let outcome = UdpPollOutcome::Processed {
            source: "192.168.10.42:49321".parse().unwrap(),
            admission: InsertOutcome::Inserted,
        };

        let debug = format!("{outcome:?}");

        assert!(debug.contains("[ipv4-address-redacted]:49321"));
        assert!(!debug.contains("192.168.10.42"));
    }
}
