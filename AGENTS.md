# SoniMesh Agent Guide

This file contains project-specific guidance for coding agents and contributors.
It complements the repository-wide instructions supplied by the workspace.

## Project status

SoniMesh is a feasibility project with a buildable foundation and platform
prototypes, not a completed end-to-end product. See `docs/mvp-baseline.md` for
implemented boundaries and remaining gates. Do not turn an unverified platform
assumption into a feature promise or a compatibility claim.

The authoritative design documents are:

- `docs/architecture.md` — product boundary and system architecture
- `docs/security-model.md` — pairing, identity, session keys, and packet security
- `docs/testing-and-measurement.md` — latency, quality, soak, fuzz, and HIL tests
- `docs/roadmap.md` — evidence-driven delivery phases and exit criteria
- `docs/mvp-baseline.md` — implementation snapshot, limits, and open gates
- `docs/adr/README.md` — recorded decisions and their evidence gates
- `docs/README.md` — documentation map and evidence conventions

The root `README.md` is the public orientation document. Keep it concise and
link to detailed documents instead of duplicating the full architecture there.

## Repository layout

Use the existing layout and its intended responsibility boundaries:

```text
apps/android/                 Kotlin sender and foreground service
apps/desktop-ui/              Tauri 2 + Vue 3 + TypeScript control UI
crates/sonimesh-app/          Desktop lifecycle and Tauri commands
crates/sonimesh-audio/        Decode, jitter, ASRC, mixing, and DSP
crates/sonimesh-network/      Discovery, control sessions, and UDP transport
crates/sonimesh-protocol/     Wire schemas, packet headers, and test vectors
crates/sonimesh-platform/     Audio output and platform adapters
crates/sonimesh-diagnostics/  Metrics and redacted diagnostic export
proto/                         Protobuf schemas
tools/                         Network simulation and latency measurement
docs/                          Architecture, security, tests, roadmap, and ADRs
```

Shared Rust crates must not depend on Tauri, a web view, or UI framework code.
Platform APIs belong behind explicit adapters.

## Architecture boundaries

- The control plane handles discovery, pairing, negotiation, lifecycle, and UI
  commands. The data plane carries encrypted Opus packets over UDP.
- The UI never carries PCM or Opus frames and is never part of the realtime
  scheduling path.
- The output callback must not allocate, lock, await, decode, access the
  network, write logs, enumerate devices, or call Tauri IPC.
- Every queue, stream count, packet, message, and buffer must have a fixed upper
  bound and an explicit overflow policy.
- A failure in one stream must not block or reset other streams.
- Opus PLC belongs in the per-stream decode path; an output callback may only
  produce bounded silence or fade behavior on a PCM underflow.
- The Android service owns MediaProjection, AudioRecord, encoding, packet
  protection, socket I/O, and reconnect behavior. UI code only presents state and
  sends low-rate commands.
- The Windows MVP must validate CPAL against the actual WASAPI behavior before a
  latency claim is made. Device changes and stream recreation stay outside the
  realtime callback.

## Security rules

- Discovery is not trust. Pairing must authenticate the peer and bind the
  displayed QR/code or SAS to the handshake transcript.
- Do not implement a one-sided, relayable six-digit code as the only first-pair
  authentication mechanism.
- Session and per-stream keys must be derived with an explicit, reviewed HKDF
  scheme. Nonces must be unique across stream, direction, epoch, and sequence.
- Replay-window state advances only after successful AEAD authentication.
- Unauthenticated or malformed payloads must never reach Opus.
- Treat an authenticated paired device as potentially compromised; fuzz and
  bound the decoder input anyway.
- Never log keys, plaintext audio, or unnecessary stable device identifiers.
- Store long-term identities in Android Keystore and an OS-protected Windows
  credential store when those platforms are implemented.

## Platform and product truthfulness

- Android playback capture is policy-limited. If the platform only exposes
  silence, report “no capturable audio detected” and list possible causes rather
  than claiming to know the source application's reason.
- Android 14+ MediaProjection and foreground-service behavior must be tested as a
  lifecycle, including `onStop()` and single-session consent. Android 17 local
  network permission and `NsdManager` discovery behavior require explicit tests.
- Windows `AudioPlaybackConnection` is not an application PCM input API. Do not
  use it as evidence that Bluetooth can feed the SoniMesh mixer.
- Windows 11 is the active desktop baseline. Windows 10 is best effort only.
- Bluetooth is a separate research track and must report `ApplicationPcmSource`,
  `SystemRouted`, or `Unsupported` rather than claiming a unified capability.

## Documentation workflow

When changing behavior or an architectural assumption:

1. Read the relevant design document and existing ADR index first.
2. Update the document that defines the boundary before changing implementation.
3. Add or update an ADR when the change affects protocol, security, timing,
   platform support, or a public compatibility promise.
4. Keep verified facts, measured results, and open hypotheses explicitly labeled.
5. Update the roadmap only when phase scope or exit criteria actually changes.

Avoid duplicating long protocol or metric definitions across README and design
documents. Link to the source document instead.

## Testing expectations

Before implementation is considered complete:

- run formatting, lint, type checks, unit tests, and fuzz/property tests for the
  affected language or crate;
- test packet parsing, replay handling, nonce uniqueness, and Protobuf limits;
- use a deterministic virtual network and clock for jitter, loss, reorder, and
  drift tests;
- separate CI claims from hardware-in-the-loop audio claims;
- report Android model/build, Windows version, AP, output device, sample rate,
  frame size, bitrate, and jitter mode for latency results;
- run the smallest relevant check first, then broaden validation for shared
  protocol or realtime changes.

Use `docs/development.md` for the existing checks and distinguish software
results from hardware evidence. Do not invent a passing result; report
unavailable checks clearly.

## Local development notes

Machine-local notes may be available at `.dev/INDEX.md`. Read them as optional
context, not as a repository contract; tracked design documents and ADRs take
precedence. Keep third-party reference checkouts under `.dev/refs/` for reading
only, check their licenses, and independently implement any ideas adopted.

## Change discipline

- Keep changes focused and preserve unrelated user work.
- Prefer existing abstractions and the documented repository layout.
- Do not add dependencies before an ADR or a concrete implementation need
  justifies them.
- Do not add Bluetooth, multi-source mixing, Linux/macOS support, or an automatic
  updater to the WLAN MVP without updating the roadmap and exit criteria.
- Do not weaken validation, authentication, permissions, memory bounds, or
  redaction to make a prototype appear to work.

## Final handoff

Before summarizing work, inspect the changed files and report:

- meaningful behavior or documentation changes;
- checks that were run and their result;
- checks that could not be run because implementation, hardware, credentials,
  or services are not available;
- any remaining assumption that still requires a prototype or ADR.

