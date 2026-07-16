# ADR 0009: Opus decoder binding and frame policy

- Status: Provisional pending native build and quality evidence
- Date: 2026-07-17

## Context

The Rust core now provides a `FrameDecoder` boundary that schedules ordinary
decode and PLC outside the output callback. Selecting a native Opus binding
without proving Windows and Android builds would turn an API choice into an
unsupported platform claim. The initial data-plane limits support bounded Opus
payloads, but they do not select an encoding bitrate, frame duration, FEC policy,
or library build path.

`opus` 0.3.1 is the current desktop candidate because it provides safe Rust
bindings to libopus through `audiopus_sys`. On Windows, its transitive build path
builds bundled libopus through CMake unless a reviewed installed library path is
supplied. The current reference host has neither CMake nor a verified libopus
installation, so no dependency is added yet.

## Decision

Keep `sonimesh-audio::FrameDecoder` as the stable boundary. A future optional
`opus-codec` feature may add a `FrameDecoder` implementation only after:

- Windows static/native build is reproducible in CI and on the reference host;
- Android native packaging and ABI coverage are proven separately;
- the binding decodes into caller-owned Float32 PCM without allocation in the
  per-stream hot path;
- malformed authenticated payloads are bounded, surfaced as stream-local errors,
  and never crash the process;
- 10 ms and 20 ms frame duration comparisons produce documented quality, CPU,
  power, and loss/PLC evidence.

The initial implementation will use one Opus stream, 48 kHz internal decoding,
and only negotiated 10 ms or 20 ms packet durations. PLC invokes the decoder's
native missing-frame path from the per-stream worker, never from the output
callback. FEC remains disabled until an encoder configuration and measured
content-class benefit justify it. DTX remains disabled for the initial media
experiments unless measured power behavior justifies its transition effects.

No public API may expose a codec-owned PCM allocation to the realtime callback.
The decoder writes to the existing fixed block supplied by `FrameDecoder`; a
length beyond that block is rejected by `StreamDecodeWorker`.

## Reliability and security implications

Native code is isolated behind the decoder trait and one stream-local adapter.
A decoder error resets or stops only that stream. Authenticated payloads remain
untrusted because a paired peer can be compromised. Decoder version, build flags,
frame duration, bitrate, FEC/DTX mode, and sample rate belong in test reports.

## Validation evidence

Before enabling the feature, publish reproducible native build instructions and
CI results for Windows; add Android ABI build evidence; test valid decode, PLC,
malformed authenticated payloads, oversized output reporting, reset behavior,
and 10/20 ms vectors. Compare loss/PLC and CPU on representative media content.
Physical latency and Android capture claims remain separate tests.

## Rollback plan

The optional feature remains behind `FrameDecoder`. It can be disabled or
replaced with another reviewed libopus binding without changing protocol, network,
realtime queue, or UI contracts.
