//! Non-blocking bounded UDP media sender.
//!
//! The data plane carries encrypted Opus datagrams over UDP; this is the
//! transmit-side counterpart to [`UdpMediaReceiver`](crate::UdpMediaReceiver).
//! It accepts at most [`MAX_MEDIA_DATAGRAM_LEN`](sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN)
//! bytes per call, never blocks, and reports a full send buffer as `Ok(false)`
//! so the caller can apply its own backpressure policy. The destination is
//! routing metadata only: this primitive does not authenticate the peer.

use core::fmt;
use std::io;
use std::net::{SocketAddr, UdpSocket};

use sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN;

use crate::redacted_io_error::RedactedIoError;

/// Failure while sending one bounded UDP media datagram.
pub enum UdpSendError {
    /// The payload exceeded the protocol's complete-media limit.
    DatagramTooLarge,
    /// The operating system rejected the send operation.
    Io(io::Error),
}

impl fmt::Debug for UdpSendError {
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

impl fmt::Display for UdpSendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DatagramTooLarge => {
                write!(
                    formatter,
                    "UDP media datagram exceeds {MAX_MEDIA_DATAGRAM_LEN} bytes"
                )
            }
            Self::Io(error) => write!(formatter, "UDP media send failed: {error}"),
        }
    }
}

impl std::error::Error for UdpSendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::DatagramTooLarge => None,
            Self::Io(error) => Some(error),
        }
    }
}

/// Non-blocking, fixed-capacity UDP sender for media datagrams.
pub struct UdpMediaSender {
    socket: UdpSocket,
}

impl UdpMediaSender {
    /// Binds a non-blocking sender to the requested local endpoint.
    ///
    /// # Errors
    ///
    /// Returns an operating-system error when binding or configuring the socket
    /// fails.
    pub fn bind(address: SocketAddr) -> Result<Self, io::Error> {
        let socket = UdpSocket::bind(address)?;
        Self::from_socket(socket)
    }

    /// Takes ownership of a UDP socket and configures it for non-blocking sends.
    ///
    /// # Errors
    ///
    /// Returns an operating-system error when non-blocking mode cannot be set.
    pub fn from_socket(socket: UdpSocket) -> Result<Self, io::Error> {
        socket.set_nonblocking(true)?;
        Ok(Self { socket })
    }

    /// Returns the local endpoint assigned to this sender.
    ///
    /// # Errors
    ///
    /// Returns an operating-system error when the endpoint cannot be queried.
    pub fn local_addr(&self) -> Result<SocketAddr, io::Error> {
        self.socket.local_addr()
    }

    /// Sends one datagram to `peer` without waiting.
    ///
    /// Returns `Ok(true)` when the datagram was handed to the operating system,
    /// and `Ok(false)` when the send buffer is full so the call would block and
    /// nothing was sent; the caller applies its own backpressure policy. The
    /// destination is routing metadata only and is not authenticated here.
    ///
    /// # Errors
    ///
    /// Returns [`UdpSendError::DatagramTooLarge`] before touching the socket
    /// when the payload exceeds the protocol limit, or [`UdpSendError::Io`]
    /// when the operating system rejects the send.
    pub fn send_to(&self, peer: SocketAddr, payload: &[u8]) -> Result<bool, UdpSendError> {
        if payload.len() > MAX_MEDIA_DATAGRAM_LEN {
            return Err(UdpSendError::DatagramTooLarge);
        }
        match self.socket.send_to(payload, peer) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(false),
            Err(error) if is_datagram_too_large(&error) => Err(UdpSendError::DatagramTooLarge),
            Err(error) => Err(UdpSendError::Io(error)),
        }
    }
}

fn is_datagram_too_large(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(40 | 90 | 10040))
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::{UdpMediaSender, UdpSendError};

    #[test]
    fn io_failure_debug_omits_the_free_form_error_message() {
        let error = UdpSendError::Io(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "UDP send to 192.168.10.42:49321 was refused",
        ));

        let debug = format!("{error:?}");

        assert!(debug.contains("ConnectionRefused"));
        assert!(!debug.contains("192.168.10.42"));
        assert!(!debug.contains("was refused"));
    }

    #[test]
    fn oversized_datagram_debug_exposes_only_the_bound() {
        let error = UdpSendError::DatagramTooLarge;

        let debug = format!("{error:?}");

        assert_eq!(debug, "DatagramTooLarge");
    }

    #[test]
    fn oversized_payload_is_rejected_before_touching_the_socket() {
        let sender = UdpMediaSender::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let oversized = [0u8; super::MAX_MEDIA_DATAGRAM_LEN + 1];

        assert!(matches!(
            sender.send_to("127.0.0.1:9".parse().unwrap(), &oversized),
            Err(UdpSendError::DatagramTooLarge)
        ));
    }
}
