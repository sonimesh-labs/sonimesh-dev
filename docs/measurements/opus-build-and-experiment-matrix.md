# Opus build and experiment matrix

> Status: pre-binding gate and proposed experiment plan; no Opus binding is enabled and the gate is not closed.
> Updated: 2026-08-16
>
> On 2026-08-16 the reference host built the candidate binding and ran the
> `tools/opus-codec-prototype` decode/PLC probe (see the dated build record).
> The binding is still not enabled; Android ABI, measured CPU-time
> utilization, allocation-profile and power, sustained coverage-guided
> fuzzing, and CI reproducibility evidence remain pending.

This document turns the acceptance conditions in ADR 0009 into reproducible evidence. A passing Rust-only decoder mock, a dependency resolution, or a successful Windows build alone is insufficient to select a codec binding or media policy.

## Current reference-host preflight

| Check | Observation | Interpretation |
| --- | --- | --- |
| Windows build tools | On 2026-07-20 `cmake`, `ninja`, and `cl.exe` were absent from PATH; on 2026-08-16 Visual Studio 18 Community provided CMake `4.2.3-msvc3` and MSVC `14.50` (CMake not on PATH by default; Ninja availability not recorded) | The reference host can now build bundled libopus with the documented `CMAKE_POLICY_VERSION_MINIMUM=3.5` workaround; see the 2026-08-16 build record. |
| Rust Windows host | Rust 1.85.0 `x86_64-pc-windows-msvc` is installed; it is the only installed Rust target | A Windows-native build still also needs the C/C++ toolchain required by libopus. Android cross-targets are not installed. |
| Reviewed installed libopus | No workspace dependency, Cargo cache entry, Gradle cache entry, or reviewed installed library was found | Do not use an ambient library path or add an unreviewed prebuilt binary. |
| Candidate binding metadata | `cargo info opus@0.3.1` reports `opus` 0.3.1, MIT/Apache-2.0, repository `SpaceManiac/opus-rs` | Candidate metadata only; it does not establish a libopus source revision, native build path, or libopus license evidence. |
| Android SDK NDK directory | Android SDK exists at the local SDK root, but contains no `ndk` directory | Android ABI work is blocked until a pinned NDK is installed and its package/path are recorded. |
| Rust decode boundary | sonimesh-audio FrameDecoder exists with caller-owned Float32 output and PLC/reset operations | Stable boundary is ready; no native implementation is selected. |

## Native build gate

All rows must be recorded before enabling any optional codec feature.

| Target | Required evidence | Result |
| --- | --- | --- |
| Windows x86_64 MSVC | Pinned Rust crate/version, libopus source/version, CMake invocation, MSVC toolset, linkage decision, artifact hash, and license texts | Partially passed on the reference host (2026-08-16): `opus` 0.3.1 / `audiopus_sys` 0.2.2 built with CMake 4.2.3-msvc3 + MSVC 14.50; artifact hash and license texts not yet recorded; CI reproducibility pending |
| Android arm64-v8a | NDK version, ABI build command, packaged native library inspection, and a device load test | Blocked: NDK not present |
| Android x86_64 | Same evidence as arm64-v8a for emulator/CI coverage | Blocked: NDK not present |
| Rust CI | Fresh Windows build plus workspace Clippy/tests with the codec feature enabled | Wired into CI (Windows probe step plus workspace clippy/test compile the member); CI execution not yet observed |
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
- loss, reorder, jitter-wait, bandwidth, and drift profiles with reproducible seed values;
- 10 ms and 20 ms scheduler/sample timestamps derived from the same OpusFrameConfig used by the future decode worker;
- decode CPU, allocation profile, and Android power/thermal observations; and
- real-device subjective quality notes separated from physical latency results.

On 2026-08-16 the reference-host probe (`tools/opus-codec-prototype`) added a
deterministic malformed-payload sweep (fixed-seed, 1,000 payloads up to the
datagram limit), decode/PLC RMS sanity checks, steady-state aligned-SNR checks
against the source tone for both negotiated shapes, a deterministic five-frame
loss burst, a 1-32 frame burst-length sweep (1, 2, 3, 5, 8, 12, 16, 24,
32) that records PLC degradation and confirms the stream re-locks to the
source after every length, deterministic
loss-pattern windows (independent 1/3/5% and a periodic 5-in-50 burst) that
keep the window mean SNR within 35.7-41.3 dB across both shapes, a
deterministic jitter-wait sweep that records the concealment/drop boundary
of waiting before declaring a loss, bounded reorder absorption and
late-drop, steady-state and loss-window quality across two deterministic
content profiles (single tone and multi-harmonic sum), per-frame
encode+decode and decode-only host throughput for both shapes, and a
sustained 120 s encode+decode pass per shape that reports realtime
headroom and a wall-clock-derived single-core utilization estimate
(see the dated build record). That is prototype-level
evidence only, not a coverage-guided fuzz result, an adaptive
jitter-target result, a measured CPU-time utilization trace, a power
measurement, or an allocation-profile result. The Linux CI fuzz job
now also runs a finite
`native_opus_decode` smoke target that drives arbitrary post-authentication
bytes through the bundled libopus decoder behind the fixed-capacity decode
worker; that smoke is a build and regression check, not sustained
coverage-guided fuzzing.

On the same date the probe added a deterministic in-band FEC recovery
comparison by content class: for one dropped 20 ms frame, speech-like content
recovers at 22.3 dB through in-band FEC versus 16.3 dB through PLC (+6.0 dB),
while a single-tone reference shows no benefit (FEC equals PLC at 33.1 dB).
The comparison is codec-level evidence only; FEC stays disabled in every cell
and the 10 ms shape cannot carry LBRR redundancy (see the
[in-band FEC content-class record](../measurements/2026-08-16-windows-opus-inband-fec.md)).

On the same date the probe extended that FEC comparison with a speech-like
loss-rate sweep and a wall-clock FEC CPU pass. At 5% announced loss the
encoder stays in CELT-only mode and codes no LBRR, so FEC equals PLC exactly
(18.6 dB, no gain); at 20% it gains +4.3 dB over PLC (21.5 dB versus
17.2 dB). Enabling FEC raises direct codec-level encode+decode wall time by
about +214-232% at the auto bitrate (hybrid mode) and +108-116% at 24 kb/s
(SILK mode), so the cost is mode-dependent rather than a flat FEC tax. The
sweep and CPU numbers are codec-level, wall-clock evidence; they do not
select an FEC policy, and FEC stays disabled in every cell (see the
in-band FEC content-class record above).

On the same date the probe added a deterministic DTX (discontinuous
transmission) comparison by content class: on pause-gapped speech-like
content, DTX turns 84 of 240 packets into one-byte DTX packets and
reduces the average packet size from 249 to 163 bytes (35% smaller)
with no measurable active-region SNR change (14.1 dB), while a
continuous tone never goes inactive and stays unchanged (0 DTX packets,
45.1 dB). The same DTX cells now also run at 10 ms: the average packet
size drops from 129 to 84 bytes (also 35%) with the same 84/240
DTX-packet count and no active-region SNR change (15.4 dB), and the
tone control still emits no DTX packets (41.2 dB). The comparison is
codec-level evidence only; DTX stays disabled in every cell and is not a
power measurement (see the
[DTX content-class record](../measurements/2026-08-16-windows-opus-dtx.md)).

On the same date the probe added a deterministic 10 ms vs 20 ms
frame-duration comparison at equal per-second codec bitrates (24 kb/s and
96 kb/s) across the tone and multi-harmonic profiles: the 20 ms shape
keeps a steady-state SNR lead (largest at 24 kb/s, +11.4 dB on tone and
+4.7 dB on multi-harmonic), and the 10 ms shape pays a per-packet wire
overhead that raises complete-datagram bitrate by about +47% at 24 kb/s
and +18% at 96 kb/s (fixed 36-byte media header plus 16-byte AEAD tag per
packet). A wall-clock-matched 160 ms PLC window is also concealed better
by the 20 ms shape on these content classes, and the per-second
encode+decode wall cost is the same order of magnitude for both shapes
(20 ms about 3-13% lower in a second short-window run; the 120 s
sustained pass favors 20 ms by about 6%). The comparison is codec-level
evidence only; it does not select a frame duration and is not a latency,
power, or Android result (see the
[10 ms vs 20 ms record](../measurements/2026-08-16-windows-opus-10ms-vs-20ms.md)).

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
