# ADR 0016: Adaptive jitter target control

- Status: Provisional pending Phase 2 jitter evidence
- Date: 2026-08-19

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

No hardware jitter evidence is selected by this record. The bounded
`TargetGatedPlayout` scheduler now exists as a scheduling-domain primitive, and
`ReceiverStreamPipeline` exposes it through an explicit opt-in boundary; the
single-stream default path remains unchanged until a production scheduler
chooses and supplies the in-flight-arrival signal.

The desktop combined UDP tick cannot infer that signal from a successful socket
poll: a received packet says nothing about future arrivals. It therefore uses
the immediate-drain path even when target gating is enabled. An explicit caller
that knows the arrival state may still use the gated decode boundary.

The desktop receiver also exposes a monotonic-time scheduling path. Each call
polls at most 16 UDP datagrams, services control and maintenance independently,
and makes at most one immediate-drain decode decision per call when the matching
active stream's negotiated frame deadline is due. It preserves frame-clock
phase, skips missed deadlines, and spaces decisions at least half a frame apart
to bound catch-up. The caller must poll more often than the frame interval;
otherwise a single decision per call cannot sustain the sender's frame rate.
When an active stream's scheduling calls pause for at least 100 ms (or eight
frame durations, whichever is longer), the scheduler retires its decoder,
source and mixer queue epochs and drains the UDP socket in bounded batches.
It discards the accumulated encoded frames when the socket becomes empty and
resumes on subsequent packets. A continuously backlogged socket delays recovery;
the host must keep polling. This deliberately sacrifices some audio around a
stall to avoid retaining the accumulated delay. A regressing clock suppresses
decode until time catches up, and an ongoing drain waits for that point before
finishing. Other admitted streams continue to be pumped during the drain. Audio
already handed to an output callback or device cannot be withdrawn. Stopped, unmatched, or
output-unavailable streams reset their cadence. This is software scheduling
behavior, not a latency or physical playback measurement.

The scheduled desktop receiver also retires its stream-local decode and mixer
epochs after three consecutive concealed frames when the jitter buffer is empty.
It does so at the next playout deadline before polling new packets, allowing the
next authenticated packet to establish a fresh sequence after an interruption.
Buffered future frames remain eligible for normal PLC traversal. This fixed
recovery policy is provisional and is not an adaptive jitter target.

The separate paired audio lab schedules Opus PLC at each live playout deadline,
including when its jitter buffer is empty. After three consecutive concealed
frames, it resets an empty stream on the next deadline and waits for a fresh
packet; buffered later frames are still reached through PLC. This lab policy is
not an adaptive jitter target or a measured quality guarantee.

The paired audio lab may explicitly select an experimental Balanced adaptive
target instead of its default fixed scheduling policy. It treats arrivals as
pending while the stream is active and gates for at most three 20 ms ticks per
decision. A gated tick produces no PCM and can therefore cause output silence;
the scheduler does not accelerate playout when the target falls. After three
consecutive PLC decisions and an empty buffer it still resets and waits for a
fresh packet. Stop draining uses immediate decode rather than gating. This is
an experiment, not evidence of improved quality or latency.

The paired audio lab uses a bounded best-effort stop drain: it polls
UDP for trailing packets, continues paced decode, and gives queued PCM time to
reach the output before stopping. A quiet interval ends the drain, and an
absolute deadline limits it even under continuous traffic. UDP has no end marker;
neither the drain nor its acknowledgement proves that hardware played the tail.

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
each update without reading controller internals. Adaptive water levels,
controller observations, and the target-gated scheduler's gated-tick and
forced-playout counters share one versioned telemetry publication, so low-rate
snapshots cannot combine a newly cleared target epoch with retained scheduler
counters. Explicit scheduler disable deliberately clears only the scheduler
counters while retaining the active target state; all of this remains owned by
the non-realtime scheduling domain.

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
(frames, high-water) pair, while production output-callback adoption of the
playout scheduler remains future Phase 2 work. The diagnostics contract for
that controller is already in place: each stream snapshot and export
bundle carries the active target state (target/min/max frames, smoothed
jitter in milliseconds, raised/lowered step counts) plus target-gated ticks and
forced playouts for the active scheduler epoch, so the receiver boundary can
publish scheduling evidence through the stream counters. The receiver
pipeline (`ReceiverStreamPipeline` and the UDP receiver) now optionally runs
`ArrivalJitter` and `AdaptiveJitterTarget` against live authenticated
packets with a caller-supplied monotonic arrival clock, so the controller is
fed from a real packet stream without a shared clock and its active target
state reaches the stream counters. The application integration test
`receiver_pipeline_adaptive_target.rs` drives two authenticated streams with
independent calm and impaired arrival histories and resets them one at a time,
proving target trajectories and epoch retirement remain stream-local. It also
proves a rejected reused session ID preserves a populated target epoch, while a
valid fresh session clears it and rebuilds from a new arrival baseline. Its UDP
loopback companion additionally proves that valid fresh-session replacement
clears the adaptive target and source pin together before the first new-session
packet rebinds the authenticated endpoint. The same application test forces a
bounded jitter-buffer resynchronization and proves that its accepted packet is
immediately recorded as the first arrival baseline of the new adaptive epoch,
so the first post-resync interval is measured instead of being skipped. A
deterministic target-gated scheduler simulation (`simulate_jitter_scheduler`)
now exercises that
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
that primitive. `ReceiverStreamPipeline` and its UDP wrapper now expose an
explicit `enable_target_gated_playout` plus
`decode_next_with_arrivals_pending` boundary: a caller-owned packet scheduler
supplies whether authenticated arrivals are still pending, the pipeline gates
below-target decode ticks within the fixed deadline, and the ordinary decode
method remains an immediate drain path. This is software scheduling evidence,
not proof of a production output-callback integration or physical latency.
The default single-stream path stays ungated until a platform/control owner
adopts the boundary deliberately.
The paired audio lab uses its own monotonic 20 ms playout loop for both fixed
and adaptive experiments. After a late decision, it skips missed deadlines and
waits at least half a frame before the next decision, shifting the next deadline
when necessary. This avoids near-back-to-back catch-up decisions; it is a lab
scheduling policy, not an acoustic latency or physical WLAN claim.
Deterministic property walks (`jitter_scheduler_property.rs`,
`multi_stream_jitter_target_property.rs`) and libFuzzer targets
(`jitter_scheduler`, `multi_stream_jitter_target`) now drive the
target-gated scheduler and multi-stream adaptive-target scenarios from
seeded and arbitrary byte inputs and assert the same bounded-target,
step-count, and completion invariants on every report. The end-to-end
`receiver_pipeline` and `udp_receiver_pipeline` fuzz targets also enable the
opt-in live controller with input-derived arrival clocks and check target
bounds and epoch reset state alongside their protected-ingress, source-pin,
and decode invariants. The `receiver_pipeline` target additionally forces a
fixed-capacity jitter resynchronization on every input and verifies that the
accepted resync packet publishes a clean target epoch and becomes the
immediate arrival baseline. The UDP target now drives the same resynchronization
through the loopback socket and checks the source pin, PCM discontinuity, and
controller state at that boundary.

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
