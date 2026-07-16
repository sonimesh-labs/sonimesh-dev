use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Low-rate diagnostic snapshot for the active CPAL stream.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CpalOutputStatus {
    /// Output callbacks observed since the current stream started.
    pub callback_count: u64,
    /// Interleaved samples in the latest callback.
    pub last_callback_samples: usize,
    /// Largest callback observed for the current stream.
    pub maximum_callback_samples: usize,
    /// Callbacks exceeding preallocated scratch/output policy.
    pub oversized_callbacks: u64,
    /// Asynchronous stream errors reported by CPAL.
    pub stream_errors: u64,
}

#[derive(Debug, Default)]
pub(crate) struct CallbackStatus {
    callback_count: AtomicU64,
    last_callback_samples: AtomicUsize,
    maximum_callback_samples: AtomicUsize,
    oversized_callbacks: AtomicU64,
    stream_errors: AtomicU64,
}

impl CallbackStatus {
    pub(crate) fn callback(&self, samples: usize) {
        self.callback_count.fetch_add(1, Ordering::Relaxed);
        self.last_callback_samples.store(samples, Ordering::Relaxed);
        self.maximum_callback_samples
            .fetch_max(samples, Ordering::Relaxed);
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
            oversized_callbacks: self.oversized_callbacks.load(Ordering::Relaxed),
            stream_errors: self.stream_errors.load(Ordering::Relaxed),
        }
    }
}
