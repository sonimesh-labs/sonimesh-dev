# ADR 0007: Bounded PCM occupancy drift control

- Status: Accepted for controlled-lab audio pipeline work
- Date: 2026-07-17

## Context

Android capture and desktop output clocks are independent. Fixed jitter and PCM
buffers eventually underflow or overflow if a small persistent rate difference
is ignored. Packet jitter is a transport signal, while PCM occupancy reflects
the downstream clock relationship; combining them in one fast control loop
causes audible target oscillation.

No real ASRC library or output device timing evidence is selected by this record.
The project needs a deterministic control boundary first, so that any future
ASRC accepts a bounded ratio command rather than embedding policy in a decoder or
realtime callback.

## Decision

Each stream owns a non-realtime PI controller driven by periodic PCM queue
occupancy snapshots. The controller returns one of:

- a signed ratio adjustment in parts per million (ppm), where a positive value
  means consume PCM faster because occupancy is above target;
- a bounded hard-resynchronization request when occupancy reaches the configured
  emergency low or high threshold.

Controller configuration has fixed bounds for target occupancy, hard thresholds,
proportional gain, integral gain, integral accumulator, maximum ratio, and
maximum per-update ratio step. Validation rejects any configuration that would
remove the target safety margin or permit an unbounded correction. Integral
updates use anti-windup: when output is clamped and the current error would push
farther into that clamp, the integral accumulator is not advanced.

A hard resynchronization resets controller history. The stream owner invokes its
stream-local hard-resynchronization boundary to discard undecoded jitter, reset
decoder state, and retire queued PCM from the active output epoch. The
queue-backed render source applies its bounded discontinuity fade-in when it
observes the resulting epoch gap; platform stream recreation remains a
control-domain responsibility. The output callback receives only precomputed
PCM blocks and never reads controller state.

`sonimesh-app::StreamDriftController` is the control-domain composition point:
it samples the receiver pipeline's bounded PCM occupancy, applies ordinary ppm
decisions through the source handle, and invokes the stream-local cleanup path
when the decision requires a hard resynchronization.

The nominal sample-rate source may expose a control-domain adjustment handle.
An `Adjust` decision is published through a bounded atomic ppm value that the
source observes on its next callback; the callback never invokes the controller.
A `HardResync` decision is returned to the stream owner and is never silently
applied to a running source.

Low-rate diagnostics publish the nominal input/output sample rates as one atomic
pair so a device-format recreation cannot present a ratio assembled from two
different output lifetimes. Counter snapshots otherwise remain non-transactional
telemetry and must not drive control decisions.

The initial controller works in integral block units and ppm values. It does not
claim that the selected gains or thresholds meet a hardware latency target.

## Security and reliability implications

Occupancy input is bounded by the fixed PCM queue capacity. Invalid configuration
is rejected before streaming. The controller allocates no memory after
construction and cannot request unbounded rate changes. A stream-local hard
resynchronization must not affect other streams.

## Validation evidence

Tests must cover configuration bounds, sign convention, slew limiting,
anti-windup, and history reset after either hard resynchronization direction.
A deterministic integerized closed-loop simulation now exists in
`tools/network-simulator` (`drift_scenario.rs`), with a seeded property walk
(`drift_scenario_property.rs`) and a `drift_scenario` libFuzzer target
asserting every accepted report stays deterministic, keeps occupancy within
the bounded PCM queue, keeps the final ratio within the configured cap, and
keeps each hard resync exactly one low- or high-side reset. Hardware tests
are still required before selecting gain defaults or making drift or latency
claims.

A deterministic multi-stream mixer-drift scenario now extends that
evidence to the Phase 2 source set: `simulate_mixer_drift` in
`tools/network-simulator` closes one independent integerized occupancy loop
per stream (two to four streams) with a private controller instance per
stream, and asserts the per-stream isolation property end to end. A seeded
property walk (`mixer_drift_scenario_property.rs`) and a
`mixer_drift_scenario` libFuzzer target drive random per-stream source rates
and initial occupancies and assert that every stream's occupancy stays within
the bounded PCM queue, every final ratio stays within the configured cap,
every hard resync is exactly one low- or high-side reset, every stream's
report equals the single-stream `simulate_drift` result for that stream's own
configuration, and a clean no-drift stream never requests a resynchronization
even while uncorrectable siblings do. Hardware tests are still required before
selecting gain defaults or making drift or latency claims.

A real-component closed-loop test now drives the actual receiver pipeline,
polyphase format adapter, fixed-slot mixer, and `MixerStreamDriftController`
under a sustained virtual-clock skew
(`crates/sonimesh-app/tests/mixer_stream_clock_skew.rs`): each control tick
decodes a 10 ms source burst at the skewed rate, samples the adapter source
occupancy, applies one decision, pumps the output blocks, and renders them.
The test asserts that a nominal clock stays at the target without correction
or resynchronization, that within-cap fast and slow sources publish
corrections with the correct sign while occupancy stays inside the emergency
band and rendering continues, that a skew beyond the adjustment cap degrades
to bounded hard resynchronizations without escaping the bounded PCM queue,
and that the whole run is deterministic. Because the adapter consumes whole
input frames per pump and the controller samples whole-block occupancy, ppm
authority is quantized and the real loop converges over long timescales; this
evidence covers boundedness and correction direction, while convergence-rate
and gain defaults remain hardware-gated.

## Rollback plan

The controller exports a small decision type rather than an ASRC implementation.
A future measured controller may replace its internal math while retaining the
same bounded decision contract, or introduce a new explicitly negotiated mode.
