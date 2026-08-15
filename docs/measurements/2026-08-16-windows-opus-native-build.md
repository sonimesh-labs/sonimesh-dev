# Windows Opus native build - 2026-08-16

> Status: native libopus build and decode/PLC probe on the reference Windows
> host, including a deterministic five-frame loss burst, a 1-12 frame
> burst-length sweep, independent/burst loss-pattern windows, and bounded
> reorder absorption and late-drop, all with recovery. ADR 0009 remains
> Provisional: Android ABI packaging, CPU/power, jitter-wait loss profiles,
> bursts beyond 12 frames, and coverage-guided malformed-payload fuzzing
> against the native decoder are still pending.
> Updated: 2026-08-16

## Summary

The candidate `opus` 0.3.1 binding (bundled libopus through `audiopus_sys`
0.2.2) built on the reference host and ran the revised
`tools/opus-codec-prototype` probe. For each negotiated shape (10 ms and 20 ms
stereo), the probe encodes a deterministic tone history, decodes it through
the `sonimesh-audio` `StreamDecodeWorker::new_for_opus` boundary, compares the
steady-state decoded block against the source with model-free aligned SNR,
conceals a deterministic burst of five consecutive lost frames with
decoder-side PLC, sweeps burst lengths from 1 to 12 frames to record how PLC
quality degrades and confirm the stream re-locks to the source after every
length, runs deterministic loss-pattern windows (independent 1/3/5% and a
periodic 5-in-50 burst) to record quality under loss-over-time, probes
bounded reorder absorption (a six-frame out-of-order arrival batch played
out in sequence with no concealment at the constant codec delay) and
late-drop (one slot concealed by PLC while the bounded buffer drops the
late frame), and sweeps deterministic malformed payloads that must surface
as bounded stream-local errors. The PCM block is sized to the negotiated
frame on each
shape, so PLC synthesizes exactly one frame of concealment per missing frame.

## Command

~~~powershell
# From the workspace root, with the Visual Studio CMake bin on PATH:
$vsCm = 'C:\Program Files\Microsoft Visual Studio\18\Community\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin'
$env:PATH = $vsCm + ';' + $env:PATH
$env:CMAKE_POLICY_VERSION_MINIMUM = '3.5'
cargo run -p opus-codec-prototype
~~~

## Observation

| Field | Observed value |
| --- | --- |
| Local time | 2026-08-16 (Asia/Shanghai) |
| Windows version | `10.0.26200.0` |
| Rust | `rustc 1.85.0`, `cargo 1.85.0` |
| CMake | `4.2.3-msvc3` (Visual Studio 18 Community, not on PATH by default) |
| MSVC | `14.50.35717` (Visual Studio 18 Community) |
| Installed Rust targets | `x86_64-pc-windows-msvc` |
| Android SDK NDK | Absent |
| ADB connected devices | None |

## Build result

The bundled libopus CMakeLists uses a `cmake_minimum_required` floor below
CMake 3.5. CMake 4.x removed compatibility with releases older than 3.5, so
the first build attempt failed with:

```text
CMake Error at CMakeLists.txt:1 (cmake_minimum_required):
  Compatibility with CMake < 3.5 has been removed from CMake.
  Update the VERSION argument <min> value. Or, use the <min>...<max> syntax
  to tell CMake that the project requires at least <min> but has been updated
  to work with policies introduced by <max> or earlier.
  Or, add -DCMAKE_POLICY_VERSION_MINIMUM=3.5 to try configuring anyway.
```

Setting the documented environment variable `CMAKE_POLICY_VERSION_MINIMUM=3.5`
lets CMake apply the pre-3.5 policy floor, and the native build completed.
The probe then passed every step:

```text
opus_10ms: ok (encode 141 B/frame; decode 960/PLC 960 samples; snr 36.9 dB; plc burst 5 min 2.8 mean 16.6 dB; align 733; recovery 42.4 dB; delay 624 samples)
opus_10ms_burst: len 1 ok (min 40.0 mean 40.0 recovery 33.4 dB)
opus_10ms_burst: len 2 ok (min 22.7 mean 31.4 recovery 26.7 dB)
opus_10ms_burst: len 3 ok (min 11.4 mean 24.7 recovery 37.9 dB)
opus_10ms_burst: len 5 ok (min 2.8 mean 16.6 recovery 42.4 dB)
opus_10ms_burst: len 8 ok (min 1.3 mean 11.0 recovery 37.7 dB)
opus_10ms_burst: len 12 ok (min 0.4 mean 7.5 recovery 26.1 dB)
opus_20ms: ok (encode 273 B/frame; decode 1920/PLC 1920 samples; snr 44.9 dB; plc burst 5 min 4.1 mean 18.5 dB; align 733; recovery 48.1 dB; delay 624 samples)
opus_20ms_burst: len 1 ok (min 48.4 mean 48.4 recovery 43.2 dB)
opus_20ms_burst: len 2 ok (min 22.4 mean 35.4 recovery 42.8 dB)
opus_20ms_burst: len 3 ok (min 11.2 mean 27.3 recovery 40.9 dB)
opus_20ms_burst: len 5 ok (min 4.1 mean 18.5 recovery 48.1 dB)
opus_20ms_burst: len 8 ok (min 1.0 mean 12.1 recovery 41.5 dB)
opus_20ms_burst: len 12 ok (min 0.4 mean 8.3 recovery 38.9 dB)
opus_10ms_loss: pattern none ok (240 frames; 0 dropped; mean 39.0 worst 33.2 dB)
opus_10ms_loss: pattern independent_1pct ok (240 frames; 2 dropped; mean 38.5 worst 3.4 dB)
opus_10ms_loss: pattern independent_3pct ok (240 frames; 7 dropped; mean 36.7 worst -1.5 dB)
opus_10ms_loss: pattern independent_5pct ok (240 frames; 10 dropped; mean 35.7 worst -1.4 dB)
opus_10ms_loss: pattern burst_5in50 ok (240 frames; 20 dropped; mean 36.2 worst 1.5 dB)
opus_20ms_loss: pattern none ok (240 frames; 0 dropped; mean 41.3 worst 34.2 dB)
opus_20ms_loss: pattern independent_1pct ok (240 frames; 2 dropped; mean 40.7 worst -1.1 dB)
opus_20ms_loss: pattern independent_3pct ok (240 frames; 7 dropped; mean 39.8 worst -1.2 dB)
opus_20ms_loss: pattern independent_5pct ok (240 frames; 10 dropped; mean 38.8 worst -1.2 dB)
opus_20ms_loss: pattern burst_5in50 ok (240 frames; 20 dropped; mean 38.4 worst 2.3 dB)
opus_10ms_reorder: absorbed ok (6 frames; 0 concealed; mean 39.1 worst 37.8 dB)
opus_10ms_reorder: late_one ok (1 concealed snr 40.0 dB; late drop 1; decoded mean 30.7 dB)
opus_20ms_reorder: absorbed ok (6 frames; 0 concealed; mean 41.2 worst 38.9 dB)
opus_20ms_reorder: late_one ok (1 concealed snr 48.4 dB; late drop 1; decoded mean 40.8 dB)
opus_malformed_payload: rejected
opus_malformed_sweep: ok (1000 payloads; 642 rejected; 358 accepted-bounded)
opus_native_build: ok
```

## Interpretation

This is evidence that the candidate binding builds natively on the reference
Windows host and that its decode and PLC paths work through the bounded
`StreamDecodeWorker` boundary with decode writing into caller-owned storage.
The steady-state decoded tone keeps a 36.9 dB SNR for the 10 ms shape and
44.9 dB for the 20 ms shape against the source, compensated for the codec's
624-sample interleaved lookahead delay. Over a deterministic five-frame loss
burst, decoder-side PLC keeps every concealed block non-silent with a per-block
SNR floor of 2.8 dB (10 ms) and 4.1 dB (20 ms) and a burst mean of 16.6 dB and
18.5 dB against the actual dropped frames; after the burst the stream re-locks
to the source with a recovered-frame SNR of 42.4 dB and 48.1 dB. The reported
733-sample alignment on the first concealed block equals the lookahead plus one
440 Hz tone period. The burst-length sweep (1, 2, 3, 5, 8, 12 frames) records
the PLC degradation curve: the worst concealed block drops monotonically from
40.0 dB (10 ms) / 48.4 dB (20 ms) at one lost frame to 0.4 dB at twelve lost
frames (120 ms of loss at the 10 ms shape), while the burst mean falls to
7.5 dB and 8.3 dB respectively; no concealed block goes silent and the stream
re-locks after every swept length with recovery SNR at or above 26 dB. That is
the first on-host quality signal for the 10 ms versus 20 ms comparison at
comparable bitrate (141 B/frame at 10 ms and 273 B/frame at 20 ms are both
about 110 kb/s) and a first longer-burst signal, but it is one deterministic
tone on one host, not a CPU/power, jitter-wait, reorder, or beyond-12-frame
result. The loss-pattern windows (240 frames each, patterns applied after a
10-frame warm-up history, immediate concealment with no jitter wait) keep the
window mean SNR between 39.0 dB and 35.7 dB for the 10 ms shape and between
41.3 dB and 38.4 dB for the 20 ms shape as loss rises from 0% to 10%
(independent 1/3/5% plus a periodic 5-in-50 burst), compared at the codec's
constant delay; no block goes silent, and the drop counts are deterministic
per seed (2, 7, 10, and 20 of 240 frames). Individual concealed blocks can
deviate sharply from the true frame at the codec delay (worst about -1.5 dB)
because PLC applies amplitude decay over concealed frames, while the window
mean stays well above the 10 dB floor. These are immediate-concealment
results; a jitter buffer that waits before declaring a loss remains
untested. The malformed-payload sweep remains a deterministic bounded probe
(fixed-seed xorshift32, 1,000 payloads up to the datagram limit) rather
than a coverage-guided fuzz result.

The bounded reorder probe adds the first jitter-buffer reorder signal.
With a six-frame out-of-order arrival batch (sequences 13, 10, 15, 12, 11,
14) inside the 8-slot jitter capacity, playout emits every frame in
sequence with no concealment, and each block keeps the codec constant-delay
SNR (window mean 39.1 dB at 10 ms and 41.2 dB at 20 ms), so absorption does
not shift the stream. When one frame arrives one slot late, exactly one
block is concealed by PLC (model-free SNR 40.0 dB and 48.4 dB), the two
following frames decode in order (mean 30.7 dB and 40.8 dB), and the bounded
buffer reports the late frame as dropped. This is immediate-concealment
evidence with no jitter-wait tuning; an adaptive target and deeper reorder
profiles remain untested.

A sizing detail changed in this revision: the PCM block is now the negotiated
frame size (960 samples for 10 ms, 1,920 for 20 ms) instead of the reserved
20 ms block, so an empty packet makes libopus synthesize exactly one frame of
concealment per missing frame on both shapes. The earlier full-block PLC for
the 10 ms shape is gone; a production 10 ms stream should size its block to
960 samples.

## Remaining ADR 0009 gates

- reproducible native build on the Android ABIs (`arm64-v8a`, `x86_64`) with
  a pinned NDK;
- CPU and power evidence for both frame durations, plus jitter-wait loss
  profiles and burst behavior beyond the 1-12 frame sweep on this host;
- authenticated malformed-payload fuzzing against the native decoder;
- physical latency and Android capture claims remain separate tests.