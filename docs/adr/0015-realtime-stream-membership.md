# ADR 0015: Realtime stream membership boundary

- Status: Provisional pending Phase 2 mixer evidence
- Date: 2026-08-12

## Context

Phase 2 of the [roadmap](../roadmap.md) adds two to four independent sources
and a fixed-slot realtime mixer. The output callback is forbidden from
allocating, locking, awaiting, decoding, doing network I/O, writing logs,
enumerating devices, or calling UI IPC; it may only touch preallocated state and
bounded data structures. The architecture therefore requires dynamic stream
membership to use fixed slots or a preallocated snapshot/RCU boundary, so adding
a source never allocates or takes a long-lived mutex inside the callback.

Today the single-stream MVP path owns exactly one `Box<dyn RenderSource>` fed by
one bounded PCM queue ([ADR 0002](./0002-realtime-pcm-queue.md)). No membership
table, mixer, or multi-stream render path exists yet. This record fixes the
Phase 2 boundary before that implementation starts; it does not change the
single-stream MVP path.

## Decision

Phase 2 uses a fixed preallocated slot table for realtime stream membership. The
roadmap already commits to a fixed-slot mixer, and the fixed bound matches the
two-to-four source scope.

- **Slot count is a construction-time constant.** Provisionally eight slots
  (headroom above the roadmap's four-source maximum). The slot count is an
  explicit upper bound: exceeding it fails closed at admission in the control
  domain, never in the callback.
- **Every slot is preallocated.** Each slot owns one stream's realtime consumer
  endpoint and its per-stream gain/mute state. The callback iterates every slot
  in fixed order; inactive or retired slots render silence and contribute zero,
  so worst-case callback work is `slot_count` times a per-slot constant with no
  allocation and no lock.
- **Membership transitions are control-domain work.** Activating, deactivating,
  or replacing a slot is driven by the control domain, which installs or retires
  the stream's queue epoch before the callback can read the slot. The existing
  atomic epoch and sequence markers ([ADR 0002](./0002-realtime-pcm-queue.md))
  let the callback discard stale blocks without observing a partially
  constructed slot.
- **Per-slot isolation is a hard invariant.** A decoder, network, or queue
  failure in one slot must not reset or block other slots. Slot-local faults
  surface as per-stream metrics and as silence for that slot only.
- **No unsafe code is required.** Fixed slots are plain preallocated arrays;
  the callback holds no interior-mutable custom structure.

### Membership handoff boundary

The callback owns the fixed slot table; the control domain publishes bounded
membership changes through a lock-free command queue. `CommandDrivenMixer`
(sonimesh-audio) drains the queue at the start of each render — at most
`MAX_MIXER_COMMAND_QUEUE` lock-free pops — applies inserts and removes to its
slots without allocating or locking, then mixes. A published change takes
effect on the next output callback; the ADR 0002 epoch markers still retire
stale blocks before the callback reads a newly inserted consumer. The control
domain fails closed when the queue is full and must retry with a fresh
endpoint. Removing a slot returns no consumer to the control domain: the
control domain drops its producer and the callback drops the slot's consumer.

### Alternatives considered

- **Preallocated snapshot/RCU:** an atomic pointer to an immutable snapshot of
  active stream references. This also satisfies the callback constraints, but it
  needs an ownership and memory-reclamation story (reclaiming retired
  snapshots without blocking the callback) and is not required for a bounded
  two-to-four source Phase 2. It remains the documented fallback if measured
  evidence later shows per-slot iteration cost or membership churn is a problem.
- **Dynamic `Vec`/mutex membership:** rejected. Growing the membership list
  allocates and taking a long-lived lock in the callback violates the
  architecture constraint.
- **Lock-free growable membership (for example epoch-based reclamation):**
  rejected for Phase 2. It requires unsafe or an extra memory-model verification
  effort for no benefit over the fixed bound.

## Security and realtime implications

Slot identity and slot index are control-domain values; the callback treats a
slot's payload as length-bounded input only and relies on the bounded queue for
that guarantee. A retired stream must not leave stale audio: slot deactivation
retires the queue epoch before the control domain stops feeding the slot, so the
next callback read discards retired blocks (existing [ADR 0002]
(./0002-realtime-pcm-queue.md) semantics). Cross-slot interference is prevented
by construction (fixed, disjoint slots) and must be demonstrated by failure
injection tests. No key or network identity crosses the membership table.

## Validation evidence

Before Phase 2 can claim multi-source streaming:

- worst-case render loop is bounded: a test proves the callback visits exactly
  `slot_count` slots and does constant work per frame regardless of membership
  churn;
- membership transitions (activate, deactivate, replace) never allocate and
  never take a lock; a deterministic property test over random transition
  sequences;
- per-slot isolation: failing the decoder or network path for one slot leaves
  other slots rendering, with metrics that distinguish the failing slot;
- slot-capacity overflow fails closed at admission in the control domain;
- the roadmap's 24-hour multi-source soak runs on the fixed slot count;
- hardware callback timing on actual WASAPI devices is still required before any
  latency claim (unchanged from [ADR 0002](./0002-realtime-pcm-queue.md)).

These are membership and mixing-boundary tests. They do not qualify pairing,
capture behavior, or physical latency.

## Implementation status

The fixed-slot mixer primitive exists in `sonimesh-audio`
(`FixedSlotMixer<SLOTS, SAMPLES_PER_BLOCK>`): it owns a preallocated slot table
of per-stream consumer endpoints, iterates active slots in ascending fixed
order during rendering, applies a per-slot discontinuity fade, fails closed at
admission when the table is full, and returns the consumer on deactivation. Each
render call (one output callback) refills at most `MAX_PCM_QUEUE_BLOCKS` blocks
per slot and records a single refill-limit event when that shared budget is
exhausted, matching the single-stream render path. Each slot also owns its
per-stream gain and mute state: the slot holds a
`GainControlHandle` that the control domain publishes to, and the callback reads
that state without locking and ramps it through the slot's `GainSmoother`. A
slot can carry an optional per-slot `StreamCounters` endpoint that the callback
records underflow, refill-limit, and discontinuity events into without locking,
so per-stream quality metrics can distinguish the failing slot. The primitive
composes with the bounded-output limiter as the final output stage.

The membership handoff boundary also exists: `CommandDrivenMixer` owns a
fixed-slot table and shares a bounded lock-free command queue with a
`MixerMembershipHandle`. The control domain publishes insert/remove commands;
the callback drains them at the start of each render and applies them without
allocating or locking, so membership advances at callback granularity.

The app-side Phase 2 boundary is wired in `sonimesh-app`:
`MixerRenderSource` owns a `CommandDrivenMixer` plus the bounded-output limiter
as the realtime `RenderSource`, and `MixerMembershipController` is the
control-domain admission boundary. The controller publishes insert/remove
commands through a `MixerMembershipHandle`, keeps a deterministic projection of
the fixed slot table (mirroring the callback's lowest-free-slot FIFO policy),
validates each stream's format against the mixer's output format, and carries
each stream's gain/mute handle so the control domain can publish per-stream
changes across the callback boundary.

The per-stream source-to-output conversion primitive also exists in
`sonimesh-app`: `StreamFormatAdapter` drains decoded source-format blocks,
applies nominal sample-rate conversion
([ADR 0013](./0013-nominal-sample-rate-conversion.md)) followed by channel
mapping ([ADR 0012](./0012-channel-layout-mapping.md)), and pushes bounded
output-format blocks into the stream's mixer slot queue. It runs in the
decode/control domain, never inside the output callback, so the callback still
reads only pre-adapted output-format blocks. The controller wires the adapter
into admission: `MixerMembershipController::admit_adapted_stream` accepts any
valid source format, constructs the per-stream adapter before publishing the
slot command (so an invalid format never reserves a slot), and returns the
source-format producer together with the adapter for the decode/control
domain. The output-format fast path `admit_stream` remains for streams that
already produce the mixer's output format.

The desktop session wiring composes these pieces in `sonimesh-app`:
`MixerOutputController` owns the platform `AudioOutput` and creates one
`MixerRenderSource` per output lifetime, exposing a clone of the live
membership handle; `MixerStreamSession` owns that controller plus the
membership boundary and the per-stream adapter pumps, starts the output,
admits streams through `admit_adapted_stream`, installs each stream's source
producer into its caller-owned pipeline, advances the adapters after every
decode step, and after output-device recreation re-admits every persisted
stream against a fresh empty slot table so each pipeline receives a fresh
producer. Per-stream drift control (`MixerStreamDriftController`) samples
only the bounded adapter source queue, publishes bounded ppm corrections to
the stream's resampler, retires the adapter epoch on an emergency occupancy
threshold, and resets after recovery so a stale correction is not
re-applied.

The app-side composition layer also exists in `sonimesh-app`:
`ControlToMixerBridge` owns the `ControlStreamCoordinator` and the
`MixerStreamSession` together, so stream lifecycle passes through one object.
The bridge starts the platform output with the first stream, admits and
removes per-stream pipelines, commits the coordinator's tokenized
start/rotation/stop completions, and rolls back partial work when a start,
rotation, or stop fails so the coordinator and mixer membership stay in sync.
A platform fault observed by `monitor` retires the membership boundary and
moves the lifecycle to `OutputUnavailable`; `recover_output` recreates the
mixer source and returns every persisted stream identity, and `recover_stream`
re-admits each one against the fresh empty slot table. The output stops and
the lifecycle returns to `Ready` when the last stream stops. The bridge is
control-domain code and is not part of the realtime scheduling path: the
caller still advances adapter pumps and renders through the owned mixer
session. The UI/Tauri command layer is not yet wired to the bridge. The bridge
also projects a redacted low-rate snapshot for a future control-surface
adapter — lifecycle phase, output lifecycle, output-active flag, active stream
count, and pending membership commands — and implements `OutputSnapshotSource`
so existing UI projection code can read its output lifecycle. The bridge
also exposes `desktop_snapshot_at` as the control-surface snapshot entry
point: it observes the pairing owner's monotonic clock and reads the output
lifecycle from the bridge's own mixer session before returning the redacted
`DesktopControlSnapshot`, so a future Tauri command can consume a synchronized
view.

Evidence produced so far:

- fixed-order rendering with a full slot table and per-slot fade/isolation unit
  tests, including a slot whose queue underflows without affecting other slots;
- per-slot gain and mute unit tests proving the bounded ramp scales or silences
  only the affected slot while other slots stay at unity;
- per-slot realtime counter unit tests proving underflow, refill-limit, and
  discontinuity events land only on the affected slot (and that metric-free
  slots render identically);
- a deterministic property test over random activate/deactivate/render
  sequences against an independent active-set model that also verifies each
  slot's underflow counter against the model under over-run renders, plus a
  limiter-ceiling composition test;
- admission overflow fails closed at the fixed bound;
- command-boundary unit tests proving FIFO drain within one render,
  once-per-callback application, fail-closed on a full command queue, fast
  rejection of out-of-range removes, tolerance of stale removes, and a
  concurrent-insert smoke test.
- app-side wiring tests proving admission and mixing on the next render,
  removal silence, slot reuse after a remove-before-render, fail-closed format
  and table-full admission, retryable command-queue-full, limiter-ceiling on
  the mixed sum, control-domain gain/mute affecting only the target stream, and
  per-slot underflow counters;
- app-level network-path failure injection: a rejected (unauthenticated) packet
  for one stream leaves the other slot's decoded audio rendering, with per-slot
  metrics distinguishing the failing stream;
- app-level decoder-path failure injection: an authenticated payload that the
  decoder rejects resets only that stream's decode path and leaves the other
  slot's decoded audio rendering, with per-slot metrics distinguishing the
  failing stream;
- decode-domain format adapter tests proving equal-rate passthrough,
  upsampling and downsampling interpolation, mono/stereo channel mapping,
  combined rate-and-channel conversion, partial-source retention across pump
  steps, oldest-block replacement on a saturated slot queue, epoch
  discontinuity discarding stale interpolation state, and drift-bound
  enforcement;
- app-side adapted-admission tests proving a 96 kHz stereo source renders as
  converted 48 kHz mono through a real mixer render, removal silence and slot
  reuse, fail-closed admission on invalid source queue capacity or source
  format without reserving a slot, and per-slot underflow isolation for a
  starved adapted stream;
- app-side session-wiring tests proving adapted admission through a real
  decode pipeline, removal silence, fail-closed source-block and duplicate
  validation, output recovery re-admitting persisted streams with fresh
  producers, per-slot underflow isolation across two admitted streams, and
  stop/recreate lifecycle;
- app-side per-stream drift tests proving bounded ppm publication through
  the session adapter, high/low emergency resyncs retiring the adapter
  epoch and clearing the correction, per-stream drift isolation across two
  admitted streams, fail-closed updates after removal, history clearing
  for a recovered stream, and per-stream drift metrics (ppm, hard resyncs,
  queue occupancy) published from each decision;
- app-side control-plane tests proving multiple concurrently active
  streams with bounded admission, per-stream stop leaving other streams
  streaming, Ready/Streaming lifecycle transitions on the first and last
  stream, fail-closed capacity, and per-stream key rotation;
- app-side bridge tests proving the coordinator and mixer session compose
  through one object: the output starts with the first stream and stops
  with the last, a second stream admits without restarting the output,
  per-stream stop keeps the other stream and the output alive, in-place
  rotation preserves slot, gain, and rendering while a format change is
  rejected, output-start and admission failures roll back without
  partial changes, output fault and recovery re-admit persisted streams,
  and connection loss with reconnect or control expiry stops the mixer;
- app-side bridge honesty tests proving a lifecycle fault is never
  misrepresented: a platform fault that crosses reconnect grace retires
  the output and the snapshot reports it down, a reconnect-grace clock
  regression force-expires the mixer together with the session so the
  snapshot never claims a live stream or an active output under an idle
  lifecycle, and a recovery that crosses a retired lifecycle fails closed
  with both domains agreeing the output is down;
- a deterministic bridge property walk (`control_to_mixer_property.rs`)
  driving six seeds by 1,500 authenticated start, rotation, stop,
  output-fault recovery, reconnect-grace, and session-expiry steps and
  asserting after every step that the coordinator and the mixer report the
  same bounded stream count and that the bridge output-active flag never
  diverges from the mixer output snapshot;
- a libFuzzer target (`mixer_membership`) publishing arbitrary insert/remove
  commands into the bounded lock-free queue shared with the callback
  `CommandDrivenMixer` and interleaving renders, finite producer pushes, and
  bounded gain/mute changes, asserting the handoff boundary cannot panic,
  applies commands at callback granularity with the lowest-free-slot FIFO
  policy, fails closed on a full queue and on out-of-range removes, never
  exceeds the fixed slot bound, and keeps render output finite while
  producers carry finite samples;
- a deterministic membership-controller property walk
  (`mixer_membership_controller_property.rs`) driving six seeds by 3,000
  admission, adapted-admission, removal, render, and drain steps and
  asserting after every drain that the controller slot projection matches
  the mixer actual slot table exactly (active count, per-slot membership,
  and pending command count), that fail-closed admission and removal leave
  both sides unchanged, and that render output stays finite;
- a deterministic controller-driven network-simulator scenario
  (`mixer_controller_membership_scenario.rs`) driving two source streams over
  one shared virtual link into the same fixed-slot `CommandDrivenMixer`
  through the real `MixerMembershipController`, with a mid-session
  remove/re-admit churn, asserting after every render that the controller
  projection matches the mixer's actual slot table exactly (active count,
  per-slot membership, and pending commands), that shared-link loss leaves a
  starved slot silent without blocking the others, that bounded per-source
  gain and mute scripts applied through each admitted slot's real
  `GainControlHandle` publish without stalling membership and survive the
  churn (the scenario retains one handle per stream and clones it into every
  admission), and that the bus is exactly silent once every muted slot has
  rendered its bounded gain ramp, and that render output stays bounded and
  finite;
- a deterministic network-simulator scenario
  (`mixer_membership_scenario.rs`) driving two source streams over one
  shared virtual link into a fixed eight-slot `CommandDrivenMixer` with a
  mid-session remove/reinsert churn, asserting that every stream's playout
  reaches the mixer, that a slot starved by shared-link loss records PCM
  underflows while the other slots keep rendering, that membership
  transitions publish through the bounded queue (three inserts, one
  remove), that the active slot count returns to the full stream count
  after the reinsert, and that render output stays bounded and finite;
- a deterministic network-simulator property walk
  (`mixer_membership_property.rs`) driving two seeded runs by 128
  shared-link configurations and asserting that every accepted report
  consumes every frame through playout or PLC, submits exactly one packet
  per stream per frame, keeps both streams active after the mid-session
  reinsert, publishes two starts plus one reinsert and one remove, and
  renders a finite bus while identical configs produce identical reports,
  plus a libFuzzer target (`mixer_membership_scenario`) asserting the same
  transport, membership, and bounded-bus invariants on byte-driven
  configurations while rejected configurations return clean scenario
  errors;
- a deterministic combined jitter-scheduler and mixer scenario
  (`mixer_jitter_scheduler_scenario.rs`) driving two source streams over one
  shared virtual link through ADR 0016's target-gated playout into the same
  fixed-slot `CommandDrivenMixer`, asserting that a gated or starved slot
  never blocks the other slots, that bounded per-source gain and mute scripts
  applied through each slot's real `GainControlHandle` publish without
  stalling playout, and that the bus is exactly silent once every muted slot
  has rendered its bounded gain ramp;
- deterministic per-source quality ratios on both shared-link scenarios (PLC,
  late, reorder, duplicate, PCM-underflow, and discontinuity, plus
  forced-playout where the gate deadline exists) derived exactly from the
  per-slot raw counters and reported in milli-units, with unit, property
  (`mixer_controller_membership_property.rs`,
  `mixer_jitter_scheduler_property.rs`), and libFuzzer
  (`mixer_controller_membership`, `mixer_jitter_scheduler`,
  `mixer_jitter_scheduler_4s`) coverage asserting every ratio stays within
  its bound and that a clean link reports zero concealment;
- a libFuzzer target (`control_to_mixer`) feeding arbitrary byte-driven
  lifecycle scripts through the real pairing path and the tokenized
  completion contract and asserting the same cross-domain invariants and
  the fixed slot bound on every input;
- a deterministic mixer-output property walk
  (`mixer_output_controller_property.rs`) driving 250k seeded start,
  monitor, recover, and stop steps against an independent spec model and
  asserting after every step that the membership handle and the output
  format exist exactly while the stream is active, that a fault is
  reported at most once and retires both, and that recovery reuses the
  last successful selection;

Still open before Phase 2 can claim multi-source streaming: the roadmap's 24-hour
soak and hardware callback timing on actual WASAPI devices.

## Rollback plan

The membership boundary lives behind a small realtime module. Switching from
fixed slots to a preallocated snapshot/RCU does not change the per-stream queue,
decode, gain, or platform APIs. The single-stream MVP path is unaffected by this
record.
