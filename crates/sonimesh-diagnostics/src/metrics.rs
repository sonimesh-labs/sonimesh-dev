use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};

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
    pcm_refill_limit_hits: AtomicU64,
    pcm_overflows: AtomicU64,
    pcm_discontinuities: AtomicU64,
    queued_pcm_state: AtomicU32,
    jitter_state: AtomicU32,
    target_water_levels: AtomicU64,
    target_smoothed_jitter_ms: AtomicU32,
    target_raised_steps: AtomicU32,
    target_lowered_steps: AtomicU32,
    stream_resets: AtomicU64,
    nominal_sample_rates: AtomicU64,
    drift_adjustment_ppm: AtomicI32,
    drift_hard_resyncs: AtomicU64,
}

impl StreamCounters {
    /// Records an authenticated packet.
    pub fn authenticated(&self) {
        saturating_increment(&self.authenticated_packets);
    }

    /// Records an authenticated duplicate.
    pub fn duplicate(&self) {
        saturating_increment(&self.duplicate_packets);
    }

    /// Records a packet rejected before it can reach decoder work.
    pub fn rejected(&self) {
        saturating_increment(&self.rejected_packets);
    }

    /// Records a packet arriving after its deadline.
    pub fn late(&self) {
        saturating_increment(&self.late_packets);
    }

    /// Records an accepted reordered packet.
    pub fn reordered(&self) {
        saturating_increment(&self.reordered_packets);
    }

    /// Records encoded frames discarded by one controlled jitter resynchronization.
    pub fn jitter_resync_dropped_frames(&self, frames: usize) {
        let frames = u64::try_from(frames).unwrap_or(u64::MAX);
        saturating_add(&self.jitter_resync_dropped_frames, frames);
    }

    /// Records one decoder-side packet-loss-concealment frame.
    pub fn plc_frame(&self) {
        saturating_increment(&self.plc_frames);
    }

    /// Records one stream-local decoder or bounded decoder-output failure.
    pub fn decode_error(&self) {
        saturating_increment(&self.decode_errors);
    }

    /// Records output silence caused by an empty PCM ring.
    pub fn pcm_underflow(&self) {
        saturating_increment(&self.pcm_underflows);
    }

    /// Records one callback that exhausted its bounded PCM refill budget.
    pub fn pcm_refill_limit_hit(&self) {
        saturating_increment(&self.pcm_refill_limit_hits);
    }

    /// Records a bounded oldest-region drop in the PCM ring.
    pub fn pcm_overflow(&self) {
        saturating_increment(&self.pcm_overflows);
    }

    /// Records a sequence or epoch gap observed by the PCM render source.
    pub fn pcm_discontinuity(&self) {
        saturating_increment(&self.pcm_discontinuities);
    }

    /// Publishes the current count of PCM blocks in the active stream epoch.
    pub fn set_queued_pcm_blocks(&self, blocks: u16) {
        let _ = self
            .queued_pcm_state
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |state| {
                let (_, high_water) = unpack_queued_pcm_state(state);
                Some(pack_queued_pcm_state(blocks, high_water.max(blocks)))
            });
    }

    /// Starts a fresh PCM queue diagnostic epoch after stream-local retirement.
    ///
    /// The current occupancy and high-water mark both become zero. Stream-wide
    /// counters intentionally remain cumulative.
    pub fn reset_queued_pcm_epoch(&self) {
        self.queued_pcm_state.store(0, Ordering::Relaxed);
    }

    /// Publishes the current encoded-frame count in the stream jitter buffer.
    ///
    /// The high-water mark never decreases within an active jitter epoch.
    pub fn set_jitter_frames(&self, frames: u16) {
        let _ = self
            .jitter_state
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |state| {
                let (_, high_water) = unpack_jitter_state(state);
                Some(pack_jitter_state(frames, high_water.max(frames)))
            });
    }

    /// Starts a fresh jitter water-level diagnostic epoch after a stream-local
    /// retirement.
    ///
    /// The current occupancy and high-water mark both become zero. Stream-wide
    /// counters intentionally remain cumulative.
    pub fn reset_jitter_epoch(&self) {
        self.jitter_state.store(0, Ordering::Relaxed);
    }

    /// Publishes the bounded adaptive-target water levels and step counts.
    ///
    /// The minimum and maximum are fixed by the controller configuration and
    /// the target moves within them. The three water levels are stored as one
    /// atomic set, so a concurrent snapshot always reads a coherent triple
    /// (`min <= target <= max`). The smoothed jitter and the raised/lowered
    /// step counters are cumulative telemetry and may briefly span an epoch
    /// transition in a snapshot.
    pub fn publish_target_state(
        &self,
        min_frames: u16,
        max_frames: u16,
        target_frames: u16,
        smoothed_jitter_ms: u32,
        raised_steps: u32,
        lowered_steps: u32,
    ) {
        self.target_water_levels.store(
            pack_target_water_levels(min_frames, max_frames, target_frames),
            Ordering::Relaxed,
        );
        self.target_smoothed_jitter_ms
            .store(smoothed_jitter_ms, Ordering::Relaxed);
        self.target_raised_steps
            .store(raised_steps, Ordering::Relaxed);
        self.target_lowered_steps
            .store(lowered_steps, Ordering::Relaxed);
    }

    /// Starts a fresh adaptive-target epoch after a stream-local retirement.
    ///
    /// Every published value becomes zero until the pipeline republishes the
    /// controller state on the next observation.
    pub fn reset_target_state(&self) {
        self.publish_target_state(0, 0, 0, 0, 0, 0);
    }

    /// Records a stream-local reset.
    pub fn stream_reset(&self) {
        saturating_increment(&self.stream_resets);
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
        saturating_increment(&self.drift_hard_resyncs);
    }

    /// Reads a non-transactional diagnostic snapshot.
    ///
    /// Counters use relaxed ordering because they describe telemetry, not
    /// synchronization. A concurrent event may appear in the next snapshot.
    #[must_use]
    pub fn snapshot(&self) -> StreamMetricsSnapshot {
        let (queued_pcm_blocks, queued_pcm_high_water_blocks) =
            unpack_queued_pcm_state(self.queued_pcm_state.load(Ordering::Relaxed));
        let (nominal_input_sample_rate_hz, nominal_output_sample_rate_hz) =
            unpack_sample_rates(self.nominal_sample_rates.load(Ordering::Relaxed));
        let (jitter_frames, jitter_high_water_frames) =
            unpack_jitter_state(self.jitter_state.load(Ordering::Relaxed));
        let (target_min_frames, target_max_frames, target_frames) =
            unpack_target_water_levels(self.target_water_levels.load(Ordering::Relaxed));

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
            pcm_refill_limit_hits: self.pcm_refill_limit_hits.load(Ordering::Relaxed),
            pcm_overflows: self.pcm_overflows.load(Ordering::Relaxed),
            pcm_discontinuities: self.pcm_discontinuities.load(Ordering::Relaxed),
            queued_pcm_blocks,
            queued_pcm_high_water_blocks,
            jitter_frames,
            jitter_high_water_frames,
            target_min_frames,
            target_max_frames,
            target_frames,
            target_smoothed_jitter_ms: self.target_smoothed_jitter_ms.load(Ordering::Relaxed),
            target_raised_steps: self.target_raised_steps.load(Ordering::Relaxed),
            target_lowered_steps: self.target_lowered_steps.load(Ordering::Relaxed),
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
    /// Output callbacks that exhausted the bounded PCM refill budget.
    pub pcm_refill_limit_hits: u64,
    /// Bounded oldest-region PCM drops.
    pub pcm_overflows: u64,
    /// Sequence or epoch gaps observed by the PCM render source.
    pub pcm_discontinuities: u64,
    /// Current PCM blocks belonging to the active stream epoch.
    pub queued_pcm_blocks: u16,
    /// Largest active-epoch PCM block count observed since stream creation.
    pub queued_pcm_high_water_blocks: u16,
    /// Encoded frames currently buffered in the stream jitter buffer.
    pub jitter_frames: u16,
    /// Largest active-epoch jitter-buffer frame count observed since stream creation.
    pub jitter_high_water_frames: u16,
    /// Adaptive-target minimum water level of the active epoch, in frames.
    pub target_min_frames: u16,
    /// Adaptive-target maximum water level of the active epoch, in frames.
    pub target_max_frames: u16,
    /// Current adaptive-target water level of the active epoch, in frames.
    pub target_frames: u16,
    /// Smoothed packet-arrival-jitter estimate of the active epoch, in ms.
    pub target_smoothed_jitter_ms: u32,
    /// Target-raising steps taken in the active epoch.
    pub target_raised_steps: u32,
    /// Target-lowering steps taken in the active epoch.
    pub target_lowered_steps: u32,
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

fn saturating_increment(counter: &AtomicU64) {
    saturating_add(counter, 1);
}

fn saturating_add(counter: &AtomicU64, amount: u64) {
    if amount == 0 {
        return;
    }
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        if value == u64::MAX {
            None
        } else {
            Some(value.saturating_add(amount))
        }
    });
}

fn ratio_from_rates(input_sample_rate_hz: u32, output_sample_rate_hz: u32) -> Option<f64> {
    (input_sample_rate_hz != 0 && output_sample_rate_hz != 0)
        .then(|| f64::from(input_sample_rate_hz) / f64::from(output_sample_rate_hz))
}

const fn pack_queued_pcm_state(blocks: u16, high_water_blocks: u16) -> u32 {
    (blocks as u32) | ((high_water_blocks as u32) << u16::BITS)
}

fn unpack_queued_pcm_state(state: u32) -> (u16, u16) {
    let blocks =
        u16::try_from(state & u32::from(u16::MAX)).expect("masked PCM queue level fits in u16");
    let high_water_blocks =
        u16::try_from(state >> u16::BITS).expect("shifted PCM queue high-water fits in u16");
    (blocks, high_water_blocks)
}

const fn pack_jitter_state(frames: u16, high_water_frames: u16) -> u32 {
    (frames as u32) | ((high_water_frames as u32) << u16::BITS)
}

fn unpack_jitter_state(state: u32) -> (u16, u16) {
    let frames =
        u16::try_from(state & u32::from(u16::MAX)).expect("masked jitter level fits in u16");
    let high_water_frames =
        u16::try_from(state >> u16::BITS).expect("shifted jitter high-water fits in u16");
    (frames, high_water_frames)
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

fn pack_target_water_levels(min_frames: u16, max_frames: u16, target_frames: u16) -> u64 {
    u64::from(min_frames)
        | (u64::from(max_frames) << u16::BITS)
        | (u64::from(target_frames) << (u16::BITS * 2))
}

fn unpack_target_water_levels(packed: u64) -> (u16, u16, u16) {
    let min_frames =
        u16::try_from(packed & u64::from(u16::MAX)).expect("masked target minimum fits in u16");
    let max_frames = u16::try_from((packed >> u16::BITS) & u64::from(u16::MAX))
        .expect("shifted target maximum fits in u16");
    let target_frames = u16::try_from((packed >> (u16::BITS * 2)) & u64::from(u16::MAX))
        .expect("shifted target water level fits in u16");
    (min_frames, max_frames, target_frames)
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
        counters.pcm_refill_limit_hit();
        counters.pcm_discontinuity();
        counters.set_queued_pcm_blocks(4);
        counters.set_queued_pcm_blocks(2);
        counters.set_jitter_frames(4);
        counters.set_jitter_frames(2);
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
        assert_eq!(snapshot.pcm_refill_limit_hits, 1);
        assert_eq!(snapshot.pcm_discontinuities, 1);
        assert_eq!(snapshot.queued_pcm_blocks, 2);
        assert_eq!(snapshot.queued_pcm_high_water_blocks, 4);
        assert_eq!(snapshot.jitter_frames, 2);
        assert_eq!(snapshot.jitter_high_water_frames, 4);
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
        counters.reset_jitter_epoch();
        let reset_snapshot = counters.snapshot();
        assert_eq!(reset_snapshot.queued_pcm_blocks, 0);
        assert_eq!(reset_snapshot.queued_pcm_high_water_blocks, 0);
        assert_eq!(reset_snapshot.jitter_frames, 0);
        assert_eq!(reset_snapshot.jitter_high_water_frames, 0);
        assert_eq!(reset_snapshot.authenticated_packets, 1);
    }

    #[test]
    fn cumulative_counters_saturate_instead_of_wrapping() {
        let counters = StreamCounters::default();
        for counter in [
            &counters.authenticated_packets,
            &counters.duplicate_packets,
            &counters.rejected_packets,
            &counters.late_packets,
            &counters.reordered_packets,
            &counters.jitter_resync_dropped_frames,
            &counters.plc_frames,
            &counters.decode_errors,
            &counters.pcm_underflows,
            &counters.pcm_refill_limit_hits,
            &counters.pcm_overflows,
            &counters.pcm_discontinuities,
            &counters.stream_resets,
            &counters.drift_hard_resyncs,
        ] {
            counter.store(u64::MAX - 1, Ordering::Relaxed);
        }

        counters.authenticated();
        counters.authenticated();
        counters.duplicate();
        counters.duplicate();
        counters.rejected();
        counters.rejected();
        counters.late();
        counters.late();
        counters.reordered();
        counters.reordered();
        counters.jitter_resync_dropped_frames(2);
        counters.plc_frame();
        counters.plc_frame();
        counters.decode_error();
        counters.decode_error();
        counters.pcm_underflow();
        counters.pcm_underflow();
        counters.pcm_refill_limit_hit();
        counters.pcm_refill_limit_hit();
        counters.pcm_overflow();
        counters.pcm_overflow();
        counters.pcm_discontinuity();
        counters.pcm_discontinuity();
        counters.stream_reset();
        counters.stream_reset();
        counters.drift_hard_resync();
        counters.drift_hard_resync();

        assert_eq!(
            counters.snapshot(),
            StreamMetricsSnapshot {
                authenticated_packets: u64::MAX,
                duplicate_packets: u64::MAX,
                rejected_packets: u64::MAX,
                late_packets: u64::MAX,
                reordered_packets: u64::MAX,
                jitter_resync_dropped_frames: u64::MAX,
                plc_frames: u64::MAX,
                decode_errors: u64::MAX,
                pcm_underflows: u64::MAX,
                pcm_refill_limit_hits: u64::MAX,
                pcm_overflows: u64::MAX,
                pcm_discontinuities: u64::MAX,
                stream_resets: u64::MAX,
                drift_hard_resyncs: u64::MAX,
                ..StreamMetricsSnapshot::default()
            }
        );
    }

    #[test]
    fn concurrent_counter_updates_remain_saturated() {
        let counters = std::sync::Arc::new(StreamCounters::default());
        counters
            .authenticated_packets
            .store(u64::MAX - 100, Ordering::Relaxed);
        let workers = (0..4)
            .map(|_| {
                let counters = std::sync::Arc::clone(&counters);
                std::thread::spawn(move || {
                    for _ in 0..10_000 {
                        counters.authenticated();
                    }
                })
            })
            .collect::<Vec<_>>();

        for worker in workers {
            worker.join().expect("counter writer must finish");
        }
        assert_eq!(counters.snapshot().authenticated_packets, u64::MAX);
    }

    #[test]
    fn unpublished_or_partial_rates_do_not_produce_resampling_ratios() {
        let counters = StreamCounters::default();
        counters.set_drift_adjustment_ppm(125);
        let snapshot = counters.snapshot();
        assert_eq!(snapshot.nominal_resampling_ratio(), None);
        assert_eq!(snapshot.effective_resampling_ratio(), None);

        counters.set_nominal_sample_rates(48_000, 0);
        let snapshot = counters.snapshot();
        assert_eq!(snapshot.nominal_resampling_ratio(), None);
        assert_eq!(snapshot.effective_resampling_ratio(), None);

        counters.set_nominal_sample_rates(0, 44_100);
        let snapshot = counters.snapshot();
        assert_eq!(snapshot.nominal_resampling_ratio(), None);
        assert_eq!(snapshot.effective_resampling_ratio(), None);
    }

    #[test]
    fn queued_pcm_level_and_high_water_are_published_as_one_valid_pair() {
        let counters = StreamCounters::default();

        for blocks in [4, 2, 9, 1, u16::MAX] {
            counters.set_queued_pcm_blocks(blocks);
            let snapshot = counters.snapshot();
            assert_eq!(snapshot.queued_pcm_blocks, blocks);
            assert!(snapshot.queued_pcm_high_water_blocks >= snapshot.queued_pcm_blocks);
        }
    }

    #[test]
    fn concurrent_queue_publication_never_exposes_an_invalid_pair() {
        let counters = std::sync::Arc::new(StreamCounters::default());
        let writer = std::sync::Arc::clone(&counters);

        let writer = std::thread::spawn(move || {
            for blocks in (0..=u16::MAX).cycle().take(100_000) {
                writer.set_queued_pcm_blocks(blocks);
            }
        });
        for _ in 0..100_000 {
            let snapshot = counters.snapshot();
            assert!(snapshot.queued_pcm_high_water_blocks >= snapshot.queued_pcm_blocks);
        }
        writer.join().expect("queue metric writer must finish");
    }

    #[test]
    fn jitter_level_and_high_water_are_published_as_one_valid_pair() {
        let counters = StreamCounters::default();

        for frames in [4, 2, 9, 1, u16::MAX] {
            counters.set_jitter_frames(frames);
            let snapshot = counters.snapshot();
            assert_eq!(snapshot.jitter_frames, frames);
            assert!(snapshot.jitter_high_water_frames >= snapshot.jitter_frames);
        }
    }

    #[test]
    fn concurrent_jitter_publication_never_exposes_an_invalid_pair() {
        let counters = std::sync::Arc::new(StreamCounters::default());
        let writer = std::sync::Arc::clone(&counters);

        let writer = std::thread::spawn(move || {
            for frames in (0..=u16::MAX).cycle().take(100_000) {
                writer.set_jitter_frames(frames);
            }
        });
        for _ in 0..100_000 {
            let snapshot = counters.snapshot();
            assert!(snapshot.jitter_high_water_frames >= snapshot.jitter_frames);
        }
        writer.join().expect("jitter metric writer must finish");
    }

    #[test]
    fn adaptive_target_state_is_published_and_retired_as_one_epoch() {
        let counters = StreamCounters::default();
        counters.publish_target_state(2, 8, 4, 35, 3, 1);
        let snapshot = counters.snapshot();
        assert_eq!(
            (
                snapshot.target_min_frames,
                snapshot.target_max_frames,
                snapshot.target_frames,
                snapshot.target_smoothed_jitter_ms,
                snapshot.target_raised_steps,
                snapshot.target_lowered_steps,
            ),
            (2, 8, 4, 35, 3, 1)
        );

        counters.reset_target_state();
        let snapshot = counters.snapshot();
        assert_eq!(
            (
                snapshot.target_min_frames,
                snapshot.target_max_frames,
                snapshot.target_frames,
                snapshot.target_smoothed_jitter_ms,
                snapshot.target_raised_steps,
                snapshot.target_lowered_steps,
            ),
            (0, 0, 0, 0, 0, 0)
        );
    }

    #[test]
    fn target_water_levels_round_trip_across_the_full_u16_range() {
        for min_frames in [0, 1, u16::MAX / 3, u16::MAX / 2, u16::MAX - 1, u16::MAX] {
            for max_frames in [0, 1, u16::MAX / 3, u16::MAX / 2, u16::MAX - 1, u16::MAX] {
                for target_frames in [0, 1, u16::MAX / 3, u16::MAX / 2, u16::MAX - 1, u16::MAX] {
                    let packed = pack_target_water_levels(min_frames, max_frames, target_frames);
                    assert_eq!(
                        unpack_target_water_levels(packed),
                        (min_frames, max_frames, target_frames)
                    );
                }
            }
        }
    }

    #[test]
    fn concurrent_target_water_levels_are_read_as_one_coherent_triple() {
        let counters = std::sync::Arc::new(StreamCounters::default());
        let writer = std::sync::Arc::clone(&counters);

        // The two epochs are chosen so that the newer minimum exceeds the
        // older maximum: independent stores could expose `min > max`, while
        // the single packed store cannot.
        // The initial default state is a coherent zero triple.
        let epochs = [
            (0_u16, 0_u16, 0_u16),
            (1_u16, 3_u16, 2_u16),
            (5_u16, 7_u16, 6_u16),
        ];
        let writer = std::thread::spawn(move || {
            for frame in 0..100_000 {
                let (min, max, target) = epochs[frame % epochs.len()];
                writer.publish_target_state(min, max, target, u32::try_from(frame).unwrap(), 1, 0);
            }
        });
        for _ in 0..100_000 {
            let snapshot = counters.snapshot();
            let (min, max, target) = (
                snapshot.target_min_frames,
                snapshot.target_max_frames,
                snapshot.target_frames,
            );
            assert!(
                min <= target && target <= max,
                "water levels must satisfy min <= target <= max in any snapshot"
            );
            assert!(
                epochs.contains(&(min, max, target)),
                "water levels must be read as one coherent epoch, got {min}/{max}/{target}"
            );
        }
        writer.join().expect("target metric writer must finish");
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

    #[test]
    fn queued_pcm_state_round_trips_across_the_full_u16_range() {
        for blocks in [0, 1, u16::MAX / 3, u16::MAX / 2, u16::MAX - 1, u16::MAX] {
            for high_water_blocks in [0, 1, u16::MAX / 3, u16::MAX / 2, u16::MAX - 1, u16::MAX] {
                let packed = pack_queued_pcm_state(blocks, high_water_blocks);
                assert_eq!(unpack_queued_pcm_state(packed), (blocks, high_water_blocks));
            }
        }
    }

    #[test]
    fn queued_pcm_state_packs_level_low_and_high_water_high() {
        let packed = pack_queued_pcm_state(0x1234, 0xABCD);
        assert_eq!(packed & u32::from(u16::MAX), 0x1234);
        assert_eq!(packed >> u16::BITS, 0xABCD);
    }

    #[test]
    fn jitter_state_round_trips_across_the_full_u16_range() {
        for frames in [0, 1, u16::MAX / 3, u16::MAX / 2, u16::MAX - 1, u16::MAX] {
            for high_water_frames in [0, 1, u16::MAX / 3, u16::MAX / 2, u16::MAX - 1, u16::MAX] {
                let packed = pack_jitter_state(frames, high_water_frames);
                assert_eq!(unpack_jitter_state(packed), (frames, high_water_frames));
            }
        }
    }

    #[test]
    fn jitter_state_packs_level_low_and_high_water_high() {
        let packed = pack_jitter_state(0x1234, 0xABCD);
        assert_eq!(packed & u32::from(u16::MAX), 0x1234);
        assert_eq!(packed >> u16::BITS, 0xABCD);
    }

    #[test]
    fn sample_rates_round_trip_across_representative_values() {
        for input_sample_rate_hz in [0, 1, 8_000, 48_000, 192_000, u32::MAX] {
            for output_sample_rate_hz in [0, 1, 44_100, 48_000, 192_000, u32::MAX] {
                let packed = pack_sample_rates(input_sample_rate_hz, output_sample_rate_hz);
                assert_eq!(
                    unpack_sample_rates(packed),
                    (input_sample_rate_hz, output_sample_rate_hz)
                );
            }
        }
    }

    #[test]
    fn sample_rates_pack_input_high_and_output_low() {
        let packed = pack_sample_rates(0x1234_5678, 0x9ABC_DEF0);
        assert_eq!(packed >> u32::BITS, 0x1234_5678);
        assert_eq!(packed & u64::from(u32::MAX), 0x9ABC_DEF0);
    }

    #[test]
    fn unpack_helpers_decode_arbitrary_bit_patterns_without_panicking() {
        // Every unpack helper masks before the fallible conversion, so any
        // stored bit pattern must decode without panicking and must equal the
        // explicit mask-and-shift reconstruction.
        for state in [
            0x0000_0000,
            0x0000_8000,
            0x8000_0000,
            0xDEAD_BEEF,
            0xFFFF_FFFF,
        ] {
            let (blocks, high_water_blocks) = unpack_queued_pcm_state(state);
            assert_eq!(u32::from(blocks), state & u32::from(u16::MAX));
            assert_eq!(u32::from(high_water_blocks), state >> u16::BITS);

            let (frames, high_water_frames) = unpack_jitter_state(state);
            assert_eq!(u32::from(frames), state & u32::from(u16::MAX));
            assert_eq!(u32::from(high_water_frames), state >> u16::BITS);
        }
        for packed in [0x0000_0000_0000_0000, 0xDEAD_BEEF_CAFE_F00D, u64::MAX] {
            let (input_sample_rate_hz, output_sample_rate_hz) = unpack_sample_rates(packed);
            assert_eq!(u64::from(input_sample_rate_hz), packed >> u32::BITS);
            assert_eq!(
                u64::from(output_sample_rate_hz),
                packed & u64::from(u32::MAX)
            );

            let (min_frames, max_frames, target_frames) = unpack_target_water_levels(packed);
            assert_eq!(u64::from(min_frames), packed & u64::from(u16::MAX));
            assert_eq!(
                u64::from(max_frames),
                (packed >> u16::BITS) & u64::from(u16::MAX)
            );
            assert_eq!(
                u64::from(target_frames),
                (packed >> (u16::BITS * 2)) & u64::from(u16::MAX)
            );
        }
    }

    #[test]
    fn ratio_from_rates_returns_none_for_zero_rates() {
        assert_eq!(ratio_from_rates(0, 44_100), None);
        assert_eq!(ratio_from_rates(48_000, 0), None);
        assert_eq!(ratio_from_rates(0, 0), None);
        assert_eq!(ratio_from_rates(48_000, 44_100), Some(48_000.0 / 44_100.0));
    }

    #[test]
    fn saturating_add_clamps_at_u64_max() {
        let counter = AtomicU64::new(0);
        saturating_add(&counter, 5);
        saturating_add(&counter, 0);
        assert_eq!(counter.load(Ordering::Relaxed), 5);

        let saturated = AtomicU64::new(u64::MAX);
        saturating_add(&saturated, 1);
        assert_eq!(saturated.load(Ordering::Relaxed), u64::MAX);

        let near_max = AtomicU64::new(u64::MAX - 1);
        saturating_add(&near_max, 2);
        assert_eq!(near_max.load(Ordering::Relaxed), u64::MAX);

        let incremented = AtomicU64::new(0);
        saturating_increment(&incremented);
        assert_eq!(incremented.load(Ordering::Relaxed), 1);
    }
}
