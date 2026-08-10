#![no_main]

use std::io::{Read, Write};

use libfuzzer_sys::fuzz_target;
use sonimesh_network::{UntrustedControlPoll, UntrustedControlStream};

/// In-memory reader that yields at most `chunk` bytes per read so the bounded
/// length-prefixed control framing state machine is exercised across arbitrary
/// partial length and payload boundaries.
struct ChunkedBytes<'a> {
    data: &'a [u8],
    offset: usize,
    chunk: usize,
}

impl Read for ChunkedBytes<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.offset >= self.data.len() {
            return Ok(0);
        }
        let available = self.data.len() - self.offset;
        let count = available.min(self.chunk).min(buf.len());
        buf[..count].copy_from_slice(&self.data[self.offset..self.offset + count]);
        self.offset += count;
        Ok(count)
    }
}

impl Write for ChunkedBytes<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fuzz_target!(|data: &[u8]| {
    // Vary the chunk size with the first input byte so different inputs traverse
    // different partial-read boundaries through the incremental framing state
    // machine. The reader never reports WouldBlock, so every poll consumes input
    // or completes one frame and the bounded loop below fully drains the input.
    let chunk = usize::from(data.first().copied().unwrap_or(0)) % 16 + 1;
    let mut stream = UntrustedControlStream::from_io(ChunkedBytes {
        data,
        offset: 0,
        chunk,
    });

    for _ in 0..data.len().saturating_add(2) {
        match stream.poll() {
            Ok(UntrustedControlPoll::Empty | UntrustedControlPoll::Frame(_)) => {}
            Ok(UntrustedControlPoll::Closed) | Err(_) => break,
        }
    }
});
