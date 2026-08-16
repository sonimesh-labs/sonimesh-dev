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

The `tools/latency-measurement` crate implements this workflow: it generates a
deterministic MLS reference WAVE file, reads WAVE recordings and reports each
recording's declared properties (sample rate, channel count, bit depth, format
tag, and length) for pre-measurement validation, estimates the correlation
offset, and prints per-run offsets plus a p50/p95/p99 summary. It is a software
measurement aid; the acoustic setup and the report metadata above still define
what the result means.

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
- sequence exhaustion at the epoch boundary (pinned by the host-JVM
  `MediaSequence` mirror), duplicate detection, and replay windows;
- nonce uniqueness across streams, directions, epochs, and reconnects;
- UDP source-endpoint pinning: bind only after a protected packet succeeds,
  reject a changed endpoint before any processing, keep automatic same-session
  address migration disabled across key epochs, and reset the pin only for a
  fresh media session;
- Protobuf size limits, unknown fields, capability negotiation, typed status/error
  semantics, preserved media receiver, control-plane, and output/render
  error-source chains,
  bounded TCP framing
  under partial closure and permanent I/O failure,
  process-unique control-transport and asynchronous-operation tokens
  across coordinators and reconnects, pre-hello start/stop rejection,
  exact-payload duplicate command handling,
  and published Rust/Kotlin control-frame vectors including the exact media key
  epoch;
- jitter ordering, late packets, overflow, resynchronization, and bounded memory;
- PLC/FEC scheduling and frame-duration transitions;
- drift-controller convergence, limits, anti-windup, and reset;
- channel mapping, gain ramps, fades, clipping protection, and limiter behavior
  (exact-ceiling peak limiter with sample-instant attack, smooth release, and
  linked interleaved frames);
- capacity-clamped PCM queue occupancy, bounded callback refill exhaustion,
  coherent level/high-water snapshots, and saturating cumulative diagnostics
  counters under concurrent publication;
- all lifecycle-state transitions, including invalid transitions;
- output-device fallback and stream-recreation behavior.

Property and fuzz tests must guarantee that arbitrary bytes cannot panic, allocate
unbounded memory, or reach the decoder before authentication and length checks.
Authenticated malformed Opus payloads must also be tested because a paired peer
may be compromised.

The Rust foundation keeps deterministic adversarial-input regressions for bounded
control/media parsing and invalid AEAD payloads. The independent `fuzz/`
workspace adds LibFuzzer targets across the control, media, and bounded
audio-DSP boundaries (the full inventory lives in `fuzz/README.md`): arbitrary
control frames, mutations of all five published control payload shapes and the
hello negotiation, near-valid media packets, protected ingress, the pure
pairing-binding verifier, and bounded length-prefixed control framing fed
through arbitrary chunk boundaries. The ingress target
also constructs valid AEAD packets carrying arbitrary bounded payload bytes,
retries them as replay, uses undersized output storage, deterministically mutates a
flags byte in the authenticated header and a tag byte on every input plus a ciphertext byte for nonempty
payloads, and requires every rejected path to clear plaintext.
The pairing target exercises both local roles, rejects reflected and altered
proofs, and checks arbitrary proof input against the exact opposite-role proof.
The realtime audio targets feed adversarial rate metadata, sample payloads,
occupancy snapshots, arrival ticks, and membership scripts into the bounded
nominal resampler, peak limiter, channel mapper, drift controller, fixed-slot
mixer, arrival-jitter estimator, adaptive target controller, and per-stream
decode worker, and the realtime PCM render source, asserting the fixed
capacity, consumed/produced, gain, and initialized-output guarantees hold
on every accepted or rejected step.
The render-source target additionally compares every rendered output slice
and the published underflow, discontinuity, and refill-limit counters
against an independent queue-plus-render model under arbitrary queue
pushes, epoch retirements, render lengths, gain, and mute scripts.
Separate lifecycle targets drive arbitrary start, monitor, recover, stop,
health, and fail-start scripts through the single-stream receiver output
controller and the Phase 2 mixer output controller, asserting after every
step that the platform call counts, producer replacements, membership-handle
and published-format presence, published output-format diagnostics, and
redacted snapshot match an independent state model, that a fault is reported
at most once and retires the stream, and that recovery reuses the last
successful selection.

A separate target drives the control-domain mixer membership boundary
(`mixer_membership_controller`): arbitrary fast-path and adapted
admissions, removals, finite producer pushes, bounded gain and mute
changes, renders, and drains against the callback fixed-slot mixer,
asserting after every drain that the controller projection matches the
real slot table exactly, that pending commands drain to zero, and that
fail-closed admission (format mismatch, table full, command queue full)
leaves both sides unchanged.

A network-domain sender target drives the bounded media session
sender boundary (`media_session_sender`): byte-selected sends, oversized
frames, valid and fail-closed key-epoch rotation, fresh-session
installation, and routing migration over real loopback sockets, asserting
after every step that every accepted seal consumed exactly the next
per-epoch sequence and reached the current peer's protected ingress with
the expected header and plaintext, that an oversized frame is rejected
before touching the socket without consuming a sequence, and that failed
rotation leaves the context and sequence unchanged.

The bounded diagnostic export journal (ADR 0011) has a dedicated target that
drives arbitrary record sizes and input-selected count and byte bounds,
comparing the result, retained count, retained bytes, and next sequence
against an independent spec model and proving oversized bundles are rejected
before any record and eviction keeps the newest exports within both fixed
bounds.

`tools/check_fuzz_targets.py` keeps the manifest registration, fuzz README
inventory and runnable commands, and finite CI smoke list synchronized. Linux
CI is configured to compile these targets with AddressSanitizer and perform 512
executions per target as a finite smoke gate. A successful job shows
that the harnesses remain buildable under the pinned toolchain and catches
immediate regressions; it is not a sustained fuzz campaign, coverage acceptance
result, physical-network test, or malformed-input qualification of a future
native Opus decoder.

Independent cross-language vector verifiers in `tools/` recompute the published
Rust vectors from the documented labels, constants, and byte layouts using only
the Python standard library: `check_pairing_vectors.py` (HMAC/HKDF-SHA256
pairing proofs and media root), `check_media_key_vectors.py` (media key
derivation and the 96-bit nonce layout for both directions),
`check_negotiation_vectors.py` (bounded version-one hello capability
negotiation), `check_control_frame_vectors.py` (Protobuf wire encoding of all
five control payloads),
`check_media_header_vectors.py` (the fixed 36-byte media header), and
`check_media_crypto_vectors.py` (RFC 8439 ChaCha20-Poly1305 AEAD packet
sealing for the three published vectors: derived media key, nonce,
authenticated header, ciphertext, and tag). The pairing, control-frame,
media-crypto, and negotiation verifiers also re-read the values pinned in the
vector docs and the Rust pins (`src/pairing_binding.rs`,
`tests/control_frame.rs`, `tests/media_crypto.rs`, and, for negotiation,
`src/control.rs`, `src/negotiation.rs`, and `tests/negotiation.rs`); the
media-key and media-header verifiers re-read the Rust pins in
`src/media_key.rs` and `tests/media_header_vector.rs`; all six verifiers
additionally re-read their Kotlin counterparts:
`PairingTlsBindingVectorTest`, `ControlFrameVectorTest`, `MediaCryptoVectorTest`,
`MediaHeaderVectorTest`, `MediaKeyVectorTest`, and, for negotiation, the
`HelloNegotiation` mirror with `HelloNegotiationVectorTest`. Each pinned value
must match the independent recomputation, so the documentation, the Rust and
Kotlin pins, and the Python verifier cannot drift apart.
Each script runs a `--self-test` mode and a check mode in CI; a schema
renumbering, layout reordering, label, salt, or byte-order change on either
side fails the job.

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
jitter, duplication, reorder delay, byte-counted bandwidth serialization,
queue-capacity pressure, and bounded sender-side socket-buffer backpressure.
Its audio scenario accepts an explicit complete media datagram size bounded by
the protocol limit and uses the negotiated 10/20 ms frame timing model. A
seeded property walk (`scenario_property.rs`) drives that audio scenario
under random bounded profiles, first with loss, reorder, duplication, and
jitter alone and then with a finite serial link and a bounded sender-side
socket buffer, and asserts determinism, playout-or-PLC accounting,
submission conservation, serialization and backpressure accounting
cross-checked against an independent byte-credit model, and the bounded
late/duplicate arrival relations. The scheduler, adaptive-jitter-target,
mixer-membership, multi-stream, mixer-controller-membership, and
mixer-jitter-scheduler walks extend the same treatment: each varies the media
datagram across the protocol range and occasionally configures the finite
serial link and a bounded sender-side socket buffer, pinning the serialization
accounting (one datagram per scheduled copy) and the no-link/no-buffer
backpressure semantics on every report. Those walks (and the base
`scenario_property` walk) also reproduce `send_buffer_full_packets` and
`send_buffer_queued_bytes` exactly with an independent byte-credit model that
replays each scenario's submission timeline at the link bandwidth; the two
scheduler walks stop the tick loop as soon as every frame is consumed, so the
mixer-jitter-scheduler report now exposes `excess_latency_ms` (the wall-clock
delay beyond the nominal playout completion) to recover the exact completion
tick. The eight network-scenario fuzz targets assert the same serialization
and backpressure accounting on every accepted report, so adversarial inputs
exercise the identical invariants the seeded walks pin. A smoke-output
regression test
(`smoke_output.rs`) pins what the CI smoke gate actually prints: the seeded
runner is byte-for-byte deterministic, every data line parses into
`key=value` fields, and the report sections satisfy their cross-field
accounting invariants, including a shaped finite-link profile with a bounded
sender-side socket buffer that exercises backpressure end to end. A cross-crate
test exercises bounded packet parsing, reviewed AEAD authentication and replay
rejection, jitter/PLC decisions, controlled jitter resynchronization, the
non-realtime decode worker, and realtime PCM transfer. An independent integerized
occupancy model exercises bounded drift controller decisions and stream-local
hard-resynchronization requests; a seeded property walk
(`drift_scenario_property.rs`) and a `drift_scenario` fuzz target drive random
tick counts, source rates, initial occupancies, and controller parameters and
assert every accepted report stays deterministic, keeps every occupancy
observation within the bounded PCM queue, keeps the final requested ratio
within the configured cap, and keeps each hard resync exactly one low- or
high-side reset. A deterministic multi-stream mixer-drift scenario
(`simulate_mixer_drift`) now extends that loop to two to four independent
Phase 2 source streams, each with its own bounded controller, and a seeded
property walk (`mixer_drift_scenario_property.rs`) plus a
`mixer_drift_scenario` fuzz target assert the per-stream isolation property
on every report: every stream's occupancy stays within the bounded PCM
queue, every final ratio stays within the configured cap, every hard resync
is exactly one low- or high-side reset, every stream's report equals the
single-stream `simulate_drift` result for that stream's own configuration,
and a clean no-drift stream never requests a resynchronization while
uncorrectable siblings do. A separate deterministic case drops a valid
control envelope while an independent authenticated media path still reaches PCM.

A deterministic virtual-endpoint test now applies the shared network-domain
source-pin policy around the protected receiver pipeline. It verifies that
rejected input cannot pin, a changed endpoint is rejected before consuming
replay state, epoch rotation preserves the binding, failed session replacement
preserves it, and successful fresh-session replacement permits rebinding.
Separate loopback UDP tests exercise the same policy through real host sockets.
A libFuzzer target (`receiver_pipeline`) drives byte-selected in-order
deliveries, duplicates, tampered ciphertext with a valid-twin probe,
retired-epoch packets, and rotations through the same protected pipeline,
asserting every outcome, plaintext zeroing, decoded PCM, and the
authenticated, duplicate, rejected, and stream-reset counters against an
independent replay oracle and counter model.

The UDP-backed composition has a dedicated target (`udp_receiver_pipeline`)
that adds real loopback sockets and the fail-closed source pin to the same
byte-driven mix, asserting every poll outcome, pin state, decoded PCM, and
diagnostic counter against an independent pin, replay, and counter model:
a changed endpoint is rejected before any protected work and never moves
the pin, failed processing never binds an unbound source, and only a fresh
authenticated session clears the binding.

A deterministic control reconnect scenario now delays a command from the old
transport until after the replacement transport installs a fresh local token and
completes hello negotiation. The old token and stale completion are rejected,
while the same command through the fresh token is admitted.

A deterministic snapshot scenario now arms a bounded reconnect window after a
connection loss and then advances only the snapshot clock: a caller that polls
`desktop_snapshot_at` sees the projected remaining time count down and the
session retire to idle at the deadline, without a separate grace observation.
Reconnecting through the same entry point clears the projected countdown and
returns the session to ready.

A deterministic recovery scenario now drives a full renegotiation through the
same bridge entry point: connection loss keeps streams playing inside the armed
grace window, reconnecting before the deadline retires those streams and stops
the output, a fresh authenticated control session renegotiates, and a new
stream restarts and renders. A companion audit scenario retires the trusted
session on expiry while the consumed pairing record stays retained and is
projected as such, so the revocation path remains reachable.

A deterministic delayed-completion suite now proves the bridge fails closed
when a platform or lifecycle boundary moves between command acceptance and
execution. A first-stream start accepted before a connection loss is rejected
after the loss: the briefly started output and admitted stream are rolled back,
and reconnection returns to ready with no dangling pending command, so the same
start is admitted as a fresh action. A rotation accepted before an output fault
is rejected after the fault and rolls the mixer back to the previous identity,
so output recovery re-admits exactly the identity the coordinator still tracks
and a fresh rotation can be accepted again. A stop commits in the control
domain before the mixer removes the stream, because a removal cannot be rolled
back without the stream's decode pipeline: a stop accepted before an output
fault or a connection loss is rejected with both domains unchanged, the stream
keeps playing inside the reconnect grace window, and recovery re-admits the
same identity the coordinator still tracks.

Output recovery fails closed in both directions. The coordinator acknowledges
the restoration before the mixer recreates its backend, because recreation
cannot be rolled back: when the lifecycle retires (for example the control
session expires) while the output is still down, a later `recover_output` is
rejected with both domains unchanged and the mixer output still retired. When
the backend cannot be recreated after the coordinator committed, the
coordinator rolls back to `OutputUnavailable` so both domains keep agreeing
the output is still down and the retried recovery can succeed once the backend
is available again.

A fault during reconnect grace is represented honestly: the state machine
cannot move to `OutputUnavailable` from `Reconnecting`, so `monitor` surfaces
a control error, but the mixer already stopped the output and the bridge marks
its own output down, so the snapshot never claims an active output. Reconnecting
afterwards resolves to ready, retires the stream, and keeps the output down.
A reconnect-grace clock regression is force-expiry, not a transient error: the
coordinator retires the session and its streams on a time regression, so the
bridge retires the mixer with it. The snapshot therefore never claims a live
stream or an active output under an idle lifecycle.

A separate deterministic suite feeds the ADR 0016 adaptive jitter target
from virtual-link arrival jitter and the encoded-frame water level for one
stream and for two to four streams over one shared link, and a target-gated
scheduler scenario makes that target the playout decision: the scheduler
waits while fewer than the target frames are buffered, drains without
gating once no frames are in flight, and forces a playout when the gate
deadline expires, so the Phase 2 scheduler loop has deterministic,
bounded-latency evidence without a physical latency claim; property
walks, fuzz targets, and the CI smoke gate all assert the scenario's
completion stays within the explicit hard deadline
(`frames * (max_gate_ticks + 2) * frame_duration_ms` plus the
worst-case one-way network delay).

A deterministic fixed-slot mixer-membership scenario
(`mixer_membership_scenario.rs`) now drives the real `CommandDrivenMixer`
boundary from a multi-stream shared-link run: every stream's playout is
decoded to a finite PCM block and pushed into its mixer slot, the callback
renders the bus every tick, and a mid-session remove/reinsert exercises
membership churn over the shared link. The report carries shared-link
counters, per-stream jitter/PLC counters, per-slot underflow, refill-limit,
and discontinuity counters, and the published membership transitions, so a
slot starved by shared-link loss is shown to go silent without blocking the
other slots while the bus stays bounded and finite.

A deterministic controller-driven membership scenario
(`mixer_controller_membership_scenario.rs`) now ties the app-side admission
boundary to that same mixer over one shared link: every stream is admitted
through the real `MixerMembershipController`, playout pushes finite PCM into
the admitted slot queues, the callback renders the bus every tick, and a
mid-session remove/re-admit exercises churn. After every render the report
checks that the controller projection matches the mixer's actual slot table
exactly (active count, per-slot membership, and pending commands), so the
cross-domain invariant is shown to hold under shared-link loss and churn
while a starved slot never blocks the other slots. Bounded per-source gain
and mute scripts are applied through each admitted slot's real
`GainControlHandle`, which the scenario retains and clones into every
admission: a muted stream keeps consuming frames and the report asserts the
bus is exactly silent once every muted slot has rendered its bounded gain
ramp — the ramp advances only as the slot renders queued content, so a
starved slot may resume it without breaking the invariant — a gain event
publishes its target without stalling membership, and a target published
before the churn survives the remove/re-admit. The report also carries
bounded per-source quality ratios (PLC, late, reorder, duplicate,
PCM-underflow, and discontinuity in milli-units) derived exactly from the
raw counters, and unit and property coverage assert every ratio stays
within its bound while a clean link reports zero concealment. The scenario
is exercised at both ends of the Phase 2 stream range (two and four
independent sources) with the same churn profile, so the cross-domain
invariant keeps every admitted slot active and the bus finite.

A deterministic combined jitter-scheduler and mixer scenario
(`mixer_jitter_scheduler_scenario.rs`) ties ADR 0016 to that same fixed-slot
mixer over one shared link: each stream's adaptive target gates playout, the
gate deadline forces a playout once starvation crosses the configured
`max_gate_ticks`, and every played-out frame is decoded to a finite PCM block
and pushed into that stream's slot while the callback renders the bus every
tick. The report carries per-stream target and scheduler counters (gated
ticks, forced playouts) and per-slot underflow, refill-limit, and
discontinuity counters, so shared-link loss is shown to exercise both the
adaptive target and the gate deadline while a gated or starved slot never
blocks the other slots and the bus stays bounded and finite. Bounded
per-source gain and mute scripts are applied through each slot's real
`GainControlHandle`: a muted stream keeps consuming frames and the report
asserts the bus is exactly silent once every muted slot has rendered its
bounded gain ramp — the ramp advances only as the slot renders queued content,
so a starved slot may resume it without breaking the invariant — and a gain
event publishes its target without stalling playout, so per-source gain
control is exercised through the same lossy receiver chain. The report also
carries bounded per-source quality ratios (PLC, late, reorder, duplicate,
forced-playout, PCM-underflow, and discontinuity in milli-units) derived
exactly from the raw counters, and unit, property, and fuzz coverage assert
every ratio stays within its bound while a clean link reports zero
concealment and forced playouts. The scenario
is exercised at both ends of the Phase 2 stream range (two and four
independent sources) with a shared burst that starves every stream past the
gate deadline, so the four-stream upper bound keeps every slot active and
the bus finite. Completion is pinned to the same explicit hard deadline
by the property walks, fuzz targets, and CI smoke gate: reported
`excess_latency_ms` stays within `frames * (max_gate_ticks + 2) *
frame_duration_ms` plus the worst-case one-way network delay, so the
bounded-latency claim is enforced at the CI gate.

This is evidence for deterministic core behavior and host loopback composition
only. It models a bounded sender-side socket buffer as backpressure, but not
real OS socket-buffer behavior, TLS/socket reconnection, real ASRC/device
clocks, operating-system scheduling, NAT rebinding, or physical
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
- Windows shared-mode sample rates and USB/Bluetooth output devices, including
  saturation tests for cumulative callback/render, capacity-fault, and
  stream-error status counters;
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
