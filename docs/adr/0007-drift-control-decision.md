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

A hard resynchronization resets controller history and leaves buffer flushing,
fade behavior, and decoder/ASRC recreation to the stream owner. The output
callback receives only precomputed PCM blocks and never reads controller state.

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
Deterministic closed-loop simulation remains required before selecting gain
defaults; hardware tests are still required before making drift or latency claims.

## Rollback plan

The controller exports a small decision type rather than an ASRC implementation.
A future measured controller may replace its internal math while retaining the
same bounded decision contract, or introduce a new explicitly negotiated mode.
