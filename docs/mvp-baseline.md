# MVP implementation baseline

> Status: foundation implemented; platform feasibility remains open
> Updated: 2026-07-14

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
  gap/PLC, and controlled-resynchronization results;
- a preallocated PCM block ring that drops the oldest block on overflow and
  emits initialized silence on underflow;
- a lock-free cross-thread PCM queue with non-cloneable logical endpoints,
  oldest-block replacement, and exact discontinuity detection;
- allocation-free gain smoothing, atomic diagnostic counters, endpoint
  redaction, output adapter contracts, and a tested desktop session state machine;
- deterministic virtual-clock/network profiles for random and burst loss,
  jitter, duplication, reorder delay, capacity pressure, and Windows/Linux CI;
- a cross-crate lab pipeline covering parse, authenticate, replay rejection,
  jitter/PLC decisions, and realtime PCM transfer without claiming real crypto.

## Initial capacities and policies

| Boundary | Current hard bound | Policy |
| --- | ---: | --- |
| Media fixed header | 36 bytes | Reject truncated input |
| Media extended header | 128 bytes | Reject larger declared header |
| Complete media datagram | 1,200 bytes | Reject before authentication/allocation |
| Encrypted payload with fixed header | 1,164 bytes | Extensions reduce this available payload |
| Replay window | 64 packets | Reject authenticated duplicate or too-old packet |
| Jitter storage | Compile-time capacity per stream | Stream-local reset on capacity collision |
| Encoded frame | Compile-time capacity per stream | Reject oversized frame copy |
| Local PCM ring | Compile-time blocks and samples | Drop oldest block; output silence when empty |
| Cross-thread PCM queue | 1–128 inline blocks | Atomically replace oldest; report sequence gap |
| Active source count | One for MVP | Separate ingress and buffers per future source |

Compile-time capacities make accidental unbounded growth impossible in these
containers. Concrete frame count and block size must be selected by the frame,
jitter, and output-backend ADRs using measured 10/20 ms and device results.

## Deliberately not implemented

- Android project and MediaProjection capture spike;
- Opus encoder/decoder bindings and PLC/FEC evidence;
- AEAD, HKDF, TLS, QR pairing, identity storage, or key rotation;
- UDP sockets, discovery, control Protobuf generation, and address migration;
- CPAL/direct-WASAPI output adapters;
- Tauri/Vue product UI, signing, packaging, and firewall integration;
- drift ASRC, nominal rate conversion, acoustic latency, and soak validation.

These are not omissions hidden behind mocks. Each is a named roadmap/ADR gate,
and no build should leave a controlled lab network before phase 0D is reviewed.

## Next implementation slice

1. Record ADR 0002 for the Android service/JNI boundary, then build the capture-
   only phase 0A app and evidence matrix.
2. Prototype CPAL and direct WASAPI behind `sonimesh-platform`; record callback
   size, negotiated format, device lifecycle, and latency results in ADR 0003.
3. Select a maintained Opus binding only after a concrete phase 0B spike need,
   then compare 10 ms and 20 ms frames without adding network security claims.
4. Specify and review pairing/media security before any non-lab network build.
