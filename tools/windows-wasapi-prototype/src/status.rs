use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Read-only direct-WASAPI render observations for one stream lifetime.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WasapiOutputStatus {
    /// Frames in the endpoint buffer returned after stream initialization.
    pub endpoint_buffer_frames: u32,
    /// Silent frames submitted before the audio client started.
    pub primed_silence_frames: u32,
    /// Number of post-start render cycles observed by the event worker.
    pub render_count: u64,
    /// Most recent post-start render quantum in frames.
    pub last_render_frames: u32,
    /// Largest post-start render quantum in frames.
    pub maximum_render_frames: u32,
    /// Monotonic offset from stream-status creation to the most recent render.
    pub last_render_offset_ns: u64,
    /// Largest observed gap between post-start render cycles.
    pub maximum_render_gap_ns: u64,
    /// Render cycles larger than the configured bounded source capacity.
    pub oversized_renders: u64,
    /// Failures observed after the worker published a started stream.
    pub stream_errors: u64,
}

#[derive(Debug)]
pub(crate) struct WasapiStatus {
    started_at: Instant,
    endpoint_buffer_frames: AtomicU64,
    primed_silence_frames: AtomicU64,
    render_count: AtomicU64,
    last_render_frames: AtomicU64,
    maximum_render_frames: AtomicU64,
    last_render_offset_ns: AtomicU64,
    maximum_render_gap_ns: AtomicU64,
    oversized_renders: AtomicU64,
    stream_errors: AtomicU64,
}

impl WasapiStatus {
    pub(crate) fn endpoint_buffer(&self, frames: u32) {
        self.endpoint_buffer_frames
            .store(u64::from(frames), Ordering::Relaxed);
    }

    pub(crate) fn primed_silence(&self, frames: u32) {
        self.primed_silence_frames
            .store(u64::from(frames), Ordering::Relaxed);
    }

    pub(crate) fn render(&self, frames: u32) {
        let offset_ns = self.elapsed_ns();
        let previous_offset_ns = self
            .last_render_offset_ns
            .swap(offset_ns, Ordering::Relaxed);
        self.render_count.fetch_add(1, Ordering::Relaxed);
        self.last_render_frames
            .store(u64::from(frames), Ordering::Relaxed);
        self.maximum_render_frames
            .fetch_max(u64::from(frames), Ordering::Relaxed);
        if previous_offset_ns > 0 {
            self.maximum_render_gap_ns.fetch_max(
                offset_ns.saturating_sub(previous_offset_ns),
                Ordering::Relaxed,
            );
        }
    }

    pub(crate) fn oversized(&self) {
        self.oversized_renders.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn stream_error(&self) {
        self.stream_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> WasapiOutputStatus {
        WasapiOutputStatus {
            endpoint_buffer_frames: narrow_frames(
                self.endpoint_buffer_frames.load(Ordering::Relaxed),
            ),
            primed_silence_frames: narrow_frames(
                self.primed_silence_frames.load(Ordering::Relaxed),
            ),
            render_count: self.render_count.load(Ordering::Relaxed),
            last_render_frames: narrow_frames(self.last_render_frames.load(Ordering::Relaxed)),
            maximum_render_frames: narrow_frames(
                self.maximum_render_frames.load(Ordering::Relaxed),
            ),
            last_render_offset_ns: self.last_render_offset_ns.load(Ordering::Relaxed),
            maximum_render_gap_ns: self.maximum_render_gap_ns.load(Ordering::Relaxed),
            oversized_renders: self.oversized_renders.load(Ordering::Relaxed),
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

impl Default for WasapiStatus {
    fn default() -> Self {
        Self {
            started_at: Instant::now(),
            endpoint_buffer_frames: AtomicU64::new(0),
            primed_silence_frames: AtomicU64::new(0),
            render_count: AtomicU64::new(0),
            last_render_frames: AtomicU64::new(0),
            maximum_render_frames: AtomicU64::new(0),
            last_render_offset_ns: AtomicU64::new(0),
            maximum_render_gap_ns: AtomicU64::new(0),
            oversized_renders: AtomicU64::new(0),
            stream_errors: AtomicU64::new(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_preserves_render_frame_bounds() {
        let status = WasapiStatus::default();
        status.endpoint_buffer(960);
        status.primed_silence(960);
        status.render(480);
        status.render(960);
        status.oversized();

        let snapshot = status.snapshot();
        assert_eq!(snapshot.endpoint_buffer_frames, 960);
        assert_eq!(snapshot.primed_silence_frames, 960);
        assert_eq!(snapshot.render_count, 2);
        assert_eq!(snapshot.last_render_frames, 960);
        assert_eq!(snapshot.maximum_render_frames, 960);
        assert_eq!(snapshot.oversized_renders, 1);
        assert!(snapshot.maximum_render_gap_ns <= snapshot.last_render_offset_ns);
    }
}
