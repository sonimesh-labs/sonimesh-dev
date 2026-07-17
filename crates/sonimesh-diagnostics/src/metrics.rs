use std::sync::atomic::{AtomicI32, AtomicU16, AtomicU64, Ordering};

/// Lock-free counters writable from realtime or network domains.
#[derive(Debug, Default)]
pub struct StreamCounters {
    authenticated_packets: AtomicU64,
    duplicate_packets: AtomicU64,
    rejected_packets: AtomicU64,
    late_packets: AtomicU64,
    reordered_packets: AtomicU64,
    jitter_resync_dropped_frames: AtomicU64,
    plc_frames: AtomicU64,
    decode_errors: AtomicU64,
    pcm_underflows: AtomicU64,
    pcm_overflows: AtomicU64,
    pcm_discontinuities: AtomicU64,
    queued_pcm_blocks: AtomicU16,
    queued_pcm_high_water_blocks: AtomicU16,
    stream_resets: AtomicU64,
    nominal_sample_rates: AtomicU64,
    drift_adjustment_ppm: AtomicI32,
    drift_hard_resyncs: AtomicU64,
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

    /// Records a packet rejected before it can reach decoder work.
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

    /// Records encoded frames discarded by one controlled jitter resynchronization.
    pub fn jitter_resync_dropped_frames(&self, frames: usize) {
        let frames = u64::try_from(frames).unwrap_or(u64::MAX);
        self.jitter_resync_dropped_frames
            .fetch_add(frames, Ordering::Relaxed);
    }

    /// Records one decoder-side packet-loss-concealment frame.
    pub fn plc_frame(&self) {
        self.plc_frames.fetch_add(1, Ordering::Relaxed);
    }

    /// Records one stream-local decoder or bounded decoder-output failure.
    pub fn decode_error(&self) {
        self.decode_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Records output silence caused by an empty PCM ring.
    pub fn pcm_underflow(&self) {
        self.pcm_underflows.fetch_add(1, Ordering::Relaxed);
    }

    /// Records a bounded oldest-region drop in the PCM ring.
    pub fn pcm_overflow(&self) {
        self.pcm_overflows.fetch_add(1, Ordering::Relaxed);
    }

    /// Records a sequence or epoch gap observed by the PCM render source.
    pub fn pcm_discontinuity(&self) {
        self.pcm_discontinuities.fetch_add(1, Ordering::Relaxed);
    }

    /// Publishes the current count of PCM blocks in the active stream epoch.
    pub fn set_queued_pcm_blocks(&self, blocks: u16) {
        self.queued_pcm_blocks.store(blocks, Ordering::Relaxed);
        self.queued_pcm_high_water_blocks
            .fetch_max(blocks, Ordering::Relaxed);
    }

    /// Starts a fresh PCM queue diagnostic epoch after stream-local retirement.
    ///
    /// The current occupancy and high-water mark both become zero. Stream-wide
    /// counters intentionally remain cumulative.
    pub fn reset_queued_pcm_epoch(&self) {
        self.queued_pcm_blocks.store(0, Ordering::Relaxed);
        self.queued_pcm_high_water_blocks
            .store(0, Ordering::Relaxed);
    }

    /// Records a stream-local reset.
    pub fn stream_reset(&self) {
        self.stream_resets.fetch_add(1, Ordering::Relaxed);
    }

    /// Publishes the validated nominal input/output sample rates.
    ///
    /// Zero means that the stream has not published a nominal format yet.
    pub fn set_nominal_sample_rates(&self, input_sample_rate_hz: u32, output_sample_rate_hz: u32) {
        self.nominal_sample_rates.store(
            pack_sample_rates(input_sample_rate_hz, output_sample_rate_hz),
            Ordering::Relaxed,
        );
    }

    /// Publishes the currently requested bounded drift correction.
    pub fn set_drift_adjustment_ppm(&self, ppm: i32) {
        self.drift_adjustment_ppm.store(ppm, Ordering::Relaxed);
    }

    /// Records a stream-owner hard resynchronization request.
    pub fn drift_hard_resync(&self) {
        self.drift_hard_resyncs.fetch_add(1, Ordering::Relaxed);
    }

    /// Reads a non-transactional diagnostic snapshot.
    ///
    /// Counters use relaxed ordering because they describe telemetry, not
    /// synchronization. A concurrent event may appear in the next snapshot.
    #[must_use]
    pub fn snapshot(&self) -> StreamMetricsSnapshot {
        let (nominal_input_sample_rate_hz, nominal_output_sample_rate_hz) =
            unpack_sample_rates(self.nominal_sample_rates.load(Ordering::Relaxed));

        StreamMetricsSnapshot {
            authenticated_packets: self.authenticated_packets.load(Ordering::Relaxed),
            duplicate_packets: self.duplicate_packets.load(Ordering::Relaxed),
            rejected_packets: self.rejected_packets.load(Ordering::Relaxed),
            late_packets: self.late_packets.load(Ordering::Relaxed),
            reordered_packets: self.reordered_packets.load(Ordering::Relaxed),
            jitter_resync_dropped_frames: self.jitter_resync_dropped_frames.load(Ordering::Relaxed),
            plc_frames: self.plc_frames.load(Ordering::Relaxed),
            decode_errors: self.decode_errors.load(Ordering::Relaxed),
            pcm_underflows: self.pcm_underflows.load(Ordering::Relaxed),
            pcm_overflows: self.pcm_overflows.load(Ordering::Relaxed),
            pcm_discontinuities: self.pcm_discontinuities.load(Ordering::Relaxed),
            queued_pcm_blocks: self.queued_pcm_blocks.load(Ordering::Relaxed),
            queued_pcm_high_water_blocks: self.queued_pcm_high_water_blocks.load(Ordering::Relaxed),
            stream_resets: self.stream_resets.load(Ordering::Relaxed),
            nominal_input_sample_rate_hz,
            nominal_output_sample_rate_hz,
            drift_adjustment_ppm: self.drift_adjustment_ppm.load(Ordering::Relaxed),
            drift_hard_resyncs: self.drift_hard_resyncs.load(Ordering::Relaxed),
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
    /// Encoded jitter frames discarded by controlled resynchronization.
    pub jitter_resync_dropped_frames: u64,
    /// Decoder-side packet-loss-concealment frames.
    pub plc_frames: u64,
    /// Stream-local decoder or bounded decoder-output failures.
    pub decode_errors: u64,
    /// Output callbacks that emitted bounded silence.
    pub pcm_underflows: u64,
    /// Bounded oldest-region PCM drops.
    pub pcm_overflows: u64,
    /// Sequence or epoch gaps observed by the PCM render source.
    pub pcm_discontinuities: u64,
    /// Current PCM blocks belonging to the active stream epoch.
    pub queued_pcm_blocks: u16,
    /// Largest active-epoch PCM block count observed since stream creation.
    pub queued_pcm_high_water_blocks: u16,
    /// Stream-local resets.
    pub stream_resets: u64,
    /// Validated nominal input sample rate, or zero before publication.
    pub nominal_input_sample_rate_hz: u32,
    /// Validated nominal output sample rate, or zero before publication.
    pub nominal_output_sample_rate_hz: u32,
    /// Current requested bounded drift adjustment in ppm.
    pub drift_adjustment_ppm: i32,
    /// Stream-owner hard resynchronizations requested by the drift controller.
    pub drift_hard_resyncs: u64,
}

impl StreamMetricsSnapshot {
    /// Returns the nominal input-frames-per-output-frame conversion ratio.
    ///
    /// Returns `None` until the stream has published both nominal sample rates.
    #[must_use]
    pub fn nominal_resampling_ratio(&self) -> Option<f64> {
        ratio_from_rates(
            self.nominal_input_sample_rate_hz,
            self.nominal_output_sample_rate_hz,
        )
    }

    /// Returns the effective conversion ratio after the current ppm correction.
    ///
    /// Positive ppm consumes input faster. Returns `None` until the stream has
    /// published both nominal sample rates.
    #[must_use]
    pub fn effective_resampling_ratio(&self) -> Option<f64> {
        self.nominal_resampling_ratio().map(|nominal_ratio| {
            nominal_ratio * (1.0 + f64::from(self.drift_adjustment_ppm) / 1_000_000.0)
        })
    }
}

fn ratio_from_rates(input_sample_rate_hz: u32, output_sample_rate_hz: u32) -> Option<f64> {
    (input_sample_rate_hz != 0 && output_sample_rate_hz != 0)
        .then(|| f64::from(input_sample_rate_hz) / f64::from(output_sample_rate_hz))
}

const fn pack_sample_rates(input_sample_rate_hz: u32, output_sample_rate_hz: u32) -> u64 {
    ((input_sample_rate_hz as u64) << u32::BITS) | output_sample_rate_hz as u64
}

fn unpack_sample_rates(packed_sample_rates: u64) -> (u32, u32) {
    (
        (packed_sample_rates >> u32::BITS) as u32,
        u32::try_from(packed_sample_rates & u64::from(u32::MAX))
            .expect("masked sample rate fits in u32"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_atomic_counters() {
        let counters = StreamCounters::default();
        counters.authenticated();
        counters.rejected();
        counters.jitter_resync_dropped_frames(3);
        counters.plc_frame();
        counters.decode_error();
        counters.pcm_underflow();
        counters.pcm_discontinuity();
        counters.set_queued_pcm_blocks(4);
        counters.set_queued_pcm_blocks(2);
        counters.set_nominal_sample_rates(48_000, 44_100);
        counters.set_drift_adjustment_ppm(125);
        counters.drift_hard_resync();

        let snapshot = counters.snapshot();
        assert_eq!(snapshot.authenticated_packets, 1);
        assert_eq!(snapshot.rejected_packets, 1);
        assert_eq!(snapshot.jitter_resync_dropped_frames, 3);
        assert_eq!(snapshot.plc_frames, 1);
        assert_eq!(snapshot.decode_errors, 1);
        assert_eq!(snapshot.pcm_underflows, 1);
        assert_eq!(snapshot.pcm_discontinuities, 1);
        assert_eq!(snapshot.queued_pcm_blocks, 2);
        assert_eq!(snapshot.queued_pcm_high_water_blocks, 4);
        assert_eq!(snapshot.nominal_input_sample_rate_hz, 48_000);
        assert_eq!(snapshot.nominal_output_sample_rate_hz, 44_100);
        assert_eq!(snapshot.drift_adjustment_ppm, 125);
        assert_eq!(snapshot.drift_hard_resyncs, 1);
        assert_eq!(
            snapshot.nominal_resampling_ratio(),
            Some(48_000.0 / 44_100.0)
        );
        assert_eq!(
            snapshot.effective_resampling_ratio(),
            Some((48_000.0 / 44_100.0) * 1.000_125)
        );

        counters.reset_queued_pcm_epoch();
        let reset_snapshot = counters.snapshot();
        assert_eq!(reset_snapshot.queued_pcm_blocks, 0);
        assert_eq!(reset_snapshot.queued_pcm_high_water_blocks, 0);
        assert_eq!(reset_snapshot.authenticated_packets, 1);
    }

    #[test]
    fn unpublished_rates_do_not_produce_resampling_ratios() {
        let snapshot = StreamCounters::default().snapshot();

        assert_eq!(snapshot.nominal_resampling_ratio(), None);
        assert_eq!(snapshot.effective_resampling_ratio(), None);
    }

    #[test]
    fn nominal_sample_rates_are_published_as_one_snapshot_pair() {
        let counters = StreamCounters::default();

        counters.set_nominal_sample_rates(48_000, 44_100);
        let snapshot = counters.snapshot();
        assert_eq!(
            (
                snapshot.nominal_input_sample_rate_hz,
                snapshot.nominal_output_sample_rate_hz,
            ),
            (48_000, 44_100),
        );

        counters.set_nominal_sample_rates(48_000, 96_000);
        let snapshot = counters.snapshot();
        assert_eq!(
            (
                snapshot.nominal_input_sample_rate_hz,
                snapshot.nominal_output_sample_rate_hz,
            ),
            (48_000, 96_000),
        );
    }
}
