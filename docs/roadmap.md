# SoniMesh Roadmap

The roadmap is evidence-driven. A later phase cannot turn an unverified platform
assumption into a product promise.

## Phase 0A — Android capture feasibility

Deliver:

- a capture-only Android prototype;
- a device and OEM compatibility matrix;
- tests for lock screen, background execution, MediaProjection revocation, and
  silent/protected sources;
- measured PCM formats, buffer sizes, and power behavior.

Exit when the supported source classes and unsupported cases are explicit.

## Phase 0B — Minimal Android to Windows audio path

Deliver:

- one Android sender and one Windows receiver;
- a fixed lab address or QR bootstrap;
- one Opus stream;
- a minimal output path and software timing instrumentation;
- no public distribution and no unreviewed custom security protocol.

Exit when continuous audio and a reproducible physical latency measurement exist.

## Phase 0C — Stability, loss, and drift

Deliver:

- 10 ms versus 20 ms comparison;
- bounded jitter buffer and PLC evaluation;
- FEC evidence by content class;
- deterministic loss/reorder simulation;
- 8-hour soak and clock-drift report;
- output-device removal and sleep/wake recovery.

Exit when the latency budget and long-running recovery behavior are accepted.

## Phase 0D — Security protocol

Deliver:

- reviewed first-pairing flow;
- identity storage and revocation;
- TLS/session binding and UDP key derivation;
- replay, nonce, key-epoch, and address-migration tests;
- cross-language crypto and packet test vectors.

Exit before enabling a build outside a controlled lab network.

## Phase 1 — Windows WLAN MVP

Scope:

- Android 12+ actively validated;
- Windows 11 actively validated;
- one source;
- authenticated pairing;
- NsdManager/mDNS discovery plus QR/manual fallback;
- one output device;
- balanced latency mode;
- mute, gain, reconnection, and basic diagnostics;
- signed installer and firewall guidance.

Explicitly excluded: Bluetooth receiver mode, multi-source mixing, Linux/macOS
support, and an automatic updater if signing infrastructure is not ready.

## Phase 2 — Multi-source and hardening

- two to four independent sources;
- fixed-slot realtime mixer;
- per-source gain and quality metrics;
- adaptive jitter target;
- complete drift controller;
- output-device switching;
- diagnostic export;
- 24-hour soak and fault-injection suite.

## Phase 3 — Linux and macOS receivers

- platform output adapters;
- Linux PipeWire primary path;
- macOS WLAN receiver;
- platform packaging, signing, permissions, and support matrices.

## Phase 4 — Bluetooth research track

Only enter this phase after platform prototypes demonstrate a supported PCM
source. Each platform reports ApplicationPcmSource, SystemRouted, or Unsupported;
no cross-platform Bluetooth guarantee is implied.

