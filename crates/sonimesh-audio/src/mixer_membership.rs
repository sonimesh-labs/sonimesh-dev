//! Lock-free membership boundary between the control domain and the realtime
//! fixed-slot mixer.
//!
//! ADR 0015 requires dynamic stream membership without allocating or taking a
//! long-lived lock inside the output callback. This module splits ownership:
//! the callback owns the [`FixedSlotMixer`] and applies membership changes; the
//! control domain publishes bounded commands through a
//! [`MixerMembershipHandle`]; and a bounded lock-free queue carries the
//! commands across the boundary. The callback drains the queue at the start of
//! [`CommandDrivenMixer::render`], so a published change takes effect on the
//! next output callback with bounded latency and bounded callback work.

use std::fmt;
use std::sync::Arc;

use crossbeam_queue::ArrayQueue;
use sonimesh_diagnostics::StreamCounters;

use crate::gain_control::GainControlHandle;
use crate::realtime_queue::RealtimePcmConsumer;
use crate::{FixedSlotMixer, MixerConfigError};

/// Hard upper bound for pending membership commands shared with the callback.
///
/// The bound keeps worst-case callback drain work constant: at most this many
/// commands are popped per render. The control domain fails closed when the
/// queue is full instead of growing it.
pub const MAX_MIXER_COMMAND_QUEUE: usize = 16;

/// Failure while publishing a mixer membership change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MixerMembershipError {
    /// The bounded command queue is full; retry after the next output callback.
    CommandQueueFull,
    /// The slot index is outside the fixed slot table.
    SlotOutOfRange,
}

impl fmt::Display for MixerMembershipError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CommandQueueFull => formatter.write_str("mixer membership command queue is full"),
            Self::SlotOutOfRange => formatter.write_str("mixer slot index is out of range"),
        }
    }
}

impl std::error::Error for MixerMembershipError {}

/// One bounded membership change crossing the realtime boundary.
enum MixerCommand<const SAMPLES_PER_BLOCK: usize> {
    /// Activate one stream at the lowest free slot.
    Insert {
        consumer: RealtimePcmConsumer<SAMPLES_PER_BLOCK>,
        gain: GainControlHandle,
        counters: Option<Arc<StreamCounters>>,
    },
    /// Deactivate one slot and drop its consumer.
    Remove { slot: usize },
}

/// Control-domain endpoint that publishes bounded mixer membership changes.
///
/// The handle is cheap to clone and shares a bounded lock-free command queue
/// with the callback-owned [`CommandDrivenMixer`]. Publishing never allocates
/// and never takes a lock; it fails closed when the queue is full.
#[derive(Clone)]
pub struct MixerMembershipHandle<const SAMPLES_PER_BLOCK: usize> {
    commands: Arc<ArrayQueue<MixerCommand<SAMPLES_PER_BLOCK>>>,
    slots: usize,
}

impl<const SAMPLES_PER_BLOCK: usize> MixerMembershipHandle<SAMPLES_PER_BLOCK> {
    /// Publishes one stream activation at the lowest free slot.
    ///
    /// The callback applies the command on its next render. `counters` is the
    /// optional per-slot metrics endpoint recorded by the callback. On failure
    /// the consumer is dropped, so a retry needs a fresh queue endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`MixerMembershipError::CommandQueueFull`] when the bounded
    /// queue is full; the mixer is unchanged until a later retry.
    pub fn insert(
        &self,
        consumer: RealtimePcmConsumer<SAMPLES_PER_BLOCK>,
        gain: GainControlHandle,
        counters: Option<Arc<StreamCounters>>,
    ) -> Result<(), MixerMembershipError> {
        self.commands
            .push(MixerCommand::Insert {
                consumer,
                gain,
                counters,
            })
            .map_err(|_| MixerMembershipError::CommandQueueFull)
    }

    /// Number of membership commands waiting to be applied by the callback.
    ///
    /// The control domain can poll this to learn whether the callback has
    /// caught up with its published membership changes: zero means every
    /// published command was applied by the most recent render.
    #[must_use]
    pub fn pending_commands(&self) -> usize {
        self.commands.len()
    }
    /// Publishes one stream deactivation for `slot`.
    ///
    /// The callback applies the command on its next render and drops the
    /// slot's consumer; the control domain drops its matching producer. A
    /// stale remove for an already-inactive slot is tolerated by the callback.
    ///
    /// # Errors
    ///
    /// Returns [`MixerMembershipError::SlotOutOfRange`] when `slot` is outside
    /// the fixed table, or [`MixerMembershipError::CommandQueueFull`] when the
    /// bounded queue is full.
    pub fn remove(&self, slot: usize) -> Result<(), MixerMembershipError> {
        if slot >= self.slots {
            return Err(MixerMembershipError::SlotOutOfRange);
        }
        self.commands
            .push(MixerCommand::Remove { slot })
            .map_err(|_| MixerMembershipError::CommandQueueFull)
    }
}

/// Callback-owned fixed-slot mixer with a lock-free membership boundary.
///
/// The callback drains pending membership commands at the start of every
/// [`Self::render`] and applies them to its [`FixedSlotMixer`] without
/// allocating or locking. Membership state therefore advances at callback
/// granularity: a published change is visible from the next render.
pub struct CommandDrivenMixer<const SLOTS: usize, const SAMPLES_PER_BLOCK: usize> {
    mixer: FixedSlotMixer<SLOTS, SAMPLES_PER_BLOCK>,
    commands: Arc<ArrayQueue<MixerCommand<SAMPLES_PER_BLOCK>>>,
}

impl<const SLOTS: usize, const SAMPLES_PER_BLOCK: usize>
    CommandDrivenMixer<SLOTS, SAMPLES_PER_BLOCK>
{
    /// Creates an empty callback mixer with a bounded command queue.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when `SLOTS` or `SAMPLES_PER_BLOCK` is
    /// zero.
    pub fn new() -> Result<Self, MixerConfigError> {
        let mixer = FixedSlotMixer::<SLOTS, SAMPLES_PER_BLOCK>::new()?;
        let commands = Arc::new(ArrayQueue::new(MAX_MIXER_COMMAND_QUEUE));
        Ok(Self { mixer, commands })
    }

    /// Returns a control-domain handle sharing this mixer's command queue.
    #[must_use]
    pub fn membership(&self) -> MixerMembershipHandle<SAMPLES_PER_BLOCK> {
        MixerMembershipHandle {
            commands: Arc::clone(&self.commands),
            slots: SLOTS,
        }
    }

    /// Renders one mixed bus block, first applying pending membership changes.
    ///
    /// This is the only realtime entry point. Draining is bounded by
    /// [`MAX_MIXER_COMMAND_QUEUE`] lock-free pops, then the fixed-slot mixer
    /// renders with its documented per-callback bounds. A command that cannot
    /// be applied (for example an insert when every slot is occupied) is
    /// dropped so one stream's failure never blocks or resets other streams.
    pub fn render(&mut self, output: &mut [f32]) {
        self.drain_pending();
        self.mixer.render(output);
    }

    /// Applies every pending membership command without rendering.
    ///
    /// The realtime path calls this from [`Self::render`]; it is public so
    /// control-domain tests and diagnostics can advance membership explicitly.
    pub fn drain_pending(&mut self) {
        while let Some(command) = self.commands.pop() {
            match command {
                MixerCommand::Insert {
                    consumer,
                    gain,
                    counters,
                } => {
                    let _ = self.mixer.insert(consumer, gain, counters);
                }
                MixerCommand::Remove { slot } => {
                    let _ = self.mixer.remove(slot);
                }
            }
        }
    }

    /// Number of membership commands waiting to be applied.
    #[must_use]
    pub fn pending_commands(&self) -> usize {
        self.commands.len()
    }

    /// Total number of slots in the fixed table.
    #[must_use]
    pub const fn slots(&self) -> usize {
        SLOTS
    }

    /// Number of currently active slots.
    #[must_use]
    pub const fn active(&self) -> usize {
        self.mixer.active()
    }

    /// Returns whether the given slot is currently active.
    #[must_use]
    pub fn is_active(&self, slot: usize) -> bool {
        self.mixer.is_active(slot)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{CommandDrivenMixer, MAX_MIXER_COMMAND_QUEUE, MixerMembershipError};
    use crate::gain_control::GainControlHandle;
    use crate::realtime_queue::{RealtimePcmProducer, RealtimePcmPushOutcome, realtime_pcm_queue};

    const SLOTS: usize = 4;
    const SAMPLES_PER_BLOCK: usize = 8;

    fn push_constant(
        producer: &mut RealtimePcmProducer<SAMPLES_PER_BLOCK>,
        value: f32,
        blocks: usize,
    ) {
        for _ in 0..blocks {
            assert_eq!(
                producer.push(&[value; SAMPLES_PER_BLOCK]),
                RealtimePcmPushOutcome::Inserted
            );
        }
    }

    #[test]
    fn insert_command_activates_a_slot_on_the_next_render() {
        let mut mixer: CommandDrivenMixer<SLOTS, SAMPLES_PER_BLOCK> =
            CommandDrivenMixer::new().unwrap();
        let membership = mixer.membership();
        let (mut producer, consumer) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(4).unwrap();
        push_constant(&mut producer, 1.0, 2);

        membership
            .insert(consumer, GainControlHandle::new(), None)
            .unwrap();
        assert_eq!(mixer.active(), 0, "command is not applied before a render");

        let mut output = [0.0; 16];
        mixer.render(&mut output);
        assert_eq!(mixer.active(), 1);
        assert!(mixer.is_active(0));
        assert!(output.iter().all(|sample| (*sample - 1.0).abs() < 1e-6));
        assert_eq!(mixer.pending_commands(), 0);
    }

    #[test]
    fn commands_drain_in_fifo_order_within_one_render() {
        let mut mixer: CommandDrivenMixer<SLOTS, SAMPLES_PER_BLOCK> =
            CommandDrivenMixer::new().unwrap();
        let membership = mixer.membership();
        let (mut producer_a, consumer_a) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(4).unwrap();
        let (mut producer_b, consumer_b) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(4).unwrap();
        push_constant(&mut producer_a, 1.0, 1);
        push_constant(&mut producer_b, 2.0, 1);

        membership
            .insert(consumer_a, GainControlHandle::new(), None)
            .unwrap();
        membership
            .insert(consumer_b, GainControlHandle::new(), None)
            .unwrap();
        membership.remove(0).unwrap();

        let mut output = [0.0; SAMPLES_PER_BLOCK];
        mixer.render(&mut output);
        assert_eq!(mixer.active(), 1);
        assert!(!mixer.is_active(0));
        assert!(mixer.is_active(1));
        // Slot a was inserted then removed before rendering, so only slot b's
        // stream survives the FIFO drain.
        assert!(output.iter().all(|sample| (*sample - 2.0).abs() < 1e-6));
    }

    #[test]
    fn remove_command_deactivates_a_slot_on_the_next_render() {
        let mut mixer: CommandDrivenMixer<SLOTS, SAMPLES_PER_BLOCK> =
            CommandDrivenMixer::new().unwrap();
        let membership = mixer.membership();
        for _ in 0..2 {
            let (_, consumer) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(2).unwrap();
            membership
                .insert(consumer, GainControlHandle::new(), None)
                .unwrap();
        }
        mixer.render(&mut [0.0; SAMPLES_PER_BLOCK]);
        assert_eq!(mixer.active(), 2);

        membership.remove(0).unwrap();
        mixer.render(&mut [0.0; SAMPLES_PER_BLOCK]);
        assert_eq!(mixer.active(), 1);
        assert!(!mixer.is_active(0));
        assert!(mixer.is_active(1));
    }

    #[test]
    fn command_queue_full_fails_closed() {
        let mut mixer: CommandDrivenMixer<SLOTS, SAMPLES_PER_BLOCK> =
            CommandDrivenMixer::new().unwrap();
        let membership = mixer.membership();
        for _ in 0..MAX_MIXER_COMMAND_QUEUE {
            let (_, consumer) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(2).unwrap();
            membership
                .insert(consumer, GainControlHandle::new(), None)
                .unwrap();
        }
        let (_, consumer) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(2).unwrap();
        assert_eq!(
            membership.insert(consumer, GainControlHandle::new(), None),
            Err(MixerMembershipError::CommandQueueFull)
        );

        // The queued commands still drain and apply up to the slot bound.
        mixer.render(&mut [0.0; SAMPLES_PER_BLOCK]);
        assert_eq!(mixer.active(), SLOTS);
        assert_eq!(mixer.pending_commands(), 0);
    }

    #[test]
    fn stale_remove_is_tolerated_and_out_of_range_fails_fast() {
        let mut mixer: CommandDrivenMixer<SLOTS, SAMPLES_PER_BLOCK> =
            CommandDrivenMixer::new().unwrap();
        let membership = mixer.membership();

        membership.remove(0).unwrap();
        mixer.render(&mut [0.0; SAMPLES_PER_BLOCK]);
        assert_eq!(mixer.active(), 0);

        assert_eq!(
            membership.remove(SLOTS),
            Err(MixerMembershipError::SlotOutOfRange)
        );
    }

    #[test]
    fn pending_commands_are_applied_only_once() {
        let mut mixer: CommandDrivenMixer<SLOTS, SAMPLES_PER_BLOCK> =
            CommandDrivenMixer::new().unwrap();
        let membership = mixer.membership();
        let (_, consumer) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(2).unwrap();
        membership
            .insert(consumer, GainControlHandle::new(), None)
            .unwrap();

        mixer.render(&mut [0.0; SAMPLES_PER_BLOCK]);
        assert_eq!(mixer.active(), 1);
        mixer.render(&mut [0.0; SAMPLES_PER_BLOCK]);
        assert_eq!(mixer.active(), 1);
        assert_eq!(mixer.pending_commands(), 0);
    }

    #[test]
    fn handle_reports_pending_commands_until_the_callback_applies_them() {
        let mut mixer: CommandDrivenMixer<SLOTS, SAMPLES_PER_BLOCK> =
            CommandDrivenMixer::new().unwrap();
        let membership = mixer.membership();
        let (_, consumer) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(2).unwrap();
        membership
            .insert(consumer, GainControlHandle::new(), None)
            .unwrap();
        assert_eq!(membership.pending_commands(), 1);

        mixer.render(&mut [0.0; SAMPLES_PER_BLOCK]);
        assert_eq!(membership.pending_commands(), 0);
    }

    #[test]
    fn concurrent_inserts_are_applied_across_renders() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let mut mixer: CommandDrivenMixer<SLOTS, SAMPLES_PER_BLOCK> =
            CommandDrivenMixer::new().unwrap();
        let membership = mixer.membership();
        let ready = Arc::new(AtomicBool::new(false));
        let ready_worker = Arc::clone(&ready);

        let worker = std::thread::spawn(move || {
            for _ in 0..SLOTS {
                let (_, consumer) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(2).unwrap();
                membership
                    .insert(consumer, GainControlHandle::new(), None)
                    .unwrap();
            }
            ready_worker.store(true, Ordering::Release);
        });

        // Render until the worker reports it published every command, with a
        // generous guard so a slow scheduler cannot hang the test.
        let mut renders = 0;
        while !ready.load(Ordering::Acquire) && renders < 1_000_000 {
            mixer.render(&mut [0.0; SAMPLES_PER_BLOCK]);
            renders += 1;
        }
        worker.join().unwrap();
        // One final render applies whatever the worker published last.
        mixer.render(&mut [0.0; SAMPLES_PER_BLOCK]);
        assert_eq!(
            mixer.active(),
            SLOTS,
            "commands published from another thread must be applied by renders"
        );
        assert_eq!(mixer.pending_commands(), 0);
    }
}
