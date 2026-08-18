# ADR 0016: Adaptive jitter target control

- Status: Provisional pending Phase 2 jitter evidence
- Date: 2026-08-13

## Context

The bounded `JitterBuffer` (sonimesh-audio) is a container: it owns ordering,
duplicate handling, and bounded overflow, but scheduling time and adaptive
target selection live above it. The architecture
([section 11](../architecture.md)) requires three user-selectable modes —
Low latency 20–40 ms, Balanced 40–70 ms (default), Stable 80–120 ms — and an
adaptive controller that changes slowly, exposes minimum/target/maximum water
levels, and resets its integral state after a stream reset. Packet arrival
jitter is a transport signal; PCM queue occupancy reflects the downstream
clock relationship and belongs to the drift controller
([ADR 0007](./0007-drift-control-decision.md)). The two signals must not be
combined in one fast control loop.

No playout scheduler exists yet, and no hardware jitter evidence is selected
by this record. This record fixes the target-controller boundary before that
scheduler starts; it does not change the `JitterBuffer` container or the
single-stream MVP path.

## Decision

Each stream owns a non-realtime `AdaptiveJitterTarget` controller in
`sonimesh-audio`. The controller converts the mode's millisecond range and a
configured frame duration into bounded integer frame targets, then moves the
current target within `[min, max]` by at most one frame per observation:

- a slow exponential moving average of packet arrival jitter is the transport
  input;
- when smoothed jitter exceeds the current target (underrun risk), the target
  grows one frame after a short debounce window, up to `max`;
- when smoothed jitter is below half the current target AND the buffered
  encoded-frame count exceeds the target with surplus, the target shrinks one
  frame after a longer debounce window, down to `min`;
- otherwise the target holds, and both debounce counters reset so stale
  pressure cannot move the target later.

The dead zone between `target / 2` and `target`, the asymmetric debounce
(shrink slower than grow), and the one-frame step give the required slow,
non-oscillating behavior. A stream reset clears the jitter estimate, both
debounce counters, and returns the target to the mode base.

Configuration is fixed and validated before streaming: the frame duration is
bounded, the buffer capacity is nonzero, the derived minimum is at least one
frame, and the derived maximum does not exceed the buffer capacity or the
controller hard bound. The controller never consumes PCM queue occupancy as a
jitter signal (that is the drift controller's input); it observes only packet
arrival jitter and the encoded-frame water level.

The controller exposes minimum/target/maximum water levels in frames and
milliseconds for diagnostics. It returns a small `JitterTargetStep` decision
(`Raised`, `Lowered`, `Held`) so a future scheduler or diagnostics can observe
each update without reading controller internals.

## Alternatives considered

- **Static target per mode:** simplest, but cannot adapt to a WLAN whose jitter
  varies across a session; underruns or excess latency would be structural.
- **Fast proportional control on raw jitter:** risks audible target oscillation
  and overreacts to single spikes; the architecture explicitly requires slow
  change.
- **Occupancy-driven target:** would fold the drift signal into the jitter
  controller, contradicting ADR 0007's separation of transport jitter from the
  downstream clock relationship.

## Security and realtime implications

The controller allocates no memory after construction and cannot request an
unbounded target: every value is integer-bounded and validated. It is a
scheduling-domain primitive and must never be called from the output callback.
Target values are bounded by the buffer capacity, so a scheduler cannot be
asked to wait for frames that cannot fit.

## Validation evidence

Tests must cover configuration bounds, preset values, the one-frame step
limit, debounce and hysteresis (no oscillation on alternating observations),
min/max clamping under sustained pressure, surplus gating on shrink, and full
reset semantics. A deterministic property test over seeded random jitter
sequences must assert the target stays within `[min, max]`, moves at most one
frame per observation, and returns to base after reset.

The required unit and property tests now exist in `sonimesh-audio`
(`jitter_target.rs` unit tests and `tests/jitter_target_property.rs`), and
the deterministic network simulator adds a jitter-target scenario
(`simulate_jitter_target`) that feeds the controller from virtual-link
arrival jitter and the encoded-frame water level and reports the bounded
target trajectory (final target, raised/lowered steps, smoothed jitter).
Measured WLAN/hardware jitter evidence remains pending Phase 2; this
record stays Provisional until then. A combined multi-stream scenario
(`simulate_multi_stream_jitter_target`) runs two to four streams over one
shared link with independent per-stream targets, so correlated loss and
bandwidth serialization are observed against per-stream adaptive jitter
control. The per-stream encoded-frame water level the controller observes
is also published as a diagnostics metric: each stream snapshot carries
the current jitter occupancy and its epoch high-water mark as one coherent
(frames, high-water) pair, while the playout scheduler that consumes the
adaptive target remains future Phase 2 work. The diagnostics contract for
that controller is already in place: each stream snapshot and export
bundle carries the active target state (target/min/max frames, smoothed
jitter in milliseconds, raised/lowered step counts), so a future
controller only has to publish through the stream counters. The receiver
pipeline (`ReceiverStreamPipeline` and the UDP receiver) now optionally runs
`ArrivalJitter` and `AdaptiveJitterTarget` against live authenticated
packets with a caller-supplied monotonic arrival clock, so the controller is
fed from a real packet stream without a shared clock and its active target
state reaches the stream counters. A deterministic target-gated scheduler
simulation (`simulate_jitter_scheduler`) now exercises that
consumption loop in the network simulator: the scheduler waits while fewer
than the target frames are buffered, drains without gating once no frames
are in flight, and forces a playout when the gate deadline expires, so the
controller can gate playout without an unbounded mid-stream stall and every
configured run completes. A combined multi-stream scenario
(`mixer_jitter_scheduler_scenario.rs`) now ties that target-gated scheduler
to the fixed-slot mixer over one shared link and additionally publishes
bounded per-source gain, mute, and quality evidence
([ADR 0015](./0015-realtime-stream-membership.md)). The bounded
target-gated decision primitive now lives in `sonimesh-audio` as
`TargetGatedPlayout`: it returns the per-tick `Play`/`Gate` decision, owns
the bounded counters (consecutive-gated streak capped at the configured
deadline, total gated ticks, and forced playouts), resets for a fresh
stream epoch, and is covered by unit tests, a deterministic seeded property
walk (`tests/playout_scheduler_property.rs`), and a dedicated libFuzzer
target (`target_gated_playout`) that asserts the streak, cumulative-gate,
and forced-playout invariants on arbitrary per-tick inputs. Both
deterministic
scheduler scenarios (`jitter_scheduler_scenario.rs` and
`mixer_jitter_scheduler_scenario.rs`) now drive every playout tick through
that primitive. The product output-callback wiring that consumes the
scheduler remains Phase 2 work.
Deterministic property walks (`jitter_scheduler_property.rs`,
`multi_stream_jitter_target_property.rs`) and libFuzzer targets
(`jitter_scheduler`, `multi_stream_jitter_target`) now drive the
target-gated scheduler and multi-stream adaptive-target scenarios from
seeded and arbitrary byte inputs and assert the same bounded-target,
step-count, and completion invariants on every report.

The completion bound is explicit: the `jitter_scheduler_property` and
`mixer_jitter_scheduler_property` walks and the `jitter_scheduler`,
`mixer_jitter_scheduler`, and `mixer_jitter_scheduler_4s` fuzz targets
assert every accepted report's `excess_latency_ms` stays within the
scenario hard deadline (`frames * (max_gate_ticks + 2) *
frame_duration_ms` plus the worst-case one-way network delay), and the
CI smoke gate pins the same bound on the seeded `scheduler` and
`mixer_jitter_scheduler` smoke lines.

A bounded relative arrival-jitter estimator in `sonimesh-audio`
(`ArrivalJitter`) measures per-packet inter-arrival deviation from a
monotonic arrival clock and the sender sample timestamps, so a real packet
stream can feed the controller without a shared clock; its unit and
property tests pin the exact injected-variation reporting.

## Rollback plan

The controller exports a small configuration and decision type rather than a
scheduler or a buffer policy. A future measured controller may replace its
internal policy while retaining the same bounded `[min, max]` contract, or
introduce a new explicitly negotiated mode. The `JitterBuffer` container and
the realtime path are unaffected.
