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
record stays Provisional until then.

## Rollback plan

The controller exports a small configuration and decision type rather than a
scheduler or a buffer policy. A future measured controller may replace its
internal policy while retaining the same bounded `[min, max]` contract, or
introduce a new explicitly negotiated mode. The `JitterBuffer` container and
the realtime path are unaffected.
