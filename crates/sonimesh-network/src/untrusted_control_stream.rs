use core::fmt;
use std::io::{self, Read, Write};
use std::net::TcpStream;

use sonimesh_protocol::{
    ControlFrameError, Envelope, MAX_CONTROL_FRAME_BYTES, decode_control_frame,
    encode_control_frame,
};

const CONTROL_LENGTH_BYTES: usize = 4;
const CONTROL_BUFFER_BYTES: usize = CONTROL_LENGTH_BYTES + MAX_CONTROL_FRAME_BYTES;

/// One result from polling an untrusted control stream.
#[derive(Debug, Eq, PartialEq)]
pub enum UntrustedControlPoll {
    /// No complete frame is ready and the socket would block.
    Empty,
    /// One complete, semantically validated envelope was decoded.
    Frame(Envelope),
    /// The peer closed the connection or framing failed permanently.
    Closed,
}

/// Failure while reading or validating an untrusted control stream.
#[derive(Debug)]
pub enum UntrustedControlError {
    /// The peer sent a malformed or semantically invalid frame.
    Protocol(ControlFrameError),
    /// The operating system rejected a socket operation.
    Io(io::Error),
    /// A previous encoded frame is still waiting for socket progress.
    WritePending,
}

impl fmt::Display for UntrustedControlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Protocol(error) => write!(formatter, "untrusted control frame rejected: {error}"),
            Self::Io(error) => write!(formatter, "untrusted control stream I/O failed: {error}"),
            Self::WritePending => {
                formatter.write_str("untrusted control stream has a pending write")
            }
        }
    }
}

impl std::error::Error for UntrustedControlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Protocol(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::WritePending => None,
        }
    }
}

/// Result of one non-blocking control frame write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UntrustedControlWrite {
    /// The complete frame was accepted by the operating system.
    Complete,
    /// The frame remains buffered and requires another flush.
    Pending,
    /// The peer has closed the connection.
    Closed,
}

/// Incremental, non-blocking length-prefixed control reader/writer without transport security.
///
/// This type only provides bounded framing and semantic Protobuf validation. It
/// must be placed behind the future authenticated TLS/pairing transport before
/// its envelopes can influence a trusted control session.
pub struct UntrustedControlStream {
    stream: TcpStream,
    buffer: Box<[u8]>,
    write_buffer: Box<[u8]>,
    write_len: usize,
    write_offset: usize,
    length_filled: usize,
    frame_len: Option<usize>,
    frame_filled: usize,
    terminated: bool,
}

impl UntrustedControlStream {
    /// Takes ownership of a TCP stream and configures it for non-blocking reads.
    ///
    /// # Errors
    ///
    /// Returns an operating-system error when non-blocking mode cannot be set.
    pub fn from_stream(stream: TcpStream) -> Result<Self, io::Error> {
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            buffer: vec![0; CONTROL_BUFFER_BYTES].into_boxed_slice(),
            write_buffer: vec![0; CONTROL_BUFFER_BYTES].into_boxed_slice(),
            write_len: 0,
            write_offset: 0,
            length_filled: 0,
            frame_len: None,
            frame_filled: 0,
            terminated: false,
        })
    }

    /// Returns the peer endpoint for control-domain connection bookkeeping.
    ///
    /// # Errors
    ///
    /// Returns an operating-system error when the peer endpoint cannot be queried.
    pub fn peer_addr(&self) -> Result<std::net::SocketAddr, io::Error> {
        self.stream.peer_addr()
    }

    /// Queues and advances one semantically validated control frame.
    ///
    /// The frame is copied into one bounded write buffer. A second frame cannot
    /// overwrite it until [`Self::flush_write`] reports completion.
    ///
    /// # Errors
    ///
    /// Returns [`UntrustedControlError::Protocol`] when envelope validation or
    /// encoding fails, [`UntrustedControlError::WritePending`] when a previous
    /// frame is still buffered, or [`UntrustedControlError::Io`] for a socket
    /// failure.
    pub fn send(
        &mut self,
        envelope: &Envelope,
    ) -> Result<UntrustedControlWrite, UntrustedControlError> {
        if self.terminated {
            return Ok(UntrustedControlWrite::Closed);
        }
        if self.write_offset < self.write_len {
            return Err(UntrustedControlError::WritePending);
        }

        let frame = encode_control_frame(envelope).map_err(UntrustedControlError::Protocol)?;
        if frame.len() > self.write_buffer.len() {
            return Err(UntrustedControlError::Protocol(
                ControlFrameError::FrameTooLarge,
            ));
        }
        self.write_buffer[..frame.len()].copy_from_slice(&frame);
        self.write_len = frame.len();
        self.write_offset = 0;
        self.flush_write()
    }

    /// Advances a previously queued frame without waiting for socket capacity.
    ///
    /// # Errors
    ///
    /// Returns [`UntrustedControlError::Io`] when the socket write fails.
    pub fn flush_write(&mut self) -> Result<UntrustedControlWrite, UntrustedControlError> {
        if self.terminated {
            return Ok(UntrustedControlWrite::Closed);
        }
        if self.write_offset == self.write_len {
            return Ok(UntrustedControlWrite::Complete);
        }

        match self
            .stream
            .write(&self.write_buffer[self.write_offset..self.write_len])
        {
            Ok(0) => {
                self.terminated = true;
                Ok(UntrustedControlWrite::Closed)
            }
            Ok(written) => {
                self.write_offset += written;
                if self.write_offset == self.write_len {
                    self.write_len = 0;
                    self.write_offset = 0;
                    Ok(UntrustedControlWrite::Complete)
                } else {
                    Ok(UntrustedControlWrite::Pending)
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                Ok(UntrustedControlWrite::Pending)
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                Ok(UntrustedControlWrite::Pending)
            }
            Err(error) => Err(UntrustedControlError::Io(error)),
        }
    }

    /// Reads at most one complete frame without waiting for network data.
    ///
    /// A protocol error permanently terminates this reader. Callers should drop
    /// the associated connection and establish a fresh authenticated transport.
    ///
    /// # Errors
    ///
    /// Returns [`UntrustedControlError`] for socket or frame validation errors.
    pub fn poll(&mut self) -> Result<UntrustedControlPoll, UntrustedControlError> {
        if self.terminated {
            return Ok(UntrustedControlPoll::Closed);
        }

        if self.frame_len.is_none() {
            match self.read_into(CONTROL_LENGTH_BYTES, self.length_filled) {
                Err(ReadState::WouldBlock) => return Ok(UntrustedControlPoll::Empty),
                Ok(_) => {}
                Err(ReadState::Closed) => {
                    self.terminated = true;
                    return Ok(UntrustedControlPoll::Closed);
                }
                Err(ReadState::Io(error)) => return Err(UntrustedControlError::Io(error)),
            }
            if self.length_filled < CONTROL_LENGTH_BYTES {
                return Ok(UntrustedControlPoll::Empty);
            }

            let Ok(length_bytes) = self.buffer[..CONTROL_LENGTH_BYTES].try_into() else {
                return self.protocol_failure(ControlFrameError::TruncatedLength);
            };
            let declared = u32::from_be_bytes(length_bytes);
            let frame_len = usize::try_from(declared).unwrap_or(usize::MAX);
            if frame_len > MAX_CONTROL_FRAME_BYTES {
                return self.protocol_failure(ControlFrameError::FrameTooLarge);
            }
            self.frame_len = Some(frame_len);
            self.frame_filled = CONTROL_LENGTH_BYTES;
        }

        let Some(payload_len) = self.frame_len else {
            return self.protocol_failure(ControlFrameError::TruncatedLength);
        };
        if self.frame_filled < CONTROL_LENGTH_BYTES + payload_len {
            match self.read_into(payload_len, self.frame_filled) {
                Err(ReadState::WouldBlock) => return Ok(UntrustedControlPoll::Empty),
                Ok(_) => {}
                Err(ReadState::Closed) => {
                    self.terminated = true;
                    return Ok(UntrustedControlPoll::Closed);
                }
                Err(ReadState::Io(error)) => return Err(UntrustedControlError::Io(error)),
            }
        }
        if self.frame_filled < CONTROL_LENGTH_BYTES + payload_len {
            return Ok(UntrustedControlPoll::Empty);
        }

        let frame = &self.buffer[..CONTROL_LENGTH_BYTES + payload_len];
        let envelope = match decode_control_frame(frame) {
            Ok(envelope) => envelope,
            Err(error) => return self.protocol_failure(error),
        };
        self.reset_frame();
        Ok(UntrustedControlPoll::Frame(envelope))
    }

    fn read_into(&mut self, target_len: usize, filled: usize) -> Result<usize, ReadState> {
        let end = if target_len == CONTROL_LENGTH_BYTES {
            CONTROL_LENGTH_BYTES
        } else {
            CONTROL_LENGTH_BYTES + target_len
        };
        let start = filled;
        match self.stream.read(&mut self.buffer[start..end]) {
            Ok(0) => Err(ReadState::Closed),
            Ok(read) => {
                if target_len == CONTROL_LENGTH_BYTES {
                    self.length_filled += read;
                } else {
                    self.frame_filled += read;
                }
                Ok(read)
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Err(ReadState::WouldBlock),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => Err(ReadState::WouldBlock),
            Err(error) => Err(ReadState::Io(error)),
        }
    }

    fn reset_frame(&mut self) {
        self.length_filled = 0;
        self.frame_len = None;
        self.frame_filled = 0;
    }

    fn protocol_failure(
        &mut self,
        error: ControlFrameError,
    ) -> Result<UntrustedControlPoll, UntrustedControlError> {
        self.terminated = true;
        Err(UntrustedControlError::Protocol(error))
    }
}

enum ReadState {
    WouldBlock,
    Closed,
    Io(io::Error),
}
