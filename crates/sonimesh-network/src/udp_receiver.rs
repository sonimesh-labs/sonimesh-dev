use core::fmt;
use std::io;
use std::net::{SocketAddr, UdpSocket};

use sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN;

const RECEIVE_BUFFER_LEN: usize = MAX_MEDIA_DATAGRAM_LEN + 1;

/// One datagram observed by the non-blocking media socket.
#[derive(Debug, Eq, PartialEq)]
pub struct ReceivedDatagram<'a> {
    /// Source endpoint supplied by the operating system.
    ///
    /// This is routing metadata only and does not authorize the sender.
    pub source: SocketAddr,
    /// Bytes received before protocol parsing and authentication.
    pub payload: &'a [u8],
}

/// Failure while polling the bounded UDP media socket.
#[derive(Debug)]
pub enum UdpReceiveError {
    /// The datagram exceeded the protocol's complete-media limit.
    DatagramTooLarge,
    /// The operating system rejected the receive operation.
    Io(io::Error),
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
        match self.socket.recv_from(&mut self.buffer) {
            Ok((length, _source)) if length > MAX_MEDIA_DATAGRAM_LEN => {
                Err(UdpReceiveError::DatagramTooLarge)
            }
            Ok((length, source)) => Ok(Some(ReceivedDatagram {
                source,
                payload: &self.buffer[..length],
            })),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error) if is_datagram_too_large(&error) => Err(UdpReceiveError::DatagramTooLarge),
            Err(error) => Err(UdpReceiveError::Io(error)),
        }
    }
}

fn is_datagram_too_large(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(40 | 90 | 10040))
}
