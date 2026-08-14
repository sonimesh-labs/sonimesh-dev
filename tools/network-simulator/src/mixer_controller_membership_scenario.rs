//! Deterministic control-domain membership over a shared virtual link.
//!
//! ADR 0015's Phase 2 mixer puts membership transitions in the control
//! domain: [`MixerMembershipController`] publishes bounded commands that the
//! callback applies at the start of its next render. This scenario drives the
//! real controller, handle, and [`CommandDrivenMixer`] from a multi-stream
//! virtual-network run: every stream is admitted through the controller,
//! playout is decoded to a deterministic finite PCM block and pushed into that
//! stream's mixer slot, the callback renders the bus every tick, and a
//! mid-session remove/re-admit exercises membership churn. After every render
//! the controller projection is checked against the mixer's actual slot
//! table, so the report proves the cross-domain invariant holds under
//! shared-link loss and churn while a starved slot never blocks the others.
//! Bounded per-source gain and mute scripts are applied through each admitted
//! slot's real [`GainControlHandle`], which the scenario retains and clones
//! into every admission so a published target survives the remove/re-admit
//! churn, and the report derives bounded per-source quality ratios from the
//! run's raw counters.

use std::fmt;
use std::sync::Arc;

use sonimesh_app::{
    GainControlHandle, MixerAdmissionError, MixerMembershipController, SourcePcmFormat,
};
use sonimesh_audio::{
    CommandDrivenMixer, EncodedFrame, GainControlError, InsertOutcome, JitterBuffer,
    MIXER_GAIN_RAMP_SAMPLES, OPUS_CHANNELS, OPUS_SAMPLE_RATE_HZ, OpusFrameConfig, PlayoutOutcome,
    RealtimePcmProducer,
};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN;

use crate::multi_stream_scenario::{MAX_MULTI_STREAM_STREAMS, MIN_MULTI_STREAM_STREAMS};
use crate::scenario::SimulationReport;
use crate::{ClockError, NetworkProfile, NetworkStats, ProfileError, VirtualClock, VirtualNetwork};

/// Hard bound on sender frames per stream.
const MAX_MIXER_CONTROLLER_FRAMES: u32 = 100_000;
/// Default complete media datagram size in bytes.
const DEFAULT_MEDIA_DATAGRAM_BYTES: u16 = 256;
/// Encoded-frame capacity of every stream's jitter buffer and the controller's
/// bounded PCM queue capacity.
const JITTER_BUFFER_CAPACITY: usize = 64;
/// Default fixed slot count of the mixer membership table.
pub const DEFAULT_MIXER_CONTROLLER_SLOTS: usize = 8;
/// Default output-format PCM block size (20 ms at 48 kHz stereo).
pub const DEFAULT_MIXER_CONTROLLER_PCM_BLOCK: usize = 960;
/// Ticks between the mid-session remove and re-admit membership churn.
const CHURN_RESTART_TICKS: u64 = 20;
/// Unity gain in milli-units; per-stream gain scripts stay within this bound.
const MAX_GAIN_MILLI: u16 = 1_000;

/// Bounded per-stream gain and mute events for the controller scenario.
///
/// Each event applies once, on the playout tick where the stream has played
/// out that many frames. A `None` frame disables the event. Gain is expressed
/// in milli-units so the script stays exact and comparable. The scenario
/// retains one [`GainControlHandle`] per stream and passes a clone into every
/// admission, so a published mute or gain target also survives the mid-session
/// remove/re-admit churn, matching the handle's documented output-lifetime
/// behavior.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MixerControllerGainScript {
    /// Playout frame count after which the stream's slot is muted.
    pub mute_at_frame: Option<u32>,
    /// Playout frame count after which the stream's slot is unmuted again.
    pub unmute_at_frame: Option<u32>,
    /// Playout frame count after which the linear gain target is published.
    pub gain_at_frame: Option<u32>,
    /// Linear gain target in milli-units, bounded by `0..=1_000` (unity).
    pub gain_milli: u16,
}

/// One deterministic controller-driven mixer membership configuration.
///
/// `STREAMS` source streams share one link and profile while each stream is
/// admitted into one slot of a fixed `SLOTS`-slot [`CommandDrivenMixer`]
/// through a [`MixerMembershipController`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MixerControllerScenarioConfig<const STREAMS: usize> {
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
    /// Per-stream gain and mute script applied at playout frame boundaries.
    pub gain_scripts: [MixerControllerGainScript; STREAMS],
}

impl<const STREAMS: usize> Default for MixerControllerScenarioConfig<STREAMS> {
    fn default() -> Self {
        Self {
            frames: 500,
            frame_duration_ms: 20,
            target_buffer_ms: 60,
            media_datagram_bytes: DEFAULT_MEDIA_DATAGRAM_BYTES,
            network: NetworkProfile::default(),
            gain_scripts: [MixerControllerGainScript::default(); STREAMS],
        }
    }
}

/// Bounded per-source quality ratios for one controller-driven run.
///
/// Each field is an exact fixed-point ratio in milli-units (`0..=1_000`),
/// derived from the raw playout and per-slot counters so reports stay
/// comparable across profiles without floating-point drift. The controller
/// path uses a fixed playout target (no gate deadline), so there is no
/// forced-playout ratio.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MixerControllerStreamQuality {
    /// Decoder-side concealment frames per sender frame.
    pub plc_ratio_milli: u16,
    /// Frames arriving behind the playout sequence per sender frame.
    pub late_ratio_milli: u16,
    /// Accepted reordered arrivals per sender frame.
    pub reorder_ratio_milli: u16,
    /// Duplicate frames rejected by jitter storage per sender frame.
    pub duplicate_ratio_milli: u16,
    /// Per-slot PCM underflow renders per mixer render.
    pub underflow_ratio_milli: u16,
    /// Per-slot discontinuity renders per mixer render.
    pub discontinuity_ratio_milli: u16,
}

impl MixerControllerStreamQuality {
    /// Derives the bounded quality ratios from one run's raw counters.
    fn from_counters(
        frames: u32,
        renders: u32,
        playout: &SimulationReport,
        underflows: u64,
        discontinuities: u64,
    ) -> Self {
        Self {
            plc_ratio_milli: ratio_milli(u64::from(playout.plc_frames), u64::from(frames)),
            late_ratio_milli: ratio_milli(u64::from(playout.late_frames), u64::from(frames)),
            reorder_ratio_milli: ratio_milli(
                u64::from(playout.reordered_arrivals),
                u64::from(frames),
            ),
            duplicate_ratio_milli: ratio_milli(
                u64::from(playout.duplicate_frames),
                u64::from(frames),
            ),
            underflow_ratio_milli: ratio_milli(underflows, u64::from(renders)),
            discontinuity_ratio_milli: ratio_milli(discontinuities, u64::from(renders)),
        }
    }

    /// True when every ratio stays within the bounded `0..=1_000` range.
    #[must_use]
    pub const fn within_bounds(&self) -> bool {
        self.plc_ratio_milli <= 1_000
            && self.late_ratio_milli <= 1_000
            && self.reorder_ratio_milli <= 1_000
            && self.duplicate_ratio_milli <= 1_000
            && self.underflow_ratio_milli <= 1_000
            && self.discontinuity_ratio_milli <= 1_000
    }
}

/// One exact fixed-point ratio in milli-units, bounded by unity.
fn ratio_milli(numerator: u64, denominator: u64) -> u16 {
    if denominator == 0 {
        return 0;
    }
    let scaled = numerator.saturating_mul(1_000) / denominator;
    u16::try_from(scaled.min(1_000)).unwrap_or(1_000)
}

/// Deterministic per-stream controller-admitted mixer counters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MixerControllerStreamReport {
    /// Per-playout transport counters; the `network` field is zero.
    pub playout: SimulationReport,
    /// Whether the stream's slot is muted at the end of the simulation.
    pub muted: bool,
    /// Gain and mute events applied to the stream's slot.
    pub gain_changes: u32,
    /// Linear gain target published at the end, in milli-units.
    pub final_gain_milli: u16,
    /// Bounded per-source quality ratios derived from the run's counters.
    pub quality: MixerControllerStreamQuality,
}

/// Deterministic mixer-bus and controller-projection counters for one run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MixerControllerMixerReport<const SLOTS: usize> {
    /// Fixed slot count of the mixer table.
    pub slots: usize,
    /// Streams the callback renders after the final drain.
    pub active_streams: usize,
    /// Streams the controller projects as active after the final drain.
    pub projected_streams: usize,
    /// Inserts published through the controller.
    pub published_inserts: u32,
    /// Removes published through the controller.
    pub published_removes: u32,
    /// Output callbacks rendered.
    pub renders: u32,
    /// True when the controller projection matched the mixer's actual slot
    /// table (active count, per-slot membership, and pending commands) after
    /// every render.
    pub projection_matched: bool,
    /// Per-slot PCM underflow events recorded by the callback.
    pub slot_underflows: [u64; SLOTS],
    /// Per-slot PCM refill-limit events recorded by the callback.
    pub slot_refill_limit_hits: [u64; SLOTS],
    /// Per-slot discontinuity events recorded by the callback.
    pub slot_discontinuities: [u64; SLOTS],
    /// True when every rendered sample stayed finite.
    pub output_finite: bool,
    /// Rendered ticks after every muted stream's gain ramp converged where
    /// the bus was exactly silent.
    pub all_muted_silent_ticks: u32,
}

/// One deterministic controller-driven mixer membership report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MixerControllerSimulationReport<const STREAMS: usize, const SLOTS: usize> {
    /// Shared virtual link counters.
    pub network: NetworkStats,
    /// Per-stream jitter, PLC, gain, mute, and quality counters.
    pub streams: [MixerControllerStreamReport; STREAMS],
    /// Mixer-bus, controller-projection, and membership counters.
    pub mixer: MixerControllerMixerReport<SLOTS>,
}

/// Invalid simulation configuration, admission, or virtual time operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MixerControllerScenarioError {
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
    /// A stream admission or removal failed at the controller boundary.
    Admission(MixerAdmissionError),
    /// A compile-time simulation capacity contradicted its runtime use.
    InternalCapacityInvariant,
    /// A gain event frame must be below the configured frame count.
    InvalidGainEvent,
    /// Published gain must stay within the bounded linear range.
    InvalidGainMilli,
    /// A validated gain publication failed.
    GainControl(GainControlError),
    /// The all-muted bus was not exactly silent after the bounded ramp.
    MutedBusNotSilent,
}

impl fmt::Display for MixerControllerScenarioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidStreamCount => formatter.write_str(
                "invalid mixer controller scenario: stream count is outside the explicit bound",
            ),
            Self::InvalidSlotCount => formatter.write_str(
                "invalid mixer controller scenario: slot count is below the stream count",
            ),
            Self::InvalidFrameCount => formatter.write_str(
                "invalid mixer controller scenario: frame count is outside the explicit bound",
            ),
            Self::InvalidFrameDuration => formatter.write_str(
                "invalid mixer controller scenario: frame duration is not a negotiated 10 ms or 20 ms value",
            ),
            Self::InvalidTargetBuffer => formatter.write_str(
                "invalid mixer controller scenario: initial playout target is above the one-second bound",
            ),
            Self::InvalidMediaDatagramBytes => formatter.write_str(
                "invalid mixer controller scenario: media datagram size is outside the version-one bound",
            ),
            Self::Profile(error) => write!(formatter, "invalid mixer controller scenario: {error}"),
            Self::Clock(error) => write!(formatter, "invalid mixer controller scenario: {error}"),
            Self::Admission(error) => {
                write!(formatter, "invalid mixer controller scenario: {error}")
            }
            Self::InternalCapacityInvariant => formatter.write_str(
                "invalid mixer controller scenario: a compile-time capacity contradicted its runtime use",
            ),
            Self::InvalidGainEvent => formatter.write_str(
                "invalid mixer controller scenario: a gain event frame is outside the sender frame count",
            ),
            Self::InvalidGainMilli => formatter.write_str(
                "invalid mixer controller scenario: gain exceeds the bounded linear range",
            ),
            Self::GainControl(error) => {
                write!(formatter, "invalid mixer controller scenario: {error}")
            }
            Self::MutedBusNotSilent => formatter.write_str(
                "invalid mixer controller scenario: the all-muted bus was not exactly silent",
            ),
        }
    }
}

impl std::error::Error for MixerControllerScenarioError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Profile(error) => Some(error),
            Self::Clock(error) => Some(error),
            Self::Admission(error) => Some(error),
            Self::GainControl(error) => Some(error),
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

fn output_format() -> Result<SourcePcmFormat, MixerControllerScenarioError> {
    SourcePcmFormat::new(OPUS_SAMPLE_RATE_HZ, OPUS_CHANNELS)
        .map_err(|_| MixerControllerScenarioError::InternalCapacityInvariant)
}

/// Runs one deterministic controller-driven mixer membership simulation.
///
/// `STREAMS` source streams share one virtual link; each stream is admitted
/// through a [`MixerMembershipController`] sharing the real
/// [`CommandDrivenMixer`]'s handle. A mid-session remove/re-admit exercises
/// membership churn over the shared link. The callback renders the bus every
/// tick, and after every render the controller projection is checked against
/// the mixer's actual slot table.
///
/// # Errors
///
/// Returns a configuration, admission, or virtual-time error when the scenario
/// cannot be run deterministically.
#[allow(clippy::too_many_lines)]
pub fn simulate_mixer_controller_membership<
    const STREAMS: usize,
    const SLOTS: usize,
    const PCM_SAMPLES_PER_BLOCK: usize,
>(
    config: MixerControllerScenarioConfig<STREAMS>,
) -> Result<MixerControllerSimulationReport<STREAMS, SLOTS>, MixerControllerScenarioError> {
    let frame = validate::<STREAMS, SLOTS>(config)?;
    let format = output_format()?;
    let mut network = VirtualNetwork::<(u8, u32)>::new(config.network)
        .map_err(MixerControllerScenarioError::Profile)?;
    let mut clock = VirtualClock::new();
    let mut jitters: [JitterBuffer<JITTER_BUFFER_CAPACITY, 8>; STREAMS] =
        std::array::from_fn(|_| JitterBuffer::<JITTER_BUFFER_CAPACITY, 8>::with_start_sequence(0));
    let mut streams = std::array::from_fn(|_| MixerControllerStreamReport {
        playout: SimulationReport::default(),
        muted: false,
        gain_changes: 0,
        final_gain_milli: MAX_GAIN_MILLI,
        quality: MixerControllerStreamQuality::default(),
    });
    let mut next_send_sequence = [0_u32; STREAMS];
    let mut next_send_ms = [0_u64; STREAMS];
    let mut next_playout_ms = [config.target_buffer_ms; STREAMS];
    let mut playout_decisions = [0_u32; STREAMS];

    let mut mixer = CommandDrivenMixer::<SLOTS, PCM_SAMPLES_PER_BLOCK>::new()
        .map_err(|_| MixerControllerScenarioError::InternalCapacityInvariant)?;
    let mut controller = MixerMembershipController::<SLOTS, PCM_SAMPLES_PER_BLOCK>::new(
        mixer.membership(),
        JITTER_BUFFER_CAPACITY,
        format,
    )
    .map_err(MixerControllerScenarioError::Admission)?;
    let mut producers: [Option<RealtimePcmProducer<PCM_SAMPLES_PER_BLOCK>>; STREAMS] =
        std::array::from_fn(|_| None);
    let mut slot_counters: [Option<Arc<StreamCounters>>; STREAMS] = std::array::from_fn(|_| None);
    let mut published_inserts = 0_u32;
    let mut published_removes = 0_u32;
    let gain_handles: [GainControlHandle; STREAMS] =
        std::array::from_fn(|_| GainControlHandle::new());
    let mut scripts = config.gain_scripts;
    let mut stream_muted = [false; STREAMS];
    let mut stream_gain_milli = [MAX_GAIN_MILLI; STREAMS];
    let mut gain_changes = [0_u32; STREAMS];
    // A gain or mute event or a fresh admission restarts that slot's bounded
    // ramp, which advances only as the slot renders queued content; under
    // starvation the ramp can span many ticks. `post_event_rendered_blocks`
    // counts rendered blocks since each stream's last event or admission, and
    // the all-muted bus is asserted exactly silent only after every muted slot
    // rendered the full ramp plus one block of render lag.
    let converged_blocks = muted_slot_converged_blocks::<PCM_SAMPLES_PER_BLOCK>();
    let mut post_event_rendered_blocks = [0_u32; STREAMS];
    let mut all_muted_silent_ticks = 0_u32;

    // Admit one stream per source before the first render; the controller and
    // the callback agree that stream `i` occupies slot `i` by the shared
    // lowest-free-slot FIFO policy.
    for stream in 0..STREAMS {
        let counters = Arc::new(StreamCounters::default());
        let admitted = controller
            .admit_stream(
                format,
                gain_handles[stream].clone(),
                Some(Arc::clone(&counters)),
            )
            .map_err(MixerControllerScenarioError::Admission)?;
        producers[stream] = Some(admitted.producer);
        slot_counters[stream] = Some(counters);
        published_inserts += 1;
    }

    // Deterministic mid-session churn: remove stream 0's slot through the
    // controller at the middle playout tick and re-admit it a short time
    // later, so the projection must round-trip through the bounded queue.
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
    let mut projection_matched = true;
    while clock.now_ms() <= end_ms {
        let now_ms = clock.now_ms();
        for stream in 0..STREAMS {
            if next_send_sequence[stream] < config.frames && now_ms == next_send_ms[stream] {
                let tag = u8::try_from(stream)
                    .map_err(|_| MixerControllerScenarioError::InternalCapacityInvariant)?;
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
                .map_err(|_| MixerControllerScenarioError::InternalCapacityInvariant)?;
            match jitters[stream].insert(encoded) {
                InsertOutcome::Inserted => {}
                InsertOutcome::Reordered => streams[stream].playout.reordered_arrivals += 1,
                InsertOutcome::Duplicate => streams[stream].playout.duplicate_frames += 1,
                InsertOutcome::Late => streams[stream].playout.late_frames += 1,
                InsertOutcome::Resynchronized { .. } => {
                    streams[stream].playout.resynchronizations += 1;
                }
                InsertOutcome::SequenceExhausted | InsertOutcome::Disabled => {
                    return Err(MixerControllerScenarioError::InternalCapacityInvariant);
                }
            }
        }

        if now_ms == churn_at_ms {
            controller
                .remove_stream(0)
                .map_err(MixerControllerScenarioError::Admission)?;
            published_removes += 1;
            producers[0] = None;
        }
        if now_ms == reinsert_at_ms {
            let counters = Arc::new(StreamCounters::default());
            let admitted = controller
                .admit_stream(format, gain_handles[0].clone(), Some(Arc::clone(&counters)))
                .map_err(MixerControllerScenarioError::Admission)?;
            producers[0] = Some(admitted.producer);
            slot_counters[0] = Some(counters);
            // The fresh slot restarts its gain ramp from unity, so the
            // all-muted convergence counter must restart as well.
            post_event_rendered_blocks[0] = 0;
            published_inserts += 1;
        }

        for stream in 0..STREAMS {
            if playout_decisions[stream] < config.frames && now_ms == next_playout_ms[stream] {
                if apply_gain_events(
                    &mut scripts[stream],
                    &gain_handles[stream],
                    &mut stream_muted[stream],
                    &mut stream_gain_milli[stream],
                    &mut gain_changes[stream],
                    playout_decisions[stream],
                ) {
                    post_event_rendered_blocks[stream] = 0;
                }
                match jitters[stream].take_next() {
                    PlayoutOutcome::Frame(_) => {
                        streams[stream].playout.playout_frames += 1;
                        if let Some(producer) = producers[stream].as_mut() {
                            let _ = producer.push(&finite_block::<PCM_SAMPLES_PER_BLOCK>(stream));
                        }
                    }
                    PlayoutOutcome::Missing { .. } => streams[stream].playout.plc_frames += 1,
                    PlayoutOutcome::NotReady => {
                        return Err(MixerControllerScenarioError::InvalidTargetBuffer);
                    }
                }
                playout_decisions[stream] += 1;
                next_playout_ms[stream] =
                    next_playout_ms[stream].saturating_add(config.frame_duration_ms);
            }
        }

        let mut queued_before = [0_u32; STREAMS];
        for stream in 0..STREAMS {
            queued_before[stream] = u32::try_from(
                producers[stream]
                    .as_ref()
                    .map_or(0, RealtimePcmProducer::len),
            )
            .unwrap_or(u32::MAX);
        }
        let mut output = [0.0f32; PCM_SAMPLES_PER_BLOCK];
        mixer.render(&mut output);
        renders += 1;
        if !output.iter().all(|sample| sample.is_finite()) {
            output_finite = false;
        }
        for stream in 0..STREAMS {
            if queued_before[stream] > 0 {
                post_event_rendered_blocks[stream] += 1;
            }
        }
        if stream_muted.iter().all(|muted| *muted)
            && post_event_rendered_blocks
                .iter()
                .all(|rendered| *rendered >= converged_blocks)
        {
            if output.iter().all(|sample| *sample == 0.0) {
                all_muted_silent_ticks += 1;
            } else {
                return Err(MixerControllerScenarioError::MutedBusNotSilent);
            }
        }
        // The render applied every pending command, so the controller
        // projection must now match the mixer's actual slot table exactly.
        if controller.active() != mixer.active()
            || controller.pending_commands() != mixer.pending_commands()
            || !(0..SLOTS).all(|slot| controller.is_active(slot) == mixer.is_active(slot))
        {
            projection_matched = false;
        }
        clock
            .advance_by(1)
            .map_err(MixerControllerScenarioError::Clock)?;
    }

    let mut slot_underflows = [0_u64; SLOTS];
    let mut slot_refill_limit_hits = [0_u64; SLOTS];
    let mut slot_discontinuities = [0_u64; SLOTS];
    for stream in 0..STREAMS {
        streams[stream].muted = stream_muted[stream];
        streams[stream].gain_changes = gain_changes[stream];
        streams[stream].final_gain_milli = stream_gain_milli[stream];
        if let Some(counters) = slot_counters[stream].as_ref() {
            let snapshot = counters.snapshot();
            slot_underflows[stream] = snapshot.pcm_underflows;
            slot_refill_limit_hits[stream] = snapshot.pcm_refill_limit_hits;
            slot_discontinuities[stream] = snapshot.pcm_discontinuities;
        }
        streams[stream].quality = MixerControllerStreamQuality::from_counters(
            config.frames,
            renders,
            &streams[stream].playout,
            slot_underflows[stream],
            slot_discontinuities[stream],
        );
    }

    Ok(MixerControllerSimulationReport {
        network: network.stats(),
        streams,
        mixer: MixerControllerMixerReport {
            slots: SLOTS,
            active_streams: mixer.active(),
            projected_streams: controller.active(),
            published_inserts,
            published_removes,
            renders,
            projection_matched,
            slot_underflows,
            slot_refill_limit_hits,
            slot_discontinuities,
            output_finite,
            all_muted_silent_ticks,
        },
    })
}

/// PCM blocks a muted slot must render after its last gain or mute event or
/// admission before every later rendered block is exactly zero.
///
/// The per-slot ramp spans [`MIXER_GAIN_RAMP_SAMPLES`] applied samples and
/// advances only when the slot renders queued content, so under starvation it
/// can span many ticks. The scenario asserts silence only after every muted
/// slot rendered the full ramp plus one block of render lag, so a starved
/// slot that resumes its ramp cannot be mistaken for a broken bus.
fn muted_slot_converged_blocks<const PCM_SAMPLES_PER_BLOCK: usize>() -> u32 {
    let ramp_samples = u32::from(MIXER_GAIN_RAMP_SAMPLES);
    let block_samples = u32::try_from(PCM_SAMPLES_PER_BLOCK).unwrap_or(u32::MAX);
    ramp_samples.div_ceil(block_samples).saturating_add(1)
}

/// Applies any gain or mute event scheduled at the current consumed frame.
///
/// Returns whether an event applied; a changed target restarts that slot's
/// bounded ramp, so the caller resets the rendered-block convergence counter.
#[allow(clippy::too_many_arguments)]
fn apply_gain_events(
    script: &mut MixerControllerGainScript,
    handle: &GainControlHandle,
    muted: &mut bool,
    gain_milli: &mut u16,
    changes: &mut u32,
    consumed_frames: u32,
) -> bool {
    let mut applied = false;
    if script.mute_at_frame == Some(consumed_frames) {
        handle.set_muted(true);
        *muted = true;
        *changes += 1;
        script.mute_at_frame = None;
        applied = true;
    }
    if script.unmute_at_frame == Some(consumed_frames) {
        handle.set_muted(false);
        *muted = false;
        *changes += 1;
        script.unmute_at_frame = None;
        applied = true;
    }
    if script.gain_at_frame == Some(consumed_frames) {
        let gain = f32::from(script.gain_milli) / f32::from(MAX_GAIN_MILLI);
        handle
            .set_gain(gain)
            .expect("validated gain milli stays within the linear bound");
        *gain_milli = script.gain_milli;
        *changes += 1;
        script.gain_at_frame = None;
        applied = true;
    }
    applied
}

fn validate<const STREAMS: usize, const SLOTS: usize>(
    config: MixerControllerScenarioConfig<STREAMS>,
) -> Result<OpusFrameConfig, MixerControllerScenarioError> {
    if !(MIN_MULTI_STREAM_STREAMS..=MAX_MULTI_STREAM_STREAMS).contains(&STREAMS) {
        return Err(MixerControllerScenarioError::InvalidStreamCount);
    }
    if SLOTS == 0 || SLOTS < STREAMS {
        return Err(MixerControllerScenarioError::InvalidSlotCount);
    }
    if config.frames == 0 || config.frames > MAX_MIXER_CONTROLLER_FRAMES {
        return Err(MixerControllerScenarioError::InvalidFrameCount);
    }
    let frame_duration_ms = u16::try_from(config.frame_duration_ms)
        .map_err(|_| MixerControllerScenarioError::InvalidFrameDuration)?;
    let frame = OpusFrameConfig::new(OPUS_SAMPLE_RATE_HZ, OPUS_CHANNELS, frame_duration_ms)
        .map_err(|_| MixerControllerScenarioError::InvalidFrameDuration)?;
    if config.target_buffer_ms > 1_000 {
        return Err(MixerControllerScenarioError::InvalidTargetBuffer);
    }
    for script in &config.gain_scripts {
        for frame in [
            script.mute_at_frame,
            script.unmute_at_frame,
            script.gain_at_frame,
        ]
        .into_iter()
        .flatten()
        {
            if frame >= config.frames {
                return Err(MixerControllerScenarioError::InvalidGainEvent);
            }
        }
        if script.gain_milli > MAX_GAIN_MILLI {
            return Err(MixerControllerScenarioError::InvalidGainMilli);
        }
    }
    if config.media_datagram_bytes == 0
        || usize::from(config.media_datagram_bytes) > MAX_MEDIA_DATAGRAM_LEN
    {
        return Err(MixerControllerScenarioError::InvalidMediaDatagramBytes);
    }
    config
        .network
        .validate()
        .map_err(MixerControllerScenarioError::Profile)?;
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use sonimesh_audio::{GainControlError, RealtimeQueueConfigError};

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
            simulate_mixer_controller_membership::<0, SLOTS, PCM_BLOCK>(
                MixerControllerScenarioConfig::<0>::default()
            ),
            Err(MixerControllerScenarioError::InvalidStreamCount)
        );
        assert_eq!(
            simulate_mixer_controller_membership::<1, SLOTS, PCM_BLOCK>(
                MixerControllerScenarioConfig::<1>::default()
            ),
            Err(MixerControllerScenarioError::InvalidStreamCount)
        );
        assert_eq!(
            simulate_mixer_controller_membership::<5, SLOTS, PCM_BLOCK>(
                MixerControllerScenarioConfig::<5>::default()
            ),
            Err(MixerControllerScenarioError::InvalidStreamCount)
        );
        assert_eq!(
            simulate_mixer_controller_membership::<2, 1, PCM_BLOCK>(
                MixerControllerScenarioConfig::<2>::default()
            ),
            Err(MixerControllerScenarioError::InvalidSlotCount)
        );
        assert_eq!(
            simulate_mixer_controller_membership::<2, 0, PCM_BLOCK>(
                MixerControllerScenarioConfig::<2>::default()
            ),
            Err(MixerControllerScenarioError::InvalidSlotCount)
        );
    }

    #[test]
    fn clean_shared_link_projection_matches_the_mixer_after_every_render() {
        let config = MixerControllerScenarioConfig {
            frames: 7,
            frame_duration_ms: 10,
            target_buffer_ms: 10,
            network: clean_profile(),
            ..MixerControllerScenarioConfig::default()
        };

        let first = simulate_mixer_controller_membership::<2, SLOTS, PCM_BLOCK>(config).unwrap();
        let second = simulate_mixer_controller_membership::<2, SLOTS, PCM_BLOCK>(config).unwrap();

        assert_eq!(first, second, "the simulation must be deterministic");
        for stream in &first.streams {
            assert_eq!(stream.playout.playout_frames, config.frames);
            assert_eq!(stream.playout.plc_frames, 0);
            assert!(stream.quality.within_bounds());
            assert_eq!(stream.quality.plc_ratio_milli, 0);
        }
        assert_eq!(first.mixer.active_streams, 2);
        assert_eq!(first.mixer.projected_streams, 2);
        assert_eq!(first.mixer.slots, SLOTS);
        assert_eq!(
            first.mixer.published_inserts, 3,
            "two starts plus the re-admit"
        );
        assert_eq!(first.mixer.published_removes, 1);
        assert!(first.mixer.projection_matched);
        assert!(first.mixer.output_finite);
    }

    #[test]
    fn shared_link_loss_keeps_the_projection_consistent_and_isolates_slots() {
        let mut network = NetworkProfile::stable(7);
        network.burst_interval_packets = 20;
        network.burst_length_packets = 3;
        let config = MixerControllerScenarioConfig {
            frames: 100,
            network,
            ..MixerControllerScenarioConfig::default()
        };

        let report = simulate_mixer_controller_membership::<2, SLOTS, PCM_BLOCK>(config).unwrap();

        for stream in &report.streams {
            assert_eq!(
                stream.playout.playout_frames + stream.playout.plc_frames,
                config.frames
            );
            assert!(stream.quality.within_bounds());
        }
        let total_plc: u32 = report
            .streams
            .iter()
            .map(|stream| stream.playout.plc_frames)
            .sum();
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
        assert!(report.mixer.projection_matched);
        assert!(report.mixer.output_finite);
        assert_eq!(report.mixer.active_streams, 2);
        assert_eq!(report.mixer.projected_streams, 2);
    }

    #[test]
    fn membership_churn_re_admits_stream_zero_through_the_controller() {
        let config = MixerControllerScenarioConfig {
            frames: 40,
            target_buffer_ms: 20,
            network: clean_profile(),
            ..MixerControllerScenarioConfig::default()
        };

        let report = simulate_mixer_controller_membership::<2, SLOTS, PCM_BLOCK>(config).unwrap();

        assert_eq!(report.mixer.published_inserts, 3);
        assert_eq!(report.mixer.published_removes, 1);
        assert_eq!(report.mixer.active_streams, 2);
        assert_eq!(report.mixer.projected_streams, 2);
        assert!(
            report.mixer.slot_underflows[0] > 0,
            "the re-admitted slot starts empty and underflows until its first playout"
        );
        assert!(report.mixer.projection_matched);
        assert!(report.mixer.output_finite);
    }

    #[test]
    fn rejects_invalid_scenario_configuration() {
        assert_eq!(
            simulate_mixer_controller_membership::<2, SLOTS, PCM_BLOCK>(
                MixerControllerScenarioConfig {
                    frames: 0,
                    ..MixerControllerScenarioConfig::default()
                }
            ),
            Err(MixerControllerScenarioError::InvalidFrameCount)
        );
        assert_eq!(
            simulate_mixer_controller_membership::<2, SLOTS, PCM_BLOCK>(
                MixerControllerScenarioConfig {
                    frame_duration_ms: 15,
                    ..MixerControllerScenarioConfig::default()
                }
            ),
            Err(MixerControllerScenarioError::InvalidFrameDuration)
        );
        assert_eq!(
            simulate_mixer_controller_membership::<2, SLOTS, PCM_BLOCK>(
                MixerControllerScenarioConfig {
                    target_buffer_ms: 1_001,
                    ..MixerControllerScenarioConfig::default()
                }
            ),
            Err(MixerControllerScenarioError::InvalidTargetBuffer)
        );
        assert_eq!(
            simulate_mixer_controller_membership::<2, SLOTS, PCM_BLOCK>(
                MixerControllerScenarioConfig {
                    media_datagram_bytes: 0,
                    ..MixerControllerScenarioConfig::default()
                }
            ),
            Err(MixerControllerScenarioError::InvalidMediaDatagramBytes)
        );
        let mut network = NetworkProfile::stable(3);
        network.bandwidth_bytes_per_second = MAX_BANDWIDTH_BYTES_PER_SECOND + 1;
        assert_eq!(
            simulate_mixer_controller_membership::<2, SLOTS, PCM_BLOCK>(
                MixerControllerScenarioConfig {
                    network,
                    ..MixerControllerScenarioConfig::default()
                }
            ),
            Err(MixerControllerScenarioError::Profile(
                ProfileError::BandwidthTooLarge
            ))
        );
        assert_eq!(
            simulate_mixer_controller_membership::<2, SLOTS, PCM_BLOCK>(
                MixerControllerScenarioConfig {
                    gain_scripts: [MixerControllerGainScript {
                        mute_at_frame: Some(500),
                        ..MixerControllerGainScript::default()
                    }; 2],
                    ..MixerControllerScenarioConfig::default()
                }
            ),
            Err(MixerControllerScenarioError::InvalidGainEvent)
        );
        assert_eq!(
            simulate_mixer_controller_membership::<2, SLOTS, PCM_BLOCK>(
                MixerControllerScenarioConfig {
                    gain_scripts: [MixerControllerGainScript {
                        gain_at_frame: Some(5),
                        gain_milli: 1_001,
                        ..MixerControllerGainScript::default()
                    }; 2],
                    ..MixerControllerScenarioConfig::default()
                }
            ),
            Err(MixerControllerScenarioError::InvalidGainMilli)
        );
    }

    #[test]
    fn per_source_mute_makes_the_bus_exactly_silent() {
        let config = MixerControllerScenarioConfig {
            frames: 40,
            target_buffer_ms: 20,
            network: clean_profile(),
            // Mute every stream after the churn re-admission, then unmute
            // stream 0, so the all-muted bus must go exactly silent and
            // per-source control must not stall the other slot.
            gain_scripts: [
                MixerControllerGainScript {
                    mute_at_frame: Some(30),
                    unmute_at_frame: Some(35),
                    ..MixerControllerGainScript::default()
                },
                MixerControllerGainScript {
                    mute_at_frame: Some(30),
                    ..MixerControllerGainScript::default()
                },
            ],
            ..MixerControllerScenarioConfig::default()
        };

        let report = simulate_mixer_controller_membership::<2, SLOTS, PCM_BLOCK>(config).unwrap();

        assert!(!report.streams[0].muted);
        assert!(report.streams[1].muted);
        assert!(
            report.mixer.all_muted_silent_ticks > 0,
            "the all-muted bus must render exactly silent ticks"
        );
        assert_eq!(report.streams[0].gain_changes, 2);
        assert_eq!(report.streams[1].gain_changes, 1);
        assert!(report.mixer.projection_matched);
        assert!(report.mixer.output_finite);
        assert_eq!(report.mixer.active_streams, 2);
        assert_eq!(report.mixer.projected_streams, 2);
    }

    #[test]
    fn per_source_gain_events_publish_without_stalling_membership() {
        let config = MixerControllerScenarioConfig {
            frames: 40,
            target_buffer_ms: 20,
            network: clean_profile(),
            gain_scripts: [
                MixerControllerGainScript {
                    gain_at_frame: Some(25),
                    gain_milli: 250,
                    ..MixerControllerGainScript::default()
                },
                MixerControllerGainScript::default(),
            ],
            ..MixerControllerScenarioConfig::default()
        };

        let report = simulate_mixer_controller_membership::<2, SLOTS, PCM_BLOCK>(config).unwrap();

        assert_eq!(report.streams[0].gain_changes, 1);
        assert_eq!(report.streams[0].final_gain_milli, 250);
        assert_eq!(report.streams[1].gain_changes, 0);
        assert_eq!(report.streams[1].final_gain_milli, 1_000);
        assert!(report.mixer.projection_matched);
        assert!(report.mixer.output_finite);
        assert_eq!(report.mixer.published_inserts, 3);
        assert_eq!(report.mixer.published_removes, 1);
    }

    #[test]
    fn derived_quality_ratios_stay_bounded_and_reflect_impairment() {
        let mut network = NetworkProfile::stable(7);
        network.burst_interval_packets = 20;
        network.burst_length_packets = 3;
        let config = MixerControllerScenarioConfig {
            frames: 100,
            network,
            ..MixerControllerScenarioConfig::default()
        };

        let report = simulate_mixer_controller_membership::<2, SLOTS, PCM_BLOCK>(config).unwrap();

        for stream in &report.streams {
            assert!(stream.quality.within_bounds());
        }
        let total_plc: u32 = report
            .streams
            .iter()
            .map(|stream| stream.playout.plc_frames)
            .sum();
        assert!(
            total_plc > 0,
            "shared-link loss must reach the jitter buffers"
        );
        for stream in &report.streams {
            assert_eq!(
                stream.quality.plc_ratio_milli,
                ratio_milli(
                    u64::from(stream.playout.plc_frames),
                    u64::from(config.frames)
                ),
                "the PLC ratio must be re-derived exactly from the raw counters"
            );
        }
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
                .streams
                .iter()
                .any(|stream| stream.quality.underflow_ratio_milli > 0),
            "the underflow ratio must reflect the recorded PCM underflows"
        );
    }

    #[test]
    fn preserved_error_variants_expose_their_causes() {
        let profile = MixerControllerScenarioError::Profile(ProfileError::DelayTooLarge);
        assert!(
            Error::source(&profile)
                .and_then(|source| source.downcast_ref::<ProfileError>())
                .is_some()
        );

        let clock = MixerControllerScenarioError::Clock(ClockError::Overflow);
        assert!(
            Error::source(&clock)
                .and_then(|source| source.downcast_ref::<ClockError>())
                .is_some()
        );

        let gain = MixerControllerScenarioError::GainControl(GainControlError::InvalidGain);
        assert!(
            Error::source(&gain)
                .and_then(|source| source.downcast_ref::<GainControlError>())
                .is_some(),
            "the gain error must expose its cause"
        );

        let admission = MixerControllerScenarioError::Admission(MixerAdmissionError::Queue(
            RealtimeQueueConfigError::ZeroCapacity,
        ));
        let admission_source = Error::source(&admission)
            .and_then(|source| source.downcast_ref::<MixerAdmissionError>())
            .expect("scenario error must expose the admission error");
        assert!(
            Error::source(admission_source)
                .and_then(|source| source.downcast_ref::<RealtimeQueueConfigError>())
                .is_some(),
            "the admission error must expose its queue cause"
        );

        assert!(Error::source(&MixerControllerScenarioError::InvalidStreamCount).is_none());
        assert!(Error::source(&MixerControllerScenarioError::InternalCapacityInvariant).is_none());
    }
}
