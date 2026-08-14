use core::fmt;
use std::io;
use std::net::{SocketAddr, UdpSocket};

use sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN;

use crate::{redacted_endpoint::RedactedEndpoint, redacted_io_error::RedactedIoError};

const RECEIVE_BUFFER_LEN: usize = MAX_MEDIA_DATAGRAM_LEN + 1;

/// One datagram observed by the non-blocking media socket.
#[derive(Eq, PartialEq)]
pub struct ReceivedDatagram<'a> {
    /// Source endpoint supplied by the operating system.
    ///
    /// This is routing metadata only and does not authorize the sender.
    pub source: SocketAddr,
    /// Bytes received before protocol parsing and authentication.
    pub payload: &'a [u8],
}

impl fmt::Debug for ReceivedDatagram<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReceivedDatagram")
            .field("source", &RedactedEndpoint(self.source))
            .field("payload_len", &self.payload.len())
            .finish()
    }
}

/// Failure while polling the bounded UDP media socket.
pub enum UdpReceiveError {
    /// The datagram exceeded the protocol's complete-media limit.
    DatagramTooLarge,
    /// The operating system rejected the receive operation.
    Io(io::Error),
}

impl fmt::Debug for UdpReceiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DatagramTooLarge => formatter.write_str("DatagramTooLarge"),
            Self::Io(error) => formatter
                .debug_tuple("Io")
                .field(&RedactedIoError(error))
                .finish(),
        }
    }
}

impl fmt::Display for UdpReceiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DatagramTooLarge => {
                write!(
                    formatter,
                    "UDP media datagram exceeds {MAX_MEDIA_DATAGRAM_LEN} bytes"
                )
            }
            Self::Io(error) => write!(formatter, "UDP media receive failed: {error}"),
        }
    }
}

impl std::error::Error for UdpReceiveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::DatagramTooLarge => None,
            Self::Io(error) => Some(error),
        }
    }
}

/// Non-blocking, fixed-capacity UDP receiver for media datagrams.
pub struct UdpMediaReceiver {
    socket: UdpSocket,
    buffer: [u8; RECEIVE_BUFFER_LEN],
}

impl UdpMediaReceiver {
    /// Binds a non-blocking receiver to the requested local endpoint.
    ///
    /// # Errors
    ///
    /// Returns an operating-system error when binding or configuring the socket
    /// fails.
    pub fn bind(address: SocketAddr) -> Result<Self, io::Error> {
        let socket = UdpSocket::bind(address)?;
        Self::from_socket(socket)
    }

    /// Takes ownership of a UDP socket and configures it for non-blocking polls.
    ///
    /// # Errors
    ///
    /// Returns an operating-system error when non-blocking mode cannot be set.
    pub fn from_socket(socket: UdpSocket) -> Result<Self, io::Error> {
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            buffer: [0; RECEIVE_BUFFER_LEN],
        })
    }

    /// Returns the local endpoint assigned to this receiver.
    ///
    /// # Errors
    ///
    /// Returns an operating-system error when the endpoint cannot be queried.
    pub fn local_addr(&self) -> Result<SocketAddr, io::Error> {
        self.socket.local_addr()
    }

    /// Polls once without waiting for network data.
    ///
    /// A returned source address is observational metadata only. Callers must
    /// still parse, authenticate, and replay-check the payload before decoding.
    ///
    /// # Errors
    ///
    /// Returns [`UdpReceiveError::DatagramTooLarge`] before exposing an
    /// oversized payload, or [`UdpReceiveError::Io`] for another receive error.
    pub fn poll(&mut self) -> Result<Option<ReceivedDatagram<'_>>, UdpReceiveError> {
        let received = classify_receive_result(self.socket.recv_from(&mut self.buffer))?;
        Ok(received.map(|(source, length)| ReceivedDatagram {
            source,
            payload: &self.buffer[..length],
        }))
    }
}

/// Maps one operating-system receive result to the bounded public outcome.
///
/// Kept apart from the socket so the classification is deterministic: a
/// truncated oversized datagram, a would-block poll, and the platform
/// datagram-too-large codes map to explicit outcomes without depending on
/// loopback timing.
fn classify_receive_result(
    result: io::Result<(usize, SocketAddr)>,
) -> Result<Option<(SocketAddr, usize)>, UdpReceiveError> {
    match result {
        Ok((length, _source)) if length > MAX_MEDIA_DATAGRAM_LEN => {
            Err(UdpReceiveError::DatagramTooLarge)
        }
        Ok((length, source)) => Ok(Some((source, length))),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(error) if is_datagram_too_large(&error) => Err(UdpReceiveError::DatagramTooLarge),
        Err(error) => Err(UdpReceiveError::Io(error)),
    }
}

fn is_datagram_too_large(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(40 | 90 | 10040))
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::net::SocketAddr;

    use super::{ReceivedDatagram, UdpReceiveError, classify_receive_result};

    #[test]
    fn debug_output_redacts_endpoint_and_payload() {
        let datagram = ReceivedDatagram {
            source: "192.168.10.42:49321".parse().unwrap(),
            payload: b"untrusted audio bytes",
        };

        let debug = format!("{datagram:?}");

        assert!(debug.contains("ipv4"));
        assert!(debug.contains("49321"));
        assert!(debug.contains("payload_len: 21"));
        assert!(!debug.contains("192.168.10.42"));
        assert!(!debug.contains("untrusted audio bytes"));
    }

    #[test]
    fn io_failure_debug_omits_the_free_form_error_message() {
        let error = UdpReceiveError::Io(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "UDP receive at 192.168.10.42:49321 was refused",
        ));

        let debug = format!("{error:?}");

        assert!(debug.contains("ConnectionRefused"));
        assert!(!debug.contains("192.168.10.42"));
        assert!(!debug.contains("was refused"));
    }

    #[test]
    fn complete_datagram_reports_source_and_payload_length() {
        let source: SocketAddr = "192.168.10.42:49321".parse().unwrap();

        let outcome = classify_receive_result(Ok((21, source)));

        match outcome {
            Ok(Some((reported_source, length))) => {
                assert_eq!(reported_source, source);
                assert_eq!(length, 21);
            }
            other => panic!("expected a complete datagram, got {other:?}"),
        }
    }

    #[test]
    fn truncated_oversized_datagram_maps_to_the_bounded_variant() {
        let source: SocketAddr = "192.168.10.42:49321".parse().unwrap();

        assert!(matches!(
            classify_receive_result(Ok((super::MAX_MEDIA_DATAGRAM_LEN + 1, source))),
            Err(UdpReceiveError::DatagramTooLarge)
        ));
    }

    #[test]
    fn empty_poll_reports_no_datagram_without_an_error() {
        let error = io::Error::new(io::ErrorKind::WouldBlock, "no datagram is ready");

        assert!(matches!(classify_receive_result(Err(error)), Ok(None)));
    }

    #[test]
    fn datagram_too_large_os_errors_map_to_the_bounded_variant() {
        for raw_os_error in [40, 90, 10040] {
            let error = io::Error::from_raw_os_error(raw_os_error);

            assert!(matches!(
                classify_receive_result(Err(error)),
                Err(UdpReceiveError::DatagramTooLarge)
            ));
        }
    }

    #[test]
    fn unrelated_os_error_maps_to_the_io_variant_and_preserves_the_kind() {
        let error = io::Error::new(io::ErrorKind::PermissionDenied, "socket receive failed");

        match classify_receive_result(Err(error)) {
            Err(UdpReceiveError::Io(error)) => {
                assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            }
            other => panic!("expected an I/O error, got {other:?}"),
        }
    }
}
