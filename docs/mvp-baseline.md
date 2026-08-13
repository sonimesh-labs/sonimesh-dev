# MVP implementation baseline

> Status: foundation implemented; platform feasibility remains open
> Updated: 2026-08-13

This baseline turns the architecture documents into a buildable core without
claiming that roadmap phases 0A–0D or the Windows WLAN MVP are complete.

## Implemented in the foundation

- one Rust workspace with separate app, audio, diagnostics, network, platform,
  and protocol crates;
- allocation-free version-one media header encoding and bounded packet parsing;
- a 1,200-byte complete-media-datagram limit and a 128-byte header limit;
- exact extended-header preservation for future AEAD associated data;
- a fixed 64-packet replay window advanced only through authenticated ingress;
- per-stream fixed-capacity encoded jitter storage with duplicate, late-frame,
  gap/PLC, controlled-resynchronization, and explicit sequence-exhaustion
  results;
- a preallocated PCM block ring that rejects empty blocks, drops the oldest
  block on overflow, and emits initialized silence on underflow;
- a lock-free cross-thread PCM queue with non-cloneable logical endpoints,
  empty-block rejection,
  oldest-block replacement, and exact discontinuity detection;
- a bounded index-based Float32 channel mapper and preallocated realtime source
  adapter for mono/stereo-to-device layout conversion without callback allocation;
- a bounded linear nominal sample-rate converter and preallocated source adapter
  for rate mismatch without callback allocation; drift control remains separate;
- a control-domain atomic ppm handle for bounded drift adjustments; hard
  resynchronization remains owned by the stream lifecycle controller;
- a bounded queue-backed realtime render source that preserves partial PCM
  blocks, initializes callback underflow to silence, records refill-budget
  exhaustion separately, caps refill attempts, and fades in after queue
  discontinuities;
- a lock-free atomic gain and mute handle shared by a stream's realtime render
  source and the desktop control domain; published targets ramp over a fixed
  128-sample window only when the target changes, apply without allocation or
  locking, and stay bounded to attenuation in the single-stream MVP;
- an app-domain receiver PCM endpoint factory that creates a fresh bounded queue
  pair and format-adapted render source together for output recovery; the
  receiver pipeline can install the paired producer without resetting ingress.
  The factory retains the per-stream gain and mute handle, so a user's choice
  survives output-device recreation without a replay command.
- a receiver output lifecycle controller that starts a matching platform source
  before swapping its paired producer into the receiver pipeline; failed output
  starts leave the existing producer intact for retry. The controller accepts
  either the direct receiver pipeline or its UDP composition wrapper without
  taking ownership of ingress or socket state. Output-format diagnostics publish
  only after the platform accepts the matching callback source and clear when
  that source stops. The controller exposes the per-stream gain and mute handle,
  so a desktop owner can publish a local volume choice that reaches the active
  source and survives output-device recovery. It also projects the same redacted
  low-rate output snapshot as the platform output controller through one shared
  projection trait, and the pairing invitation controller reads the desktop
  control snapshot directly from either output owner, so a bridge cannot
  fabricate output lifecycle state.
- allocation-free gain smoothing, saturating atomic stream and output-status
  counters, endpoint redaction, output adapter contracts, and a tested desktop
  session state machine;
- diagnostic-safe Debug boundaries for media payloads, encoded jitter frames,
  PCM queue blocks, UDP endpoints and network I/O error text, control-envelope
  text, output device metadata, and arbitrary protocol-decoder, decoder, or
  output-backend error details; control-session and operation token
  issuance fails closed instead of wrapping at the `u64` boundary;
- control-domain output health monitoring and stream recreation with a fresh
  render source; platform operations never mutate session state directly, which
  remains owned by the control-session coordinator;
- deterministic virtual-clock/network profiles for random and burst loss,
  jitter, duplication, reorder delay, byte-counted bandwidth serialization,
  capacity pressure, and Windows/Linux CI;
- a cross-crate protected pipeline covering AEAD parse/authentication, replay
  rejection, jitter/PLC decisions, non-realtime decode scheduling, and realtime
  PCM transfer without claiming an Opus backend or physical audio result.
- a generated, length-prefixed Protobuf control envelope with hard frame and
  field limits for controlled-lab command exchange.
- symmetric bounded control framing proven over one real TCP connection: both
  ends read and write length-prefixed envelopes through the same non-blocking
  framing boundary, so round trips, pipelined wire order, and peer-close
  retirement hold in both directions before any authenticated transport is
  added;
- bounded version-one hello negotiation that selects the exact supported
  protocol version and a canonical fixed intersection of known capabilities;
  receiver start and stop commands require this one-time negotiation, and
  starts reject unnegotiated frame durations before media work; each
  authenticated transport has a non-reusable local token rejected after loss or
  retirement;
  control-session expiry retires the token, selected capabilities, and stream
  lifecycle state together; process-unique start/rotation/stop operation tokens
  reject stale or cross-coordinator asynchronous completions; reconnection with
  fresh session material
  requires a newly installed transport token and hello.
- a clock-driven bounded reconnect-grace window that keeps a lost control
  session in the reconnecting lifecycle only until its armed deadline; the
  control-session coordinator expires the token, negotiation, media identity,
  and pending work together when the window elapses or the clock regresses,
  and reconnection with fresh session material ends the window immediately.
- a per-stream, non-realtime decode-worker boundary that schedules ordinary
  decode and PLC from authenticated jitter decisions, rejects empty or
  oversized encoded frames before decoder work, then writes bounded PCM blocks
  to the existing lock-free output queue; no concrete Opus binding yet.
- an application-domain receiver pipeline that composes protected ingress,
  bounded decode admission, and stream-local epoch reset without owning TLS,
  platform output, or a concrete decoder implementation. It can install a
  control-domain authenticated fresh media session without recreating socket or
  output ownership, retiring the prior replay and playout epoch first.
- atomic receiver-pipeline diagnostics for authenticated/rejected/replayed/late
  packets, accepted reordered packets, decoder-side PLC/errors, bounded PCM
  replacement, jitter resynchronization, successful epoch resets, and
  capacity-clamped active-epoch PCM queue level/high water published as one
  coherent atomic pair;
  output-callback underflow remains owned by the future output adapter.
- a bounded per-stream PCM occupancy controller that emits slew-limited ppm
  decisions or stream-local hard-resynchronization requests; hard resync clears
  only that stream's jitter, decoder state, and queued PCM epoch; no
  quality-qualified drift ASRC is selected or performance-claimed yet.
- an app-domain drift coordinator that samples bounded PCM occupancy, publishes
  normal ppm changes to the source handle, and performs the matching
  stream-local hard resynchronization without touching ingress or output state.
- a pairing-invitation control owner that synchronizes bounded record expiry,
  monotonic-clock failure, user expiry, and proof outcomes with the desktop
  session state. It invokes a caller-supplied verifier only after bounded
  per-source admission and charges the attempt first; only a consumed proof can
  establish trust. It does not implement TLS, QR payloads, pairing secrets, or
  persistent identity storage.
- ChaCha20-Poly1305 packet protection with HKDF-SHA256 per-stream keys, a
  reviewed nonce layout, sender sequence exhaustion, authentication before
  replay admission, and matching Rust/Kotlin-JVM public vectors; Android runtime
  provider and pairing/TLS root-secret creation remain unimplemented.
- dependency-free Python verifiers independently recompute the published
  pairing, control-frame, media-header, media-key, and media-AEAD vectors from
  RFC 8439 ChaCha20-Poly1305, HKDF-SHA256, and the documented wire layouts, so
  vector drift is caught without a second compiler; sealed media packets are
  resealed from constants, not replayed through the Rust code.
- a pure TLS-exporter pairing-binding boundary with fixed Rust/Kotlin test
  vectors for role-separated proofs and the one-time media root. It accepts only
  an externally supplied 32-byte exporter result after the future pinned TLS
  1.3 handshake, performs no TLS, QR, pairing-record consumption, network I/O,
  certificate validation, or persistent identity storage, and therefore does
  not establish pairing or root-secret provisioning.
- a non-blocking fixed-buffer UDP media receiver composition that rejects
  oversized datagrams before parser ingress, pins the source only after complete
  protected admission, rejects silent endpoint changes without advancing replay
  state, and permits rebinding only after fresh-session installation. The
  authenticated control-plane reconnection that supplies that fresh session
  remains unimplemented.
- a non-blocking fixed-capacity UDP media sender that rejects oversized
  datagrams before touching the socket, never blocks (a full OS send buffer is
  reported as backpressure for the caller's own drop or pacing policy), and
  treats the destination as routing metadata only.
- a bounded session-level UDP media sender that composes the per-epoch sealer
  and the non-blocking socket for one stream: it seals and sends bounded frames,
  reports a full send buffer as backpressure without blocking, refuses a frame
  that exceeds the plaintext limit before touching the socket, rotates to a
  strictly newer key epoch (fresh sequence at zero, same session/stream/direction),
  installs a fresh session with a new context, and treats the destination as
  routing metadata only.
- an application-domain UDP receiver pipeline that feeds one socket datagram
  through protected ingress, bounded jitter admission, and non-realtime decode;
  control-session and output lifecycle wiring remain separate.
- a bounded adaptive jitter target fed by per-packet relative arrival jitter:
  the controller moves the target water level within configured limits, an
  opt-in receiver-pipeline configuration supplies a monotonic arrival clock and
  feeds live authenticated packets, and the published water levels (one atomic
  set), smoothed jitter, and step counts reach the diagnostic export. The
  product playout scheduler that consumes the target remains Phase 2 work.
- an independent LibFuzzer workspace for bounded control decoding, media packet
  parsing, and protected ingress. The ingress target covers arbitrary bytes,
  valid authenticated arbitrary payloads, replay, output bounds, tampering,
  and plaintext clearing. Configured finite CI executions are smoke evidence
  rather than a sustained campaign.
- a bounded incremental TCP control-frame reader/writer that validates the
  existing length-prefixed Protobuf envelope and retires partial read/write
  state after a framing, semantic, or non-transient socket error, plus
  Rust/Kotlin-JVM host tests for all five published Hello, StartStream,
  StopStream, StreamStatus, and ControlError wire vectors and their semantic
  bounds. The transport is explicitly unauthenticated and is not a TLS or
  pairing implementation.
- a capture-only Kotlin/Compose Android phase 0A bootstrap with direct,
  non-persisted MediaProjection consent transfer, a media-projection foreground
  service, a fixed PCM16 AudioRecord read buffer, low-rate state publication
  that rebases after a nonnegative clock regression, bounded evidence counters
  that saturate at the signed 64-bit maximum, validated local monotonic
  timestamps and read-completion offsets, every non-teardown negative or thrown
  AudioRecord read failure, and fail-closed
  validation of positive read bounds and channel alignment; a
  frame-duration-based no-signal transition stable across partial reads;
  and an atomic idempotent teardown gate that transactionally admits reader
  startup, publishes terminal-outcome readiness, rejects a fresh consent result
  delivered to a retiring service generation with an actionable retry state,
  permits one final cleanup claimant only after reader exit, continues bounded
  retirement after an ordinary cleanup exception, and rethrows a fatal JVM
  failure after the bounded attempts finish, while ordinary failures emit one
  count-only redacted diagnostic; partial Vivo physical-device
  capture and background evidence is recorded in the [Android capture matrix](./measurements/android-capture-matrix.md).
  The remaining lifecycle, power, source-policy, and OEM matrix is still open.
- a buildable Vue/Vite receiver-control preview with a single low-rate bridge
  boundary, session controls, output selection, per-stream volume and mute,
  pairing-invitation state, and diagnostics presentation for packet,
  PCM-queue/refill, and resampling state; it uses in-memory preview data and
  has no Tauri, audio, key, or realtime integration.
- a dependency-free `tools/latency-measurement` crate that generates a
  deterministic MLS reference WAVE, reads WAVE recordings (PCM 16/32, IEEE
  float 32/64, and WAVE_FORMAT_EXTENSIBLE), estimates the correlation offset
  with a bounded search window and distinctness check, and prints per-run
  offsets plus p50/p95/p99 summaries; it is a software measurement aid, not a
  physical latency proof.

## Initial capacities and policies

| Boundary | Current hard bound | Policy |
| --- | ---: | --- |
| Media fixed header | 36 bytes | Reject truncated input |
| Media extended header | 128 bytes | Reject larger declared header |
| Complete media datagram | 1,200 bytes | Reject before authentication/allocation |
| Control Protobuf frame | 64 KiB | Reject before Protobuf decoding |
| Control start stream | 8–192 kHz, 1–8 channels, 10/20 ms | Reject invalid command parameters |
| Protected media plaintext | 1,148 bytes | Reserve 16 bytes for Poly1305 tag |
| Encrypted payload with fixed header | 1,164 bytes | Extensions reduce this available payload |
| Replay window | 64 packets | Reject authenticated duplicate or too-old packet |
| Jitter storage | Compile-time capacity per stream | Stream-local reset on capacity collision |
| Encoded frame | Compile-time capacity per stream | Reject empty or oversized frame copy |
| Local PCM ring | Compile-time blocks and samples | Drop oldest block; output silence when empty |
| Cross-thread PCM queue | 1–128 inline blocks | Atomically replace oldest; report sequence gap |
| Realtime render refills | 128 block pops per callback | Stop refilling, emit silence, and count one limit hit |
| Per-stream gain control | 0.0–1.0 linear, attenuation-only | Reject non-finite or out-of-range; ramp over 128 samples |
| Output sample rate | 8–192 kHz | Reject outside platform-neutral bound |
| Output channels | 1–8 | Reject zero or excessive layouts |
| Requested callback | default or 16–8192 frames | Fixed request may be unsupported |
| Maximum callback | 16–8192 frames | Oversized callback emits silence and a fault |
| Active source count | One for MVP | Separate ingress and buffers per future source |

Compile-time capacities make accidental unbounded growth impossible in these
containers. Concrete frame count and block size must be selected by the frame,
jitter, and output-backend ADRs using measured 10/20 ms and device results.

## Deliberately not implemented

- complete Android capture hardware/OEM evidence, lifecycle matrix results, and
  power measurements; partial Vivo capture and background evidence is recorded
  in the [Android capture matrix](./measurements/android-capture-matrix.md);
- Opus encoder/decoder bindings, PLC/FEC evidence, and authenticated malformed
  payload fuzzing against the eventual native decoder;
- first pairing, TLS, QR bootstrap, identity storage, revocation, or actual
  root-secret provisioning; the pure TLS-exporter vector does not prove a TLS
  provider can pin the QR SPKI or export the required bytes. AEAD/HKDF media
  protection and bounded epoch rotation exist only after a caller supplies an
  authenticated control-session secret;
- authenticated TLS control transport, discovery, and control-plane address
  migration/reconnection. The UDP source pin can be reset by a fresh media
  session, but the untrusted TCP framing reader cannot authorize or supply one;
- Windows output backend selection, physical device/power lifecycle evidence,
  and latency qualification. The independent direct-WASAPI prototype and
  controlled same-host callback comparison exist, but do not settle a backend;
- Tauri integration, signing, packaging, and firewall integration;
- a quality-qualified resampler/ASRC, acoustic latency, and soak validation; the
  nominal converter and occupancy controller do not establish timing or quality
  claims.

An optional Windows CPAL adapter and a separate direct-WASAPI prototype are now
available for phase 0B measurement. Their same-host comparison records actual
callback/render frames and backend-local software timing under matching silent
smoke configuration in the [Windows output backend comparison
record](./measurements/2026-07-20-windows-output-backend-comparison.md). This
does not close the backend-selection, physical device/power lifecycle, or
physical-latency gates in ADR 0003.

A two-second silent CPAL smoke on the Windows reference host observed callback
sizes beyond both the requested and device-reported frame sizes. The evidence and
remaining gaps are documented in
[the Windows CPAL smoke record](./measurements/2026-07-17-windows-cpal-smoke.md).
The second host sample used an eight-channel USB endpoint; semantic speaker
position mapping and physical source-to-device routing remain unqualified.

These are not omissions hidden behind mocks. Each is a named roadmap/ADR gate,
and no build should leave a controlled lab network before phase 0D is reviewed.

## Next implementation slice

1. Run the capture-only phase 0A app across the evidence matrix under
   [ADR 0010](./adr/0010-android-capture-service-boundary.md); the current
   project is only the buildable bootstrap.
2. Measure CPAL and prototype direct WASAPI behind `sonimesh-platform`; record
   callback size, negotiated format, device lifecycle, and latency in ADR 0003.
3. Validate the provisional Opus binding/build policy in ADR 0009 on Windows and
   Android, then compare 10 ms and 20 ms frames without adding latency claims.
4. Implement the reviewed QR-pinned TLS pairing design only after Android
   Keystore and Windows credential-store spikes; do not enable any non-lab
   network build before that validation.
