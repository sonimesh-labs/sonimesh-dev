use core::fmt;
use std::io;
use std::net::SocketAddr;

use sonimesh_protocol::{
    MAX_MEDIA_PLAINTEXT_LEN, MediaCryptoError, MediaKeyContext, MediaSealer, SessionSecret,
};

use crate::redacted_io_error::RedactedIoError;
use crate::udp_sender::{UdpMediaSender, UdpSendError};

/// Failure while constructing or rotating a bounded media session sender.
pub enum MediaSessionSenderError {
    /// The media socket could not be bound or configured.
    Socket(io::Error),
    /// The media key could not be derived from the session context.
    Crypto(MediaCryptoError),
    /// Rotation changed session, stream, or direction.
    RotationContextMismatch,
    /// Rotation did not advance to a strictly newer key epoch.
    NonIncreasingEpoch,
}

impl fmt::Debug for MediaSessionSenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Socket(error) => formatter
                .debug_struct("Socket")
                .field("kind", &error.kind())
                .field("raw_os_error", &error.raw_os_error())
                .finish(),
            Self::Crypto(error) => formatter.debug_tuple("Crypto").field(error).finish(),
            Self::RotationContextMismatch => formatter.write_str("RotationContextMismatch"),
            Self::NonIncreasingEpoch => formatter.write_str("NonIncreasingEpoch"),
        }
    }
}

impl fmt::Display for MediaSessionSenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Socket(error) => write!(formatter, "media session sender setup failed: {error}"),
            Self::Crypto(error) => {
                write!(formatter, "media session sender key setup failed: {error}")
            }
            Self::RotationContextMismatch => {
                formatter.write_str("media session rotation changed session, stream, or direction")
            }
            Self::NonIncreasingEpoch => {
                formatter.write_str("media session rotation did not advance the key epoch")
            }
        }
    }
}

impl std::error::Error for MediaSessionSenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Socket(error) => Some(error),
            Self::Crypto(error) => Some(error),
            Self::RotationContextMismatch | Self::NonIncreasingEpoch => None,
        }
    }
}

/// Failure while sending one bounded media frame.
pub enum MediaSessionSendError {
    /// The encoded frame exceeded the bounded media plaintext capacity.
    PayloadTooLarge,
    /// The sender sequence for the current key epoch is exhausted.
    SequenceExhausted,
    /// The sealer rejected the frame for a reason outside the bounded policy.
    Crypto(MediaCryptoError),
    /// The operating system rejected the send operation.
    Io(io::Error),
}

impl fmt::Debug for MediaSessionSendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLarge => formatter.write_str("PayloadTooLarge"),
            Self::SequenceExhausted => formatter.write_str("SequenceExhausted"),
            Self::Crypto(error) => formatter.debug_tuple("Crypto").field(error).finish(),
            Self::Io(error) => formatter
                .debug_tuple("Io")
                .field(&RedactedIoError(error))
                .finish(),
        }
    }
}

impl fmt::Display for MediaSessionSendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLarge => {
                write!(
                    formatter,
                    "media frame exceeds the {MAX_MEDIA_PLAINTEXT_LEN}-byte plaintext limit"
                )
            }
            Self::SequenceExhausted => formatter.write_str("media key epoch sequence is exhausted"),
            Self::Crypto(error) => write!(formatter, "media frame sealing failed: {error}"),
            Self::Io(error) => write!(formatter, "UDP media send failed: {error}"),
        }
    }
}

impl std::error::Error for MediaSessionSendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::PayloadTooLarge | Self::SequenceExhausted => None,
            Self::Crypto(error) => Some(error),
            Self::Io(error) => Some(error),
        }
    }
}

/// Result of one bounded media frame send.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaSessionSendOutcome {
    /// The datagram was handed to the operating system for `sequence`.
    Sent {
        /// Packet sequence within the current key epoch.
        sequence: u32,
    },
    /// The send buffer was full and nothing reached the wire for `sequence`.
    ///
    /// The sequence was still consumed by the sealer, so the receiver observes
    /// a gap and applies its own loss policy; the caller should apply
    /// backpressure rather than retrying the same frame.
    BufferFull {
        /// Packet sequence consumed by the sealing step.
        sequence: u32,
    },
}

/// Session-level bounded UDP media sender for one stream and key epoch.
///
/// This is a network-domain object; it must never be called from an output
/// callback. It owns the per-epoch sealer (sequence and AEAD), the non-blocking
/// socket, and the routing destination. Rotating to a strictly newer key epoch
/// replaces the sealer and starts a fresh sequence at zero, mirroring the
/// receiver pipeline's rotation boundary.
pub struct MediaSessionSender {
    transport: UdpMediaSender,
    sealer: MediaSealer,
    context: MediaKeyContext,
    peer: SocketAddr,
}

impl MediaSessionSender {
    /// Binds a media session sender and derives the initial media key.
    ///
    /// # Errors
    ///
    /// Returns [`MediaSessionSenderError::Socket`] when binding or configuring
    /// the socket fails, or [`MediaSessionSenderError::Crypto`] when the media
    /// key cannot be derived from the caller-supplied authenticated session
    /// secret.
    pub fn bind(
        address: SocketAddr,
        secret: &SessionSecret,
        context: MediaKeyContext,
        peer: SocketAddr,
    ) -> Result<Self, MediaSessionSenderError> {
        let transport = UdpMediaSender::bind(address).map_err(MediaSessionSenderError::Socket)?;
        Self::with_transport(transport, secret, context, peer)
    }

    /// Takes ownership of a UDP socket and derives the initial media key.
    ///
    /// # Errors
    ///
    /// Returns [`MediaSessionSenderError::Socket`] when non-blocking mode cannot
    /// be set, or [`MediaSessionSenderError::Crypto`] when the media key cannot
    /// be derived from the caller-supplied authenticated session secret.
    pub fn from_socket(
        socket: std::net::UdpSocket,
        secret: &SessionSecret,
        context: MediaKeyContext,
        peer: SocketAddr,
    ) -> Result<Self, MediaSessionSenderError> {
        let transport =
            UdpMediaSender::from_socket(socket).map_err(MediaSessionSenderError::Socket)?;
        Self::with_transport(transport, secret, context, peer)
    }

    fn with_transport(
        transport: UdpMediaSender,
        secret: &SessionSecret,
        context: MediaKeyContext,
        peer: SocketAddr,
    ) -> Result<Self, MediaSessionSenderError> {
        let sealer = MediaSealer::new(secret, context).map_err(MediaSessionSenderError::Crypto)?;
        Ok(Self {
            transport,
            sealer,
            context,
            peer,
        })
    }

    /// Returns the local endpoint assigned to this sender.
    ///
    /// # Errors
    ///
    /// Returns an operating-system error when the endpoint cannot be queried.
    pub fn local_addr(&self) -> Result<SocketAddr, io::Error> {
        self.transport.local_addr()
    }

    /// Returns the routing destination for this media session.
    #[must_use]
    pub const fn peer(&self) -> SocketAddr {
        self.peer
    }

    /// Updates the routing destination for this media session.
    ///
    /// The destination is routing metadata only and is not authenticated. A
    /// control-session address migration may update it without rotating media
    /// keys; the receiver rebinds its source pin only after a fresh session is
    /// installed.
    pub fn set_peer(&mut self, peer: SocketAddr) {
        self.peer = peer;
    }

    /// Returns the media key context currently used to seal packets.
    #[must_use]
    pub const fn context(&self) -> MediaKeyContext {
        self.context
    }

    /// Seals and sends one bounded encoded media frame.
    ///
    /// Returns [`MediaSessionSendOutcome::Sent`] when the datagram was handed
    /// to the operating system, and [`MediaSessionSendOutcome::BufferFull`]
    /// when the send buffer is full so the call would block; the caller applies
    /// its own backpressure policy. The frame is rejected before touching the
    /// socket when it exceeds the protocol plaintext limit.
    ///
    /// # Errors
    ///
    /// Returns [`MediaSessionSendError::PayloadTooLarge`] for an oversized
    /// frame, [`MediaSessionSendError::SequenceExhausted`] when the current key
    /// epoch requires rotation, or [`MediaSessionSendError::Io`] when the
    /// operating system rejects the send.
    pub fn send_frame(
        &mut self,
        flags: u8,
        sample_timestamp: u64,
        plaintext: &[u8],
    ) -> Result<MediaSessionSendOutcome, MediaSessionSendError> {
        let packet = self
            .sealer
            .seal(flags, sample_timestamp, plaintext)
            .map_err(map_seal_error)?;
        let sequence = packet.header.sequence;
        classify_send_outcome(
            self.transport.send_to(self.peer, &packet.datagram),
            sequence,
        )
    }

    /// Rotates this session to a strictly newer authenticated media epoch.
    ///
    /// The new context must keep the same session, stream, and direction and
    /// advance the key epoch; the replacement sealer starts a fresh sequence at
    /// zero, matching the receiver pipeline's rotation boundary.
    ///
    /// # Errors
    ///
    /// Returns [`MediaSessionSenderError`] without changing the sender when the
    /// replacement context is invalid, changes the session/stream/direction, or
    /// does not advance the epoch.
    pub fn rotate(
        &mut self,
        secret: &SessionSecret,
        next: MediaKeyContext,
    ) -> Result<(), MediaSessionSenderError> {
        if next.session_id != self.context.session_id
            || next.stream_id != self.context.stream_id
            || next.direction != self.context.direction
        {
            return Err(MediaSessionSenderError::RotationContextMismatch);
        }
        if next.key_epoch <= self.context.key_epoch {
            return Err(MediaSessionSenderError::NonIncreasingEpoch);
        }
        let sealer = MediaSealer::new(secret, next).map_err(MediaSessionSenderError::Crypto)?;
        self.sealer = sealer;
        self.context = next;
        Ok(())
    }

    /// Installs authenticated media state for a newly established session.
    ///
    /// This replaces the sealer and context for a fresh session, stream, and
    /// epoch and restarts the sequence at zero. The routing destination is
    /// unchanged; call [`Self::set_peer`] when the fresh session targets a new
    /// endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`MediaSessionSenderError::Crypto`] when the media key cannot be
    /// derived from the new session secret.
    pub fn install_fresh_session(
        &mut self,
        secret: &SessionSecret,
        context: MediaKeyContext,
    ) -> Result<(), MediaSessionSenderError> {
        let sealer = MediaSealer::new(secret, context).map_err(MediaSessionSenderError::Crypto)?;
        self.sealer = sealer;
        self.context = context;
        Ok(())
    }
}

fn map_seal_error(error: MediaCryptoError) -> MediaSessionSendError {
    match error {
        MediaCryptoError::PayloadTooLarge => MediaSessionSendError::PayloadTooLarge,
        MediaCryptoError::SequenceExhausted => MediaSessionSendError::SequenceExhausted,
        other => MediaSessionSendError::Crypto(other),
    }
}

/// Maps one bounded transport send result to the session-level send outcome.
///
/// The sealer already consumed `sequence` before the transport result existed,
/// so a full send buffer still reports the consumed sequence and the receiver
/// observes a gap rather than a retried frame. The sealer only produces bounded
/// datagrams, so an impossible oversized transport result fails closed.
fn classify_send_outcome(
    result: Result<bool, UdpSendError>,
    sequence: u32,
) -> Result<MediaSessionSendOutcome, MediaSessionSendError> {
    match result {
        Ok(true) => Ok(MediaSessionSendOutcome::Sent { sequence }),
        Ok(false) => Ok(MediaSessionSendOutcome::BufferFull { sequence }),
        Err(UdpSendError::DatagramTooLarge) => Err(MediaSessionSendError::PayloadTooLarge),
        Err(UdpSendError::Io(error)) => Err(MediaSessionSendError::Io(error)),
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use sonimesh_protocol::{
        MAX_MEDIA_PLAINTEXT_LEN, MEDIA_HEADER_LEN, MediaDirection, MediaKeyContext,
        parse_media_packet,
    };

    use super::{
        MediaSessionSendError, MediaSessionSendOutcome, MediaSessionSender,
        MediaSessionSenderError, classify_send_outcome,
    };
    use crate::udp_sender::UdpSendError;

    fn context(key_epoch: u16) -> MediaKeyContext {
        MediaKeyContext::new(7, 9, key_epoch, MediaDirection::AndroidToDesktop).unwrap()
    }

    fn loopback_peer() -> std::net::SocketAddr {
        "127.0.0.1:9".parse().unwrap()
    }

    fn secret() -> sonimesh_protocol::SessionSecret {
        sonimesh_protocol::SessionSecret::new([0xA5; 32])
    }

    fn sender(key_epoch: u16) -> MediaSessionSender {
        MediaSessionSender::bind(
            "127.0.0.1:0".parse().unwrap(),
            &secret(),
            context(key_epoch),
            loopback_peer(),
        )
        .unwrap()
    }

    #[test]
    fn oversized_frame_is_rejected_before_touching_the_socket() {
        let mut sender = sender(1);
        let oversized = [0xA5; MAX_MEDIA_PLAINTEXT_LEN + 1];

        assert!(matches!(
            sender.send_frame(0, 0, &oversized),
            Err(MediaSessionSendError::PayloadTooLarge)
        ));

        // Rejection must not consume a sequence: the next valid frame is zero.
        let outcome = sender.send_frame(0, 0, &[1]).unwrap();
        assert_eq!(
            outcome,
            super::MediaSessionSendOutcome::Sent { sequence: 0 }
        );
    }

    #[test]
    fn rotation_requires_the_same_session_stream_and_direction() {
        let mut sender = sender(1);
        let mut changed_stream = context(2);
        changed_stream.stream_id += 1;
        assert!(matches!(
            sender.rotate(&secret(), changed_stream),
            Err(MediaSessionSenderError::RotationContextMismatch)
        ));
        assert_eq!(sender.context(), context(1));
    }

    #[test]
    fn rotation_requires_a_strictly_newer_epoch() {
        let mut sender = sender(3);
        assert!(matches!(
            sender.rotate(&secret(), context(3)),
            Err(MediaSessionSenderError::NonIncreasingEpoch)
        ));
        assert!(matches!(
            sender.rotate(&secret(), context(2)),
            Err(MediaSessionSenderError::NonIncreasingEpoch)
        ));
        assert_eq!(sender.context(), context(3));
    }

    #[test]
    fn rotation_replaces_the_sealer_and_resets_the_sequence() {
        let mut sender = sender(1);
        sender.send_frame(0, 0, &[1]).unwrap();

        sender.rotate(&secret(), context(2)).unwrap();
        assert_eq!(sender.context().key_epoch, 2);

        let outcome = sender.send_frame(0, 960, &[2]).unwrap();
        assert_eq!(
            outcome,
            super::MediaSessionSendOutcome::Sent { sequence: 0 }
        );
    }

    #[test]
    fn fresh_session_installs_a_new_context_and_keeps_the_peer() {
        let mut sender = sender(1);
        sender.send_frame(0, 0, &[1]).unwrap();
        let peer = sender.peer();

        let fresh = MediaKeyContext::new(11, 12, 1, MediaDirection::AndroidToDesktop).unwrap();
        sender.install_fresh_session(&secret(), fresh).unwrap();
        assert_eq!(sender.context(), fresh);
        assert_eq!(sender.peer(), peer);

        let outcome = sender.send_frame(0, 0, &[3]).unwrap();
        assert_eq!(
            outcome,
            super::MediaSessionSendOutcome::Sent { sequence: 0 }
        );
    }

    #[test]
    fn send_error_debug_omits_the_free_form_error_message() {
        let error = MediaSessionSendError::Io(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "UDP send to 192.168.10.42:49321 was refused",
        ));

        let debug = format!("{error:?}");

        assert!(debug.contains("ConnectionRefused"));
        assert!(!debug.contains("192.168.10.42"));
        assert!(!debug.contains("was refused"));
    }

    #[test]
    fn oversized_frame_debug_exposes_only_the_bound() {
        let error = MediaSessionSendError::PayloadTooLarge;
        assert_eq!(format!("{error:?}"), "PayloadTooLarge");
    }

    #[test]
    fn loopback_delivers_a_sealed_datagram_to_the_receiver() {
        let mut sender = sender(1);
        let mut receiver = crate::UdpMediaReceiver::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        sender.set_peer(receiver.local_addr().unwrap());

        let outcome = sender
            .send_frame(0x02, 0x0123_4567_89AB_CDEF, &[7, 8, 9])
            .unwrap();
        assert!(matches!(
            outcome,
            super::MediaSessionSendOutcome::Sent { sequence: 0 }
        ));

        for _ in 0..1_000 {
            if let Some(datagram) = receiver.poll().unwrap() {
                assert_eq!(datagram.payload.len(), MEDIA_HEADER_LEN + 3 + 16);
                let packet = parse_media_packet(datagram.payload).unwrap();
                assert_eq!(packet.header.flags, 0x02);
                assert_eq!(packet.header.sample_timestamp, 0x0123_4567_89AB_CDEF);
                assert_eq!(packet.header.session_id, 7);
                assert_eq!(packet.header.stream_id, 9);
                assert_eq!(packet.header.key_epoch, 1);
                assert_eq!(packet.header.sequence, 0);
                let opener = sonimesh_protocol::MediaOpener::new(&secret(), context(1)).unwrap();
                let mut plaintext = [0_u8; MAX_MEDIA_PLAINTEXT_LEN];
                let length = opener.open(packet, &mut plaintext).unwrap();
                assert_eq!(&plaintext[..length], [7, 8, 9]);
                return;
            }
            std::thread::yield_now();
        }
        panic!("loopback datagram was not delivered");
    }

    #[test]
    fn handed_to_the_os_reports_sent_with_the_consumed_sequence() {
        assert!(matches!(
            classify_send_outcome(Ok(true), 41),
            Ok(MediaSessionSendOutcome::Sent { sequence: 41 })
        ));
    }

    #[test]
    fn full_send_buffer_reports_backpressure_with_the_consumed_sequence() {
        // The sealer consumed the sequence even though nothing reached the wire,
        // so the receiver observes a gap and the caller must not retry the frame.
        assert!(matches!(
            classify_send_outcome(Ok(false), 41),
            Ok(MediaSessionSendOutcome::BufferFull { sequence: 41 })
        ));
    }

    #[test]
    fn impossible_oversized_transport_result_fails_closed() {
        assert!(matches!(
            classify_send_outcome(Err(UdpSendError::DatagramTooLarge), 41),
            Err(MediaSessionSendError::PayloadTooLarge)
        ));
    }

    #[test]
    fn os_send_error_maps_to_the_io_variant_and_preserves_the_kind() {
        let error = UdpSendError::Io(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "peer refused the datagram",
        ));

        match classify_send_outcome(Err(error), 41) {
            Err(MediaSessionSendError::Io(error)) => {
                assert_eq!(error.kind(), io::ErrorKind::ConnectionRefused);
            }
            other => panic!("expected an I/O error, got {other:?}"),
        }
    }
}
