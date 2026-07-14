use std::sync::atomic::{AtomicU64, Ordering};

/// Lock-free counters writable from realtime or network domains.
#[derive(Debug, Default)]
pub struct StreamCounters {
    authenticated_packets: AtomicU64,
    duplicate_packets: AtomicU64,
    rejected_packets: AtomicU64,
    late_packets: AtomicU64,
    reordered_packets: AtomicU64,
    pcm_underflows: AtomicU64,
    pcm_overflows: AtomicU64,
    stream_resets: AtomicU64,
}

impl StreamCounters {
    /// Records an authenticated packet.
    pub fn authenticated(&self) {
        self.authenticated_packets.fetch_add(1, Ordering::Relaxed);
    }

    /// Records an authenticated duplicate.
    pub fn duplicate(&self) {
        self.duplicate_packets.fetch_add(1, Ordering::Relaxed);
    }

    /// Records a malformed, unknown, or unauthenticated packet.
    pub fn rejected(&self) {
        self.rejected_packets.fetch_add(1, Ordering::Relaxed);
    }

    /// Records a packet arriving after its deadline.
    pub fn late(&self) {
        self.late_packets.fetch_add(1, Ordering::Relaxed);
    }

    /// Records an accepted reordered packet.
    pub fn reordered(&self) {
        self.reordered_packets.fetch_add(1, Ordering::Relaxed);
    }

    /// Records output silence caused by an empty PCM ring.
    pub fn pcm_underflow(&self) {
        self.pcm_underflows.fetch_add(1, Ordering::Relaxed);
    }

    /// Records a bounded oldest-region drop in the PCM ring.
    pub fn pcm_overflow(&self) {
        self.pcm_overflows.fetch_add(1, Ordering::Relaxed);
    }

    /// Records a stream-local reset.
    pub fn stream_reset(&self) {
        self.stream_resets.fetch_add(1, Ordering::Relaxed);
    }

    /// Reads a non-transactional diagnostic snapshot.
    ///
    /// Counters use relaxed ordering because they describe telemetry, not
    /// synchronization. A concurrent event may appear in the next snapshot.
    #[must_use]
    pub fn snapshot(&self) -> StreamMetricsSnapshot {
        StreamMetricsSnapshot {
            authenticated_packets: self.authenticated_packets.load(Ordering::Relaxed),
            duplicate_packets: self.duplicate_packets.load(Ordering::Relaxed),
            rejected_packets: self.rejected_packets.load(Ordering::Relaxed),
            late_packets: self.late_packets.load(Ordering::Relaxed),
            reordered_packets: self.reordered_packets.load(Ordering::Relaxed),
            pcm_underflows: self.pcm_underflows.load(Ordering::Relaxed),
            pcm_overflows: self.pcm_overflows.load(Ordering::Relaxed),
            stream_resets: self.stream_resets.load(Ordering::Relaxed),
        }
    }
}

/// Copyable diagnostic view for low-rate control/UI publication.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StreamMetricsSnapshot {
    /// Successfully authenticated packets.
    pub authenticated_packets: u64,
    /// Authenticated duplicates.
    pub duplicate_packets: u64,
    /// Packets rejected before decode.
    pub rejected_packets: u64,
    /// Packets discarded after their deadline.
    pub late_packets: u64,
    /// Accepted out-of-order packets.
    pub reordered_packets: u64,
    /// Output callbacks that emitted bounded silence.
    pub pcm_underflows: u64,
    /// Bounded oldest-region PCM drops.
    pub pcm_overflows: u64,
    /// Stream-local resets.
    pub stream_resets: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_atomic_counters() {
        let counters = StreamCounters::default();
        counters.authenticated();
        counters.rejected();
        counters.pcm_underflow();

        let snapshot = counters.snapshot();
        assert_eq!(snapshot.authenticated_packets, 1);
        assert_eq!(snapshot.rejected_packets, 1);
        assert_eq!(snapshot.pcm_underflows, 1);
    }
}
