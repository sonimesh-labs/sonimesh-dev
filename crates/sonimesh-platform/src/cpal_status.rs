use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

/// Read-only callback measurements captured without touching the control domain.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CpalOutputStatus {
    /// Number of output callbacks observed for the current stream.
    pub callback_count: u64,
    /// Most recent interleaved callback length.
    pub last_callback_samples: usize,
    /// Largest interleaved callback length observed for the current stream.
    pub maximum_callback_samples: usize,
    /// Most recent callback length expressed in device frames.
    pub last_callback_frames: u32,
    /// Largest callback length expressed in device frames.
    pub maximum_callback_frames: u32,
    /// Monotonic offset from stream-status creation to the most recent callback.
    pub last_callback_offset_ns: u64,
    /// Largest observed gap between callbacks in the current stream.
    pub maximum_callback_gap_ns: u64,
    /// Callbacks exceeding preallocated scratch/output policy.
    pub oversized_callbacks: u64,
    /// Asynchronous stream errors reported by CPAL.
    pub stream_errors: u64,
}

#[derive(Debug)]
pub(crate) struct CallbackStatus {
    started_at: Instant,
    callback_count: AtomicU64,
    last_callback_samples: AtomicUsize,
    maximum_callback_samples: AtomicUsize,
    last_callback_frames: AtomicU64,
    maximum_callback_frames: AtomicU64,
    last_callback_offset_ns: AtomicU64,
    maximum_callback_gap_ns: AtomicU64,
    oversized_callbacks: AtomicU64,
    stream_errors: AtomicU64,
}

impl CallbackStatus {
    pub(crate) fn callback(&self, samples: usize, frames: u32) {
        let offset_ns = self.elapsed_ns();
        let previous_offset_ns = self
            .last_callback_offset_ns
            .swap(offset_ns, Ordering::Relaxed);
        self.callback_count.fetch_add(1, Ordering::Relaxed);
        self.last_callback_samples.store(samples, Ordering::Relaxed);
        self.maximum_callback_samples
            .fetch_max(samples, Ordering::Relaxed);
        self.last_callback_frames
            .store(u64::from(frames), Ordering::Relaxed);
        self.maximum_callback_frames
            .fetch_max(u64::from(frames), Ordering::Relaxed);
        if previous_offset_ns > 0 {
            self.maximum_callback_gap_ns.fetch_max(
                offset_ns.saturating_sub(previous_offset_ns),
                Ordering::Relaxed,
            );
        }
    }

    pub(crate) fn oversized(&self) {
        self.oversized_callbacks.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn stream_error(&self) {
        self.stream_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> CpalOutputStatus {
        CpalOutputStatus {
            callback_count: self.callback_count.load(Ordering::Relaxed),
            last_callback_samples: self.last_callback_samples.load(Ordering::Relaxed),
            maximum_callback_samples: self.maximum_callback_samples.load(Ordering::Relaxed),
            last_callback_frames: narrow_frames(self.last_callback_frames.load(Ordering::Relaxed)),
            maximum_callback_frames: narrow_frames(
                self.maximum_callback_frames.load(Ordering::Relaxed),
            ),
            last_callback_offset_ns: self.last_callback_offset_ns.load(Ordering::Relaxed),
            maximum_callback_gap_ns: self.maximum_callback_gap_ns.load(Ordering::Relaxed),
            oversized_callbacks: self.oversized_callbacks.load(Ordering::Relaxed),
            stream_errors: self.stream_errors.load(Ordering::Relaxed),
        }
    }

    fn elapsed_ns(&self) -> u64 {
        u64::try_from(
            self.started_at
                .elapsed()
                .as_nanos()
                .min(u128::from(u64::MAX)),
        )
        .unwrap_or(u64::MAX)
    }
}

fn narrow_frames(frames: u64) -> u32 {
    u32::try_from(frames).unwrap_or(u32::MAX)
}

impl Default for CallbackStatus {
    fn default() -> Self {
        Self {
            started_at: Instant::now(),
            callback_count: AtomicU64::new(0),
            last_callback_samples: AtomicUsize::new(0),
            maximum_callback_samples: AtomicUsize::new(0),
            last_callback_frames: AtomicU64::new(0),
            maximum_callback_frames: AtomicU64::new(0),
            last_callback_offset_ns: AtomicU64::new(0),
            maximum_callback_gap_ns: AtomicU64::new(0),
            oversized_callbacks: AtomicU64::new(0),
            stream_errors: AtomicU64::new(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_preserves_frame_counts_and_callback_gap() {
        let status = CallbackStatus::default();
        status.callback(960, 480);
        status.callback(1_920, 960);

        let snapshot = status.snapshot();
        assert_eq!(snapshot.callback_count, 2);
        assert_eq!(snapshot.last_callback_samples, 1_920);
        assert_eq!(snapshot.maximum_callback_samples, 1_920);
        assert_eq!(snapshot.last_callback_frames, 960);
        assert_eq!(snapshot.maximum_callback_frames, 960);
        assert!(snapshot.maximum_callback_gap_ns <= snapshot.last_callback_offset_ns);
    }
}
