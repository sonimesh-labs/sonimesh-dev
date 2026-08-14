//! Deterministic fixed-slot mixer membership over a shared virtual link.
//!
//! ADR 0015's Phase 2 mixer is a fixed-slot realtime bus whose membership is
//! published through a bounded lock-free queue and applied by the callback at
//! the start of its next render. This scenario drives the real
//! [`CommandDrivenMixer`] boundary from a multi-stream virtual-network run:
//! every stream's playout is decoded to a deterministic finite PCM block and
//! pushed into that stream's mixer slot, the callback renders the bus every
//! tick, and a mid-session remove/reinsert exercises membership churn over
//! the shared link. The report carries shared link counters, per-stream
//! jitter/PLC counters, per-slot mixer counters, and the membership
//! transitions, proving the bus stays bounded and finite and that a starved
//! slot never blocks the other slots. It makes no physical latency claim.

use std::fmt;
use std::sync::Arc;

use sonimesh_audio::{
    CommandDrivenMixer, EncodedFrame, GainControlHandle, InsertOutcome, JitterBuffer,
    OPUS_CHANNELS, OPUS_SAMPLE_RATE_HZ, OpusFrameConfig, PlayoutOutcome, RealtimePcmProducer,
    RealtimeQueueConfigError, realtime_pcm_queue,
};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN;

use crate::multi_stream_scenario::{MAX_MULTI_STREAM_STREAMS, MIN_MULTI_STREAM_STREAMS};
use crate::scenario::SimulationReport;
use crate::{ClockError, NetworkProfile, NetworkStats, ProfileError, VirtualClock, VirtualNetwork};

/// Hard bound on sender frames per stream.
const MAX_MIXER_MEMBERSHIP_FRAMES: u32 = 100_000;
/// Default complete media datagram size in bytes.
const DEFAULT_MEDIA_DATAGRAM_BYTES: u16 = 256;
/// Encoded-frame capacity of every stream's jitter buffer.
const JITTER_BUFFER_CAPACITY: usize = 64;
/// Default fixed slot count of the mixer membership table.
pub const DEFAULT_MIXER_MEMBERSHIP_SLOTS: usize = 8;
/// Default output-format PCM block size (20 ms at 48 kHz stereo).
pub const DEFAULT_MIXER_MEMBERSHIP_PCM_BLOCK: usize = 960;
/// Ticks between the mid-session remove and reinsert membership churn.
const CHURN_RESTART_TICKS: u64 = 20;

/// One deterministic mixer-membership simulation configuration.
///
/// `STREAMS` source streams share one link and profile while each stream
/// feeds one slot of a fixed `SLOTS`-slot [`CommandDrivenMixer`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MixerMembershipScenarioConfig<const STREAMS: usize> {
    /// Number of sender frames per stream.
    pub frames: u32,
    /// Negotiated 10 ms or 20 ms frame duration used by sender and receiver scheduling.
    pub frame_duration_ms: u64,
    /// Fixed initial playout delay applied to every stream.
    pub target_buffer_ms: u64,
    /// Complete on-wire media datagram bytes, including header and authentication tag.
    pub media_datagram_bytes: u16,
    /// Shared virtual link impairments applied to every stream.
    pub network: NetworkProfile,
}

impl<const STREAMS: usize> Default for MixerMembershipScenarioConfig<STREAMS> {
    fn default() -> Self {
        Self {
            frames: 500,
            frame_duration_ms: 20,
            target_buffer_ms: 60,
            media_datagram_bytes: DEFAULT_MEDIA_DATAGRAM_BYTES,
            network: NetworkProfile::default(),
        }
    }
}

/// Deterministic mixer-bus counters for one membership simulation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MixerMembershipMixerReport<const SLOTS: usize> {
    /// Fixed slot count of the mixer table.
    pub slots: usize,
    /// Streams the callback renders after the final drain.
    pub active_streams: usize,
    /// Inserts published through the membership handle.
    pub published_inserts: u32,
    /// Removes published through the membership handle.
    pub published_removes: u32,
    /// Output callbacks rendered.
    pub renders: u32,
    /// Per-slot PCM underflow events recorded by the callback.
    pub slot_underflows: [u64; SLOTS],
    /// Per-slot PCM refill-limit events recorded by the callback.
    pub slot_refill_limit_hits: [u64; SLOTS],
    /// Per-slot discontinuity events recorded by the callback.
    pub slot_discontinuities: [u64; SLOTS],
    /// True when every rendered sample stayed finite.
    pub output_finite: bool,
}

/// One deterministic mixer-membership simulation report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MixerMembershipSimulationReport<const STREAMS: usize, const SLOTS: usize> {
    /// Shared virtual link counters.
    pub network: NetworkStats,
    /// Per-stream jitter and PLC counters.
    pub streams: [SimulationReport; STREAMS],
    /// Mixer-bus and membership counters.
    pub mixer: MixerMembershipMixerReport<SLOTS>,
}

/// Invalid simulation configuration or virtual time operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MixerMembershipScenarioError {
    /// Stream count must be between the shared multi-stream bound.
    InvalidStreamCount,
    /// Slot count must be at least the stream count and non-zero.
    InvalidSlotCount,
    /// Frame count must be between one and the scenario hard bound.
    InvalidFrameCount,
    /// Frame duration must be one negotiated version-one value: 10 ms or 20 ms.
    InvalidFrameDuration,
    /// Initial playout target must be no more than one second.
    InvalidTargetBuffer,
    /// Complete media datagram size must fit the version-one packet limit.
    InvalidMediaDatagramBytes,
    /// Network profile is invalid.
    Profile(ProfileError),
    /// Virtual time overflowed.
    Clock(ClockError),
    /// A bounded mixer command queue rejected a membership change.
    CommandQueueFull,
    /// A compile-time simulation capacity contradicted its runtime use.
    InternalCapacityInvariant,
}

impl fmt::Display for MixerMembershipScenarioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidStreamCount => formatter
                .write_str("invalid mixer scenario: stream count is outside the explicit bound"),
            Self::InvalidSlotCount => {
                formatter.write_str("invalid mixer scenario: slot count is below the stream count")
            }
            Self::InvalidFrameCount => formatter
                .write_str("invalid mixer scenario: frame count is outside the explicit bound"),
            Self::InvalidFrameDuration => formatter.write_str(
                "invalid mixer scenario: frame duration is not a negotiated 10 ms or 20 ms value",
            ),
            Self::InvalidTargetBuffer => formatter.write_str(
                "invalid mixer scenario: initial playout target is above the one-second bound",
            ),
            Self::InvalidMediaDatagramBytes => formatter.write_str(
                "invalid mixer scenario: media datagram size is outside the version-one bound",
            ),
            Self::Profile(error) => write!(formatter, "invalid mixer scenario: {error}"),
            Self::Clock(error) => write!(formatter, "invalid mixer scenario: {error}"),
            Self::CommandQueueFull => formatter.write_str(
                "invalid mixer scenario: the bounded membership command queue rejected a change",
            ),
            Self::InternalCapacityInvariant => formatter.write_str(
                "invalid mixer scenario: a compile-time capacity contradicted its runtime use",
            ),
        }
    }
}

impl std::error::Error for MixerMembershipScenarioError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Profile(error) => Some(error),
            Self::Clock(error) => Some(error),
            _ => None,
        }
    }
}

/// Deterministic finite samples each stream contributes when it renders.
#[allow(clippy::cast_precision_loss)]
fn finite_block<const N: usize>(stream: usize) -> [f32; N] {
    [1.0 + 0.125 * (stream as f32); N]
}

fn sample_timestamp(sequence: u32, frame: OpusFrameConfig) -> u64 {
    u64::from(sequence).saturating_mul(frame.samples_per_channel() as u64)
}

/// Runs one deterministic fixed-slot mixer membership simulation.
///
/// `STREAMS` source streams share one virtual link; each stream's playout is
/// decoded to a finite PCM block and pushed into its mixer slot. A
/// mid-session remove/reinsert exercises membership churn over the shared
/// link. The callback renders the bus every tick.
///
/// # Errors
///
/// Returns a configuration or virtual-time error when the scenario cannot be
/// run deterministically.
#[allow(clippy::too_many_lines)]
pub fn simulate_mixer_membership<
    const STREAMS: usize,
    const SLOTS: usize,
    const PCM_SAMPLES_PER_BLOCK: usize,
>(
    config: MixerMembershipScenarioConfig<STREAMS>,
) -> Result<MixerMembershipSimulationReport<STREAMS, SLOTS>, MixerMembershipScenarioError> {
    let frame = validate::<STREAMS, SLOTS>(config)?;
    let mut network = VirtualNetwork::<(u8, u32)>::new(config.network)
        .map_err(MixerMembershipScenarioError::Profile)?;
    let mut clock = VirtualClock::new();
    let mut jitters: [JitterBuffer<JITTER_BUFFER_CAPACITY, 8>; STREAMS] =
        std::array::from_fn(|_| JitterBuffer::<JITTER_BUFFER_CAPACITY, 8>::with_start_sequence(0));
    let mut streams = std::array::from_fn(|_| SimulationReport::default());
    let mut next_send_sequence = [0_u32; STREAMS];
    let mut next_send_ms = [0_u64; STREAMS];
    let mut next_playout_ms = [config.target_buffer_ms; STREAMS];
    let mut playout_decisions = [0_u32; STREAMS];

    let mut mixer = CommandDrivenMixer::<SLOTS, PCM_SAMPLES_PER_BLOCK>::new()
        .map_err(|_| MixerMembershipScenarioError::InternalCapacityInvariant)?;
    let membership = mixer.membership();
    let mut producers: [Option<RealtimePcmProducer<PCM_SAMPLES_PER_BLOCK>>; STREAMS] =
        std::array::from_fn(|_| None);
    let mut slot_counters: [Option<Arc<StreamCounters>>; STREAMS] = std::array::from_fn(|_| None);
    let mut published_inserts = 0_u32;
    let mut published_removes = 0_u32;

    // Publish one activation per stream before the first render; the callback
    // assigns stream `i` to slot `i` by its lowest-free-slot FIFO policy.
    for stream in 0..STREAMS {
        let (producer, consumer) = realtime_pcm_queue::<PCM_SAMPLES_PER_BLOCK>(
            JITTER_BUFFER_CAPACITY,
        )
        .map_err(|error| match error {
            RealtimeQueueConfigError::ZeroCapacity
            | RealtimeQueueConfigError::CapacityTooLarge
            | RealtimeQueueConfigError::ZeroSamplesPerBlock => {
                MixerMembershipScenarioError::InternalCapacityInvariant
            }
        })?;
        let counters = Arc::new(StreamCounters::default());
        membership
            .insert(
                consumer,
                GainControlHandle::new(),
                Some(Arc::clone(&counters)),
            )
            .map_err(|_| MixerMembershipScenarioError::CommandQueueFull)?;
        producers[stream] = Some(producer);
        slot_counters[stream] = Some(counters);
        published_inserts += 1;
    }

    // Deterministic mid-session churn: remove stream 0's slot at the middle
    // playout tick and reinsert it a short time later.
    let churn_at_ms = config
        .target_buffer_ms
        .saturating_add(
            u64::from(config.frames.saturating_sub(1) / 2).saturating_mul(config.frame_duration_ms),
        )
        .saturating_add(config.network.base_delay_ms);
    let reinsert_at_ms = churn_at_ms.saturating_add(CHURN_RESTART_TICKS);

    let last_playout_ms = config.target_buffer_ms.saturating_add(
        u64::from(config.frames.saturating_sub(1)).saturating_mul(config.frame_duration_ms),
    );
    let end_ms = last_playout_ms
        .saturating_add(config.network.base_delay_ms)
        .saturating_add(config.network.jitter_ms)
        .saturating_add(config.network.reorder_delay_ms)
        .saturating_add(2);

    let mut renders = 0_u32;
    let mut output_finite = true;
    let mut removed_slot: Option<usize> = None;
    while clock.now_ms() <= end_ms {
        let now_ms = clock.now_ms();
        for stream in 0..STREAMS {
            if next_send_sequence[stream] < config.frames && now_ms == next_send_ms[stream] {
                let tag = u8::try_from(stream)
                    .map_err(|_| MixerMembershipScenarioError::InternalCapacityInvariant)?;
                network.send(
                    now_ms,
                    (tag, next_send_sequence[stream]),
                    u64::from(config.media_datagram_bytes),
                );
                next_send_sequence[stream] += 1;
                next_send_ms[stream] =
                    next_send_ms[stream].saturating_add(config.frame_duration_ms);
            }
        }

        for delivery in network.drain_ready(now_ms) {
            let (tag, sequence) = delivery.payload;
            let stream = usize::from(tag);
            let encoded = EncodedFrame::new(sequence, sample_timestamp(sequence, frame), &[0])
                .map_err(|_| MixerMembershipScenarioError::InternalCapacityInvariant)?;
            match jitters[stream].insert(encoded) {
                InsertOutcome::Inserted => {}
                InsertOutcome::Reordered => streams[stream].reordered_arrivals += 1,
                InsertOutcome::Duplicate => streams[stream].duplicate_frames += 1,
                InsertOutcome::Late => streams[stream].late_frames += 1,
                InsertOutcome::Resynchronized { .. } => streams[stream].resynchronizations += 1,
                InsertOutcome::SequenceExhausted | InsertOutcome::Disabled => {
                    return Err(MixerMembershipScenarioError::InternalCapacityInvariant);
                }
            }
        }

        if now_ms == churn_at_ms {
            let slot = removed_slot.get_or_insert(0);
            membership
                .remove(*slot)
                .map_err(|_| MixerMembershipScenarioError::CommandQueueFull)?;
            published_removes += 1;
            producers[0] = None;
        }
        if now_ms == reinsert_at_ms {
            let (producer, consumer) = realtime_pcm_queue::<PCM_SAMPLES_PER_BLOCK>(
                JITTER_BUFFER_CAPACITY,
            )
            .map_err(|error| match error {
                RealtimeQueueConfigError::ZeroCapacity
                | RealtimeQueueConfigError::CapacityTooLarge
                | RealtimeQueueConfigError::ZeroSamplesPerBlock => {
                    MixerMembershipScenarioError::InternalCapacityInvariant
                }
            })?;
            let counters = Arc::new(StreamCounters::default());
            membership
                .insert(
                    consumer,
                    GainControlHandle::new(),
                    Some(Arc::clone(&counters)),
                )
                .map_err(|_| MixerMembershipScenarioError::CommandQueueFull)?;
            producers[0] = Some(producer);
            slot_counters[0] = Some(counters);
            published_inserts += 1;
        }

        for stream in 0..STREAMS {
            if playout_decisions[stream] < config.frames && now_ms == next_playout_ms[stream] {
                match jitters[stream].take_next() {
                    PlayoutOutcome::Frame(_) => {
                        streams[stream].playout_frames += 1;
                        if let Some(producer) = producers[stream].as_mut() {
                            let _ = producer.push(&finite_block::<PCM_SAMPLES_PER_BLOCK>(stream));
                        }
                    }
                    PlayoutOutcome::Missing { .. } => streams[stream].plc_frames += 1,
                    PlayoutOutcome::NotReady => {
                        // Playout is pre-armed at the negotiated sequence, so a
                        // playout before the first frame arrives conceals via PLC;
                        // reaching NotReady here means the wiring lost the start sequence.
                        return Err(MixerMembershipScenarioError::InternalCapacityInvariant);
                    }
                }
                playout_decisions[stream] += 1;
                next_playout_ms[stream] =
                    next_playout_ms[stream].saturating_add(config.frame_duration_ms);
            }
        }

        let mut output = [0.0f32; PCM_SAMPLES_PER_BLOCK];
        mixer.render(&mut output);
        renders += 1;
        if !output.iter().all(|sample| sample.is_finite()) {
            output_finite = false;
        }
        clock
            .advance_by(1)
            .map_err(MixerMembershipScenarioError::Clock)?;
    }

    let mut slot_underflows = [0_u64; SLOTS];
    let mut slot_refill_limit_hits = [0_u64; SLOTS];
    let mut slot_discontinuities = [0_u64; SLOTS];
    for (stream, counters) in slot_counters.iter().enumerate() {
        if let Some(counters) = counters {
            let snapshot = counters.snapshot();
            slot_underflows[stream] = snapshot.pcm_underflows;
            slot_refill_limit_hits[stream] = snapshot.pcm_refill_limit_hits;
            slot_discontinuities[stream] = snapshot.pcm_discontinuities;
        }
    }

    Ok(MixerMembershipSimulationReport {
        network: network.stats(),
        streams,
        mixer: MixerMembershipMixerReport {
            slots: SLOTS,
            active_streams: mixer.active(),
            published_inserts,
            published_removes,
            renders,
            slot_underflows,
            slot_refill_limit_hits,
            slot_discontinuities,
            output_finite,
        },
    })
}

fn validate<const STREAMS: usize, const SLOTS: usize>(
    config: MixerMembershipScenarioConfig<STREAMS>,
) -> Result<OpusFrameConfig, MixerMembershipScenarioError> {
    if !(MIN_MULTI_STREAM_STREAMS..=MAX_MULTI_STREAM_STREAMS).contains(&STREAMS) {
        return Err(MixerMembershipScenarioError::InvalidStreamCount);
    }
    if SLOTS == 0 || SLOTS < STREAMS {
        return Err(MixerMembershipScenarioError::InvalidSlotCount);
    }
    if config.frames == 0 || config.frames > MAX_MIXER_MEMBERSHIP_FRAMES {
        return Err(MixerMembershipScenarioError::InvalidFrameCount);
    }
    let frame_duration_ms = u16::try_from(config.frame_duration_ms)
        .map_err(|_| MixerMembershipScenarioError::InvalidFrameDuration)?;
    let frame = OpusFrameConfig::new(OPUS_SAMPLE_RATE_HZ, OPUS_CHANNELS, frame_duration_ms)
        .map_err(|_| MixerMembershipScenarioError::InvalidFrameDuration)?;
    if config.target_buffer_ms > 1_000 {
        return Err(MixerMembershipScenarioError::InvalidTargetBuffer);
    }
    if config.media_datagram_bytes == 0
        || usize::from(config.media_datagram_bytes) > MAX_MEDIA_DATAGRAM_LEN
    {
        return Err(MixerMembershipScenarioError::InvalidMediaDatagramBytes);
    }
    config
        .network
        .validate()
        .map_err(MixerMembershipScenarioError::Profile)?;
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use crate::{ClockError, MAX_BANDWIDTH_BYTES_PER_SECOND, ProfileError};

    use super::*;

    const SLOTS: usize = 4;
    const PCM_BLOCK: usize = 64;

    fn clean_profile() -> NetworkProfile {
        let mut network = NetworkProfile::stable(0x0A0C_2026);
        network.base_delay_ms = 0;
        network.jitter_ms = 0;
        network
    }

    #[test]
    fn stream_or_slot_count_outside_the_explicit_bound_is_rejected() {
        assert_eq!(
            simulate_mixer_membership::<0, SLOTS, PCM_BLOCK>(
                MixerMembershipScenarioConfig::<0>::default()
            ),
            Err(MixerMembershipScenarioError::InvalidStreamCount)
        );
        assert_eq!(
            simulate_mixer_membership::<1, SLOTS, PCM_BLOCK>(
                MixerMembershipScenarioConfig::<1>::default()
            ),
            Err(MixerMembershipScenarioError::InvalidStreamCount)
        );
        assert_eq!(
            simulate_mixer_membership::<5, SLOTS, PCM_BLOCK>(
                MixerMembershipScenarioConfig::<5>::default()
            ),
            Err(MixerMembershipScenarioError::InvalidStreamCount)
        );
        assert_eq!(
            simulate_mixer_membership::<2, 1, PCM_BLOCK>(
                MixerMembershipScenarioConfig::<2>::default()
            ),
            Err(MixerMembershipScenarioError::InvalidSlotCount)
        );
        assert_eq!(
            simulate_mixer_membership::<2, 0, PCM_BLOCK>(
                MixerMembershipScenarioConfig::<2>::default()
            ),
            Err(MixerMembershipScenarioError::InvalidSlotCount)
        );
    }

    #[test]
    fn clean_shared_link_plays_every_frame_into_the_mixer() {
        let config = MixerMembershipScenarioConfig {
            frames: 7,
            frame_duration_ms: 10,
            target_buffer_ms: 10,
            network: clean_profile(),
            ..MixerMembershipScenarioConfig::default()
        };

        let first = simulate_mixer_membership::<2, SLOTS, PCM_BLOCK>(config).unwrap();
        let second = simulate_mixer_membership::<2, SLOTS, PCM_BLOCK>(config).unwrap();

        assert_eq!(first, second, "the simulation must be deterministic");
        for stream in &first.streams {
            assert_eq!(stream.playout_frames, config.frames);
            assert_eq!(stream.plc_frames, 0);
        }
        assert_eq!(first.mixer.active_streams, 2);
        assert_eq!(first.mixer.slots, SLOTS);
        assert_eq!(
            first.mixer.published_inserts, 3,
            "two starts plus the reinsert"
        );
        assert_eq!(first.mixer.published_removes, 1);
        assert!(first.mixer.output_finite);
    }

    #[test]
    fn shared_link_loss_leaves_a_starved_slot_silent_without_blocking_others() {
        let mut network = NetworkProfile::stable(7);
        network.burst_interval_packets = 20;
        network.burst_length_packets = 3;
        let config = MixerMembershipScenarioConfig {
            frames: 100,
            network,
            ..MixerMembershipScenarioConfig::default()
        };

        let report = simulate_mixer_membership::<2, SLOTS, PCM_BLOCK>(config).unwrap();

        for stream in &report.streams {
            assert_eq!(stream.playout_frames + stream.plc_frames, config.frames);
        }
        let total_plc: u32 = report.streams.iter().map(|stream| stream.plc_frames).sum();
        assert!(
            total_plc > 0,
            "shared-link loss must reach the jitter buffers"
        );
        assert!(
            report
                .mixer
                .slot_underflows
                .iter()
                .any(|&underflows| underflows > 0),
            "a slot starved by the link must record PCM underflows"
        );
        assert!(
            report
                .mixer
                .slot_underflows
                .iter()
                .any(|&underflows| underflows == 0)
                || report.mixer.active_streams == 2,
            "one starved slot must not silence the whole bus"
        );
        assert!(report.mixer.output_finite);
        assert_eq!(report.mixer.active_streams, 2);
    }

    #[test]
    fn membership_churn_reinserts_stream_zero_into_the_lowest_free_slot() {
        let config = MixerMembershipScenarioConfig {
            frames: 40,
            target_buffer_ms: 20,
            network: clean_profile(),
            ..MixerMembershipScenarioConfig::default()
        };

        let report = simulate_mixer_membership::<2, SLOTS, PCM_BLOCK>(config).unwrap();

        assert_eq!(report.mixer.published_inserts, 3);
        assert_eq!(report.mixer.published_removes, 1);
        assert_eq!(report.mixer.active_streams, 2);
        assert!(
            report.mixer.slot_underflows[0] > 0,
            "the removed slot underflows while away"
        );
        assert!(report.mixer.output_finite);
    }

    #[test]
    fn rejects_invalid_scenario_configuration() {
        assert_eq!(
            simulate_mixer_membership::<2, SLOTS, PCM_BLOCK>(MixerMembershipScenarioConfig {
                frames: 0,
                ..MixerMembershipScenarioConfig::default()
            }),
            Err(MixerMembershipScenarioError::InvalidFrameCount)
        );
        assert_eq!(
            simulate_mixer_membership::<2, SLOTS, PCM_BLOCK>(MixerMembershipScenarioConfig {
                frame_duration_ms: 15,
                ..MixerMembershipScenarioConfig::default()
            }),
            Err(MixerMembershipScenarioError::InvalidFrameDuration)
        );
        assert_eq!(
            simulate_mixer_membership::<2, SLOTS, PCM_BLOCK>(MixerMembershipScenarioConfig {
                target_buffer_ms: 1_001,
                ..MixerMembershipScenarioConfig::default()
            }),
            Err(MixerMembershipScenarioError::InvalidTargetBuffer)
        );
        assert_eq!(
            simulate_mixer_membership::<2, SLOTS, PCM_BLOCK>(MixerMembershipScenarioConfig {
                media_datagram_bytes: 0,
                ..MixerMembershipScenarioConfig::default()
            }),
            Err(MixerMembershipScenarioError::InvalidMediaDatagramBytes)
        );
        let mut network = NetworkProfile::stable(3);
        network.bandwidth_bytes_per_second = MAX_BANDWIDTH_BYTES_PER_SECOND + 1;
        assert_eq!(
            simulate_mixer_membership::<2, SLOTS, PCM_BLOCK>(MixerMembershipScenarioConfig {
                network,
                ..MixerMembershipScenarioConfig::default()
            }),
            Err(MixerMembershipScenarioError::Profile(
                ProfileError::BandwidthTooLarge
            ))
        );
    }

    #[test]
    fn preserved_error_variants_expose_their_causes() {
        let profile = MixerMembershipScenarioError::Profile(ProfileError::DelayTooLarge);
        assert!(
            Error::source(&profile)
                .and_then(|source| source.downcast_ref::<ProfileError>())
                .is_some()
        );

        let clock = MixerMembershipScenarioError::Clock(ClockError::Overflow);
        assert!(
            Error::source(&clock)
                .and_then(|source| source.downcast_ref::<ClockError>())
                .is_some()
        );

        assert!(Error::source(&MixerMembershipScenarioError::InvalidStreamCount).is_none());
        assert!(Error::source(&MixerMembershipScenarioError::InternalCapacityInvariant).is_none());
    }
}
