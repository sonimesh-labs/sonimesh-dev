# SoniMesh

SoniMesh is a local-first audio bridge that sends Android playback audio to a
desktop computer over the local network. The desktop mixes the received stream
into the selected output device, allowing one headset or speaker to be shared
between a PC and an Android device without uploading audio to a cloud service.

The project is currently in the feasibility and architecture-validation stage.
The documents in this repository are implementation guidance, not a claim that
every platform capability has already been proven.

## Product boundary

The first supported product path is:

- Android 12+ as the actively validated sender baseline
- Windows 11 as the actively validated desktop baseline
- one Android source per desktop session
- one local WLAN, with manual-address fallback during early validation
- Opus audio over an authenticated and encrypted UDP data plane
- a Tauri desktop UI that controls sessions but never carries PCM or Opus data

Android 10 and 11 remain technical compatibility targets because
`AudioPlaybackCapture` starts at API 29, but they are best-effort until the
compatibility matrix has evidence. Windows 10 is not the primary support target;
ordinary Windows 10 Home/Pro reached end of support on 2025-10-14.

SoniMesh does not attempt to bypass Android capture policy, DRM, protected media,
phone calls, VoIP calls, or application-level capture restrictions. Bluetooth
receiver support is a separate research track and is not part of the WLAN MVP.

## Repository layout

```text
apps/
  android/                 Android sender application and foreground service
  desktop-ui/              Tauri 2 + Vue 3 + TypeScript control surface
crates/
  sonimesh-app/            Desktop lifecycle and Tauri commands
  sonimesh-audio/          Decode, jitter, resampling, mixing and DSP
  sonimesh-network/        Discovery, control sessions and UDP transport
  sonimesh-protocol/       Wire schemas, packet headers and test vectors
  sonimesh-platform/       Audio output and platform adapters
  sonimesh-diagnostics/    Metrics, structured events and redaction
proto/                     Control-plane Protocol Buffer schemas
tools/
  latency-measurement/     Deterministic MLS signal generation and offset estimation
  network-simulator/       Deterministic network impairment runner
  opus-codec-prototype/    Native Opus binding and frame-duration evidence spike
  windows-*-prototype/     Windows credential, TLS, and WASAPI feasibility spikes
  check_*.py               Consistency guards for docs, CI, and architecture boundaries
fuzz/
  fuzz_targets/            LibFuzzer targets for bounded protocol and audio parsing
docs/
  architecture.md          Product and system architecture
  security-model.md        Pairing, identity, key derivation and packet security
  testing-and-measurement.md  Latency, audio quality and failure testing
  roadmap.md               Evidence-driven delivery phases
  adr/README.md            Architecture decisions required before implementation
```

## Current status

The repository contains a buildable Rust foundation for
bounded protocol parsing, ChaCha20-Poly1305 media protection, authenticated
media ingress, jitter/PCM buffers, diagnostics, platform adapters, and desktop
session lifecycle. A separately buildable Vue/Vite receiver control preview
exercises only in-memory, low-rate state through its bridge boundary; it is not
a Tauri or platform-audio integration. The project is still a feasibility
project: a capture-only Android prototype and isolated codec, TLS, and Windows
output experiments exist, but an integrated sender, qualified output backend,
end-to-end pairing and measured physical latency remain open.

See [MVP implementation baseline](./docs/mvp-baseline.md) for implemented limits,
explicit gaps, and the next platform-validation slices.

## Documentation

- [Documentation map](./docs/README.md) — find design, implementation status,
  development checks, decisions, protocol vectors, and measurement evidence.
- [Development](./docs/development.md) — prerequisites and checks for contributors.

## Development principle

SoniMesh should only make a platform or latency promise after a reproducible
test produces evidence. When the platform exposes only an observable symptom,
the UI must describe the symptom and possible causes instead of inventing a
more precise diagnosis.
