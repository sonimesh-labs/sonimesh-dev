# Opus build and experiment matrix

> Status: pre-binding gate and proposed experiment plan; no Opus binding is enabled and no codec-quality result is recorded.
> Updated: 2026-07-20

This document turns the acceptance conditions in ADR 0009 into reproducible evidence. A passing Rust-only decoder mock, a dependency resolution, or a successful Windows build alone is insufficient to select a codec binding or media policy.

## Current reference-host preflight

| Check | Observation | Interpretation |
| --- | --- | --- |
| Windows build tools | `cmake`, `ninja`, and `cl.exe` were absent from PATH; CMake's conventional locations and VS 2019/2022 BuildTools/Community MSVC directories were also absent | The reference host cannot build bundled libopus or a C/C++ binding until reviewed build tools are installed and their versions are recorded. |
| Rust Windows host | Rust 1.85.0 `x86_64-pc-windows-msvc` is installed; it is the only installed Rust target | A Windows-native build still also needs the C/C++ toolchain required by libopus. Android cross-targets are not installed. |
| Reviewed installed libopus | No workspace dependency, Cargo cache entry, Gradle cache entry, or reviewed installed library was found | Do not use an ambient library path or add an unreviewed prebuilt binary. |
| Candidate binding metadata | `cargo info opus@0.3.1` reports `opus` 0.3.1, MIT/Apache-2.0, repository `SpaceManiac/opus-rs` | Candidate metadata only; it does not establish a libopus source revision, native build path, or libopus license evidence. |
| Android SDK NDK directory | Android SDK exists at the local SDK root, but contains no `ndk` directory | Android ABI work is blocked until a pinned NDK is installed and its package/path are recorded. |
| Rust decode boundary | sonimesh-audio FrameDecoder exists with caller-owned Float32 output and PLC/reset operations | Stable boundary is ready; no native implementation is selected. |

## Native build gate

All rows must be recorded before enabling any optional codec feature.

| Target | Required evidence | Result |
| --- | --- | --- |
| Windows x86_64 MSVC | Pinned Rust crate/version, libopus source/version, CMake invocation, MSVC toolset, linkage decision, artifact hash, and license texts | Blocked: CMake, Ninja, and MSVC compiler/toolset unavailable on the reference host |
| Android arm64-v8a | NDK version, ABI build command, packaged native library inspection, and a device load test | Blocked: NDK not present |
| Android x86_64 | Same evidence as arm64-v8a for emulator/CI coverage | Blocked: NDK not present |
| Rust CI | Fresh Windows build plus workspace Clippy/tests with the codec feature enabled | Not run |
| Cross-platform vectors | Identical 10 ms and 20 ms decode/PLC vectors accepted by Rust and Android implementations | Not run |

A build artifact must not be accepted through an unreviewed system library search path. Record the exact dependency and license evidence before any app or Android packaging integration.

## Unblock sequence

Before retrying the native binding gate on this host, install or provision the
following reviewed tools and record their exact versions in a new dated build
record:

1. CMake and a supported generator such as Ninja;
2. the MSVC C/C++ build tools compatible with the pinned Windows Rust toolchain;
3. a pinned Android NDK with `arm64-v8a` and `x86_64` ABI toolchains; and
4. the selected libopus source revision, its license text, the binding's
   transitive native-build configuration, and the intended static/dynamic
   linkage decision.

Only then may a dependency-only codec spike attempt a clean Windows build. It
must not use an ambient libopus discovery path or enable Android packaging until
the corresponding ABI evidence exists.

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

The deterministic simulator currently expands C and D into fixed 1% and 3% independent-loss runs, producing eight A-F cases total. Its bitrate field is report metadata only; no encoder is configured.

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

## Codec-neutral media-path regression

The deterministic cross-crate test in
`tools/network-simulator/tests/media_pipeline.rs` now passes authenticated test
datagrams through `ProtectedMediaIngress`, `StreamDecodeWorker`, a mock
`FrameDecoder`, and the bounded realtime PCM queue. It asserts that a missing
sequence produces `DecodeStep::Concealed` in the decode worker, not in the
consumer callback, and that a stalled two-block queue reports
`ReplacedOldest` at the worker producer boundary.

This is codec-neutral control-flow and overflow-policy coverage only. The same
eight A-F profiles also run through a `mock_worker` suite that constructs a
`StreamDecodeWorker` with the selected 960/1,920 sample block, dispatches PLC
in the worker, and lets a paired PCM consumer only copy queued blocks. It does
not load libopus, configure an encoder, evaluate audio quality, measure
CPU/power, or measure device or physical latency.

## Entry condition for the first binding commit

A first optional binding commit may contain only the pinned dependency, a platform-or-codec-boundary implementation, fixed-capacity decode tests, and the native build evidence it relies on. It must not add encoder work, Android capture transport, keys, pairing, FEC enablement, DTX enablement, or output-callback codec calls.
