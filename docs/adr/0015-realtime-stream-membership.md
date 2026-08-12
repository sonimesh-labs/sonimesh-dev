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

## Rollback plan

The membership boundary lives behind a small realtime module. Switching from
fixed slots to a preallocated snapshot/RCU does not change the per-stream queue,
decode, gain, or platform APIs. The single-stream MVP path is unaffected by this
record.
