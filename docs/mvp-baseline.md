# MVP implementation baseline

> Status: foundation implemented; platform feasibility remains open
> Updated: 2026-07-17

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
- a preallocated PCM block ring that drops the oldest block on overflow and
  emits initialized silence on underflow;
- a lock-free cross-thread PCM queue with non-cloneable logical endpoints,
  oldest-block replacement, and exact discontinuity detection;
- a bounded index-based Float32 channel mapper and preallocated realtime source
  adapter for mono/stereo-to-device layout conversion without callback allocation;
- a bounded linear nominal sample-rate converter and preallocated source adapter
  for rate mismatch without callback allocation; drift control remains separate;
- a control-domain atomic ppm handle for bounded drift adjustments; hard
  resynchronization remains owned by the stream lifecycle controller;
- a bounded queue-backed realtime render source that preserves partial PCM
  blocks, initializes callback underflow to silence, and caps refill attempts;
- allocation-free gain smoothing, atomic diagnostic counters, endpoint
  redaction, output adapter contracts, and a tested desktop session state machine;
- control-domain output health monitoring and stream recreation with a fresh
  render source; platform operations never mutate session state directly, which
  remains owned by the control-session coordinator;
- deterministic virtual-clock/network profiles for random and burst loss,
  jitter, duplication, reorder delay, capacity pressure, and Windows/Linux CI;
- a cross-crate protected pipeline covering AEAD parse/authentication, replay
  rejection, jitter/PLC decisions, non-realtime decode scheduling, and realtime
  PCM transfer without claiming an Opus backend or physical audio result.
- a generated, length-prefixed Protobuf control envelope with hard frame and
  field limits for controlled-lab command exchange.
- bounded version-one hello negotiation that selects the exact supported
  protocol version and a canonical fixed intersection of known capabilities;
  receiver start commands require this one-time negotiation and reject
  unnegotiated frame durations before media work; control-session expiry retires
  the selected capabilities and stream lifecycle state together; tokenized
  start/rotation/stop acknowledgements reject stale asynchronous completions;
  reconnection with fresh session material requires a new hello.
- a per-stream, non-realtime decode-worker boundary that schedules ordinary
  decode and PLC from authenticated jitter decisions, then writes bounded PCM
  blocks to the existing lock-free output queue; no concrete Opus binding yet.
- an application-domain receiver pipeline that composes protected ingress,
  bounded decode admission, and stream-local epoch reset without owning TLS,
  platform output, or a concrete decoder implementation.
- atomic receiver-pipeline diagnostics for authenticated/rejected/replayed/late
  packets, decoder-side PLC, bounded PCM replacement, jitter resynchronization,
  successful epoch resets, and active-epoch PCM queue level/high water;
  output-callback underflow remains owned by the future output adapter.
- a bounded per-stream PCM occupancy controller that emits slew-limited ppm
  decisions or stream-local hard-resynchronization requests; hard resync clears
  only that stream's jitter, decoder state, and queued PCM epoch; no
  quality-qualified drift ASRC is selected or performance-claimed yet.
- an app-domain drift coordinator that samples bounded PCM occupancy, publishes
  normal ppm changes to the source handle, and performs the matching
  stream-local hard resynchronization without touching ingress or output state.
- a pairing-invitation control owner that synchronizes bounded record expiry,
  monotonic-clock failure, user expiry, and externally verified proof outcomes
  with the desktop session state; only a consumed proof can establish trust. It
  does not implement TLS, QR payloads, pairing secrets, or persistent identity
  storage.
- ChaCha20-Poly1305 packet protection with HKDF-SHA256 per-stream keys, a
  reviewed nonce layout, sender sequence exhaustion, and authentication before
  replay admission; pairing/TLS root-secret creation remains unimplemented.
- a non-blocking fixed-buffer UDP media receiver that rejects datagrams above
  the protocol limit before parser ingress; authentication and address policy
  remain separate boundaries.
- an application-domain UDP receiver pipeline that feeds one socket datagram
  through protected ingress, bounded jitter admission, and non-realtime decode;
  control-session and output lifecycle wiring remain separate.
- a bounded incremental TCP control-frame reader/writer that validates the
  existing length-prefixed Protobuf envelope; it is explicitly unauthenticated
  and is not a TLS or pairing implementation.
- a capture-only Kotlin/Compose Android phase 0A bootstrap with direct,
  non-persisted MediaProjection consent transfer, a media-projection foreground
  service, a fixed PCM16 AudioRecord read buffer, low-rate state publication,
  bounded evidence counters, and one idempotent teardown path; this is build
  evidence only and has no physical-device capture result yet.
- a buildable Vue/Vite receiver-control preview with a single low-rate bridge
  boundary, session controls, output selection, pairing-invitation state, and
  diagnostics presentation; it uses in-memory preview data and has no Tauri,
  audio, key, or realtime integration.

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
| Encoded frame | Compile-time capacity per stream | Reject oversized frame copy |
| Local PCM ring | Compile-time blocks and samples | Drop oldest block; output silence when empty |
| Cross-thread PCM queue | 1–128 inline blocks | Atomically replace oldest; report sequence gap |
| Realtime render refills | 128 block pops per callback | Stop refilling and emit silence after the bound |
| Output sample rate | 8–192 kHz | Reject outside platform-neutral bound |
| Output channels | 1–8 | Reject zero or excessive layouts |
| Requested callback | default or 16–8192 frames | Fixed request may be unsupported |
| Maximum callback | 16–8192 frames | Oversized callback emits silence and a fault |
| Active source count | One for MVP | Separate ingress and buffers per future source |

Compile-time capacities make accidental unbounded growth impossible in these
containers. Concrete frame count and block size must be selected by the frame,
jitter, and output-backend ADRs using measured 10/20 ms and device results.

## Deliberately not implemented

- Android capture hardware/OEM evidence, lifecycle matrix results, and power
  measurements;
- Opus encoder/decoder bindings and PLC/FEC evidence;
- first pairing, TLS, QR bootstrap, identity storage, revocation, or root-secret
  provisioning; AEAD/HKDF media protection and bounded epoch rotation exist only
  after a caller supplies an authenticated control-session secret;
- authenticated TLS control transport, discovery, and address migration; the
  untrusted TCP framing reader is not a trusted control session.
- direct-WASAPI comparison adapter and backend selection evidence;
- Tauri integration, signing, packaging, and firewall integration;
- a quality-qualified resampler/ASRC, acoustic latency, and soak validation; the
  nominal converter and occupancy controller do not establish timing or quality
  claims.

An optional Windows CPAL adapter and read-only device probe are now available for
phase 0B measurement. Their presence does not close the direct-WASAPI comparison,
device lifecycle, callback timing, or physical latency gates in ADR 0003.

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
