# Opus build and experiment matrix

> Status: pre-binding gate and proposed experiment plan; no Opus binding is enabled and no codec-quality result is recorded.
> Updated: 2026-07-19

This document turns the acceptance conditions in ADR 0009 into reproducible evidence. A passing Rust-only decoder mock, a dependency resolution, or a successful Windows build alone is insufficient to select a codec binding or media policy.

## Current reference-host preflight

| Check | Observation | Interpretation |
| --- | --- | --- |
| Conventional CMake locations | No executable found in Program Files, Program Files (x86), or the per-user Programs directory | Not ready to claim a bundled Windows libopus build path. PATH was not executed by the command host, so this is not proof that CMake is absent everywhere. |
| Reviewed installed libopus | None recorded | Do not use an ambient library path. |
| Android SDK NDK directory | No NDK directory observed under the local Android SDK | Android native ABI work is not ready. |
| Rust decode boundary | sonimesh-audio FrameDecoder exists with caller-owned Float32 output and PLC/reset operations | Stable boundary is ready; no native implementation is selected. |

## Native build gate

All rows must be recorded before enabling any optional codec feature.

| Target | Required evidence | Result |
| --- | --- | --- |
| Windows x86_64 MSVC | Pinned Rust crate/version, libopus source/version, CMake invocation, MSVC toolset, linkage decision, artifact hash, and license texts | Not run |
| Android arm64-v8a | NDK version, ABI build command, packaged native library inspection, and a device load test | Blocked: NDK not present |
| Android x86_64 | Same evidence as arm64-v8a for emulator/CI coverage | Blocked: NDK not present |
| Rust CI | Fresh Windows build plus workspace Clippy/tests with the codec feature enabled | Not run |
| Cross-platform vectors | Identical 10 ms and 20 ms decode/PLC vectors accepted by Rust and Android implementations | Not run |

A build artifact must not be accepted through an unreviewed system library search path. Record the exact dependency and license evidence before any app or Android packaging integration.

## Fixed decode boundary

| Property | Required value |
| --- | --- |
| Decoder owner | One stream-local FrameDecoder implementation outside the output callback |
| Output storage | Existing caller-owned Float32 PCM block only |
| Internal sample rate | 48,000 Hz |
| Channels for first sender path | Stereo, matching the Android capture bootstrap; mono may be an explicit later test cell |
| Maximum authenticated Opus plaintext | 1,150 bytes, derived from the existing version-one media datagram bound |
| Oversized PCM result | Rejected by StreamDecodeWorker; never queued to the realtime callback |
| Decoder fault scope | One stream only; no session-wide reset from the callback |
| Codec-neutral shape gate | Implemented as `OpusFrameConfig`; validates 48 kHz stereo 10/20 ms and caller-owned 960/1,920 sample capacity before any binding |

## Proposed frame and resilience matrix

The following cells are proposed experiments, not product defaults. FEC and DTX start disabled in every cell; a future change requires evidence tied to content class, loss pattern, CPU, and power.

| Cell | Frame duration | Stereo PCM block | Proposed bitrate | PLC | In-band FEC | DTX | Required comparison |
| --- | ---: | ---: | ---: | --- | --- | --- | --- |
| A | 10 ms | 960 interleaved samples | 64 kb/s | Enabled on confirmed loss | Disabled | Disabled | Stable WLAN, 0% loss |
| B | 20 ms | 1,920 interleaved samples | 64 kb/s | Enabled on confirmed loss | Disabled | Disabled | Stable WLAN, 0% loss |
| C | 10 ms | 960 interleaved samples | 96 kb/s | Enabled on confirmed loss | Disabled | Disabled | Independent 1% and 3% loss |
| D | 20 ms | 1,920 interleaved samples | 96 kb/s | Enabled on confirmed loss | Disabled | Disabled | Independent 1% and 3% loss |
| E | 10 ms | 960 interleaved samples | 96 kb/s | Enabled on confirmed loss | Disabled | Disabled | Deterministic burst/reorder profile |
| F | 20 ms | 1,920 interleaved samples | 96 kb/s | Enabled on confirmed loss | Disabled | Disabled | Deterministic burst/reorder profile |

The bitrate values are test parameters only. No cell selects a release bitrate, FEC policy, DTX policy, or latency target.

## Required decoder and simulator checks

For every implemented cell, retain these results with the existing deterministic network simulator:

- valid frame decode into exactly bounded PCM output;
- PLC after a confirmed gap, with no callback-thread decode;
- malformed but authenticated payload rejection without process crash;
- reset after discontinuity and stream-local decoder error isolation;
- output-length overflow rejection by the decode worker;
- loss, reorder, bandwidth, and drift profiles with reproducible seed values;
- 10 ms and 20 ms scheduler/sample timestamps derived from the same OpusFrameConfig used by the future decode worker;
- decode CPU, allocation profile, and Android power/thermal observations; and
- real-device subjective quality notes separated from physical latency results.

## Entry condition for the first binding commit

A first optional binding commit may contain only the pinned dependency, a platform-or-codec-boundary implementation, fixed-capacity decode tests, and the native build evidence it relies on. It must not add encoder work, Android capture transport, keys, pairing, FEC enablement, DTX enablement, or output-callback codec calls.
