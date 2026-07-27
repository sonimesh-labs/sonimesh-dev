# Testing and Measurement

SoniMesh should promote a platform capability only after a repeatable test
produces evidence. This document separates deterministic tests from hardware-in-
the-loop validation.

## Latency measurement

### Software timing

Record monotonic timestamps for:

- capture read completion;
- frame aggregation;
- encode completion;
- send enqueue;
- receive;
- authentication completion;
- jitter-buffer playout decision;
- decode completion;
- ASRC output;
- output-ring enqueue;
- output callback consumption.

These timestamps explain the pipeline but do not, by themselves, prove acoustic
end-to-end latency because Android and Windows clocks are independent.

### Hardware or acoustic timing

Use a deterministic pulse or MLS signal that is capturable by the Android source
and record both the source reference and desktop output through a common audio
interface or calibrated acoustic setup. Compute the correlation offset and report
p50, p95, and p99 over multiple runs.

Every report must include Android model and build, Windows version, Wi-Fi AP,
network band, output device, sample rate, channel count, buffer mode, frame size,
Opus bitrate, jitter mode, and limiter configuration.

The UI should expose a playout-delay estimate and its components. A physical
measurement report should be the source of truth for the product's end-to-end
latency claim.

## Deterministic unit and property tests

Cover at least:

- packet header encoding, decoding, byte order, and size limits;
- published Rust/Kotlin vectors for media key derivation, nonce construction, and
  AEAD packet authentication;
- published Rust/Kotlin vectors for the fixed TLS 1.3 exporter inputs,
  role-separated pairing proofs, and one-time media-root derivation; a vector
  result does not qualify TLS-provider exporter support or QR SPKI pinning;
- pairing expiry, single use, bounded source/attempt admission, and proof-verifier
  gating before a session can become trusted;
- sequence exhaustion at the epoch boundary, duplicate detection, and replay
  windows;
- nonce uniqueness across streams, directions, epochs, and reconnects;
- Protobuf size limits, unknown fields, capability negotiation, typed status/error
  semantics, bounded TCP framing under partial closure and permanent I/O failure,
  process-unique control-transport and asynchronous-operation tokens
  across coordinators and reconnects, pre-hello start/stop rejection,
  exact-payload duplicate command handling,
  and published Rust/Kotlin control-frame vectors including the exact media key
  epoch;
- jitter ordering, late packets, overflow, resynchronization, and bounded memory;
- PLC/FEC scheduling and frame-duration transitions;
- drift-controller convergence, limits, anti-windup, and reset;
- channel mapping, gain ramps, fades, clipping protection, and limiter behavior;
- coherent PCM queue level/high-water snapshots and saturating cumulative
  diagnostics counters under concurrent publication;
- all lifecycle-state transitions, including invalid transitions;
- output-device fallback and stream-recreation behavior.

Property and fuzz tests must guarantee that arbitrary bytes cannot panic, allocate
unbounded memory, or reach the decoder before authentication and length checks.
Authenticated malformed Opus payloads must also be tested because a paired peer
may be compromised.

The Rust foundation keeps deterministic adversarial-input regressions for bounded
control/media parsing and invalid AEAD payloads. The independent `fuzz/`
workspace adds LibFuzzer targets for arbitrary control frames, mutations of
all five published control payload shapes, near-valid media packets, and
protected ingress. The ingress target also constructs valid
AEAD packets carrying arbitrary bounded payload bytes, retries them as replay,
uses undersized output storage, mutates one authenticated header/ciphertext/tag
byte, and requires every rejected path to clear plaintext.

Linux CI is configured to compile these targets with AddressSanitizer and
perform 512 executions per target as a finite smoke gate. A successful job shows
that the harnesses remain buildable under the pinned toolchain and catches
immediate regressions; it is not a sustained fuzz campaign, coverage acceptance
result, physical-network test, or malformed-input qualification of a future
native Opus decoder.

Deterministic audio tests also cover the provisional bounded channel mapper and
its callback source adapter. They do not qualify physical speaker positions or
OEM-specific channel routing.

They also cover nominal linear sample-rate conversion and phase continuity. They
do not qualify filter quality, drift correction, acoustic latency, or long-run
rate stability.

The source adapter tests cover atomic ppm adjustment publication and explicit
hard-resync handoff; they do not prove controller gains on a physical device.

## Network simulation

The protocol and audio crates should run against an in-memory virtual transport
with a deterministic clock. Inject:

- 0%, 1%, 3%, and 5% random loss;
- burst loss;
- 10–100 ms base delay;
- jitter, duplication, reordering, and bandwidth limits;
- control-channel loss while UDP remains reachable;
- address changes and session restart.

Physical WLAN tests are still required, but deterministic simulation should be
the first regression signal in CI.

### Current deterministic harness

The buildable harness in `tools/network-simulator` currently provides a virtual
monotonic clock, bounded in-flight event queue, seeded random and burst loss,
jitter, duplication, reorder delay, byte-counted bandwidth serialization, and
queue-capacity pressure. Its audio scenario accepts an explicit complete media
datagram size bounded by the protocol limit and uses the negotiated 10/20 ms
frame timing model. A cross-crate
test exercises bounded packet parsing, reviewed AEAD authentication and replay
rejection, jitter/PLC decisions, controlled jitter resynchronization, the
non-realtime decode worker, and realtime PCM transfer. An independent integerized
occupancy model exercises bounded drift controller decisions and stream-local
hard-resynchronization requests. A separate deterministic case drops a valid
control envelope while an independent authenticated media path still reaches PCM.

A deterministic virtual-endpoint test now applies the shared network-domain
source-pin policy around the protected receiver pipeline. It verifies that
rejected input cannot pin, a changed endpoint is rejected before consuming
replay state, epoch rotation preserves the binding, failed session replacement
preserves it, and successful fresh-session replacement permits rebinding.
Separate loopback UDP tests exercise the same policy through real host sockets.

A deterministic control reconnect scenario now delays a command from the old
transport until after the replacement transport installs a fresh local token and
completes hello negotiation. The old token and stale completion are rejected,
while the same command through the fresh token is admitted.

This is evidence for deterministic core behavior and host loopback composition
only. It does not yet model socket buffers, real TLS/socket reconnection, real
ASRC/device clocks, operating-system scheduling, NAT rebinding, or physical
network-interface changes, and it is not physical WLAN evidence.

## Stability and compatibility matrix

Before MVP release:

- 8-hour continuous playback on the reference matrix;
- 24-hour clock-drift soak before multi-source release;
- at least 500 connect/disconnect cycles;
- output-device unplug/replug;
- sleep/wake and user-session changes;
- Android lock, backgrounding, service recreation, projection revocation, and
  teardown races between outcome publication, reader exit, duplicate stops, and
  fresh starts delivered while the old service generation is retiring;
  read-error accounting for negative return codes, thrown read-call exceptions,
  and invalid positive sizes before sample access; rejection of negative capture
  evidence timestamps while nonnegative snapshot regressions clamp duration to
  zero and rebase the low-rate publication cadence; sequential and concurrent
  evidence-counter saturation without signed wraparound; injected ordinary exceptions
  in each final cleanup step; and
  exact bounded ordinary-failure counts with no exception text; and deferred
  propagation of fatal cleanup errors;
- representative Android OEMs across the supported baseline;
- Windows shared-mode sample rates and USB/Bluetooth output devices;
- private/public network profiles and firewall approval/denial.

## CI versus hardware-in-the-loop

CI should run pure protocol/audio logic, fuzz targets, virtual network tests,
serialization vectors, UI tests, lint, format, type checks, and cross-platform
builds. It should not claim to validate physical audio latency without hardware.

Hardware-in-the-loop tests should run on a documented lab matrix and publish
artifacts containing raw metrics, configuration, software versions, and failure
classification.

The first Windows CPAL silent-output smoke is recorded in
[2026-07-17 Windows CPAL smoke](./measurements/2026-07-17-windows-cpal-smoke.md).
It validates callback observation only, not a latency target.

A subsequent read-only endpoint enumeration is recorded in
[2026-07-18 Windows CPAL probe](./measurements/2026-07-18-windows-cpal-probe.md).
It validates only current WASAPI endpoint/default-format discovery and does not
open an output stream.

A current-default silent stream run is recorded in
[2026-07-18 Windows CPAL smoke](./measurements/2026-07-18-windows-cpal-smoke.md).
It confirms the bounded callback observation path only, not latency or output
device qualification.

## Resource and power acceptance

The Android sender must record CPU, memory, thermal state, and battery impact
while the screen is locked. The desktop must record CPU, memory, thread count,
network rate, and output callback underruns. “Recorded” is not an acceptance
criterion by itself; the release plan must set thresholds after the first PoC
establishes a realistic baseline.
