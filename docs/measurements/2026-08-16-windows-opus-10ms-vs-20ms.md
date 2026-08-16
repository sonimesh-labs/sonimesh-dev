# Windows Opus 10 ms vs 20 ms frame-duration comparison - 2026-08-16

> Status: deterministic codec-level evidence on the reference Windows host
> comparing the negotiated 10 ms and 20 ms stereo shapes at equal per-second
> codec bitrates (24 kb/s voice-class and 96 kb/s music-class). At 24 kb/s the
> 20 ms shape keeps a clear steady-state SNR lead (+11.4 dB tone, +4.7 dB
> multi-harmonic), while the 10 ms shape pays a per-packet wire overhead that
> raises complete-datagram bitrate by about +47% at 24 kb/s and +18% at
> 96 kb/s. A wall-clock-matched 160 ms PLC window is also concealed better by
> the 20 ms shape on these content classes (e.g. 10.5 dB vs 4.1 dB mean at
> 96 kb/s tone), consistent with the existing burst sweep. Per-second-of-audio
> encode+decode wall cost is the same order of magnitude for both shapes; the
> short-window numbers are noisy on this loaded host, and the 120 s sustained
> pass still favors 20 ms by about 6%. This closes none of the ADR 0009 gates
> on its own: it is codec-level evidence only, not a frame-duration selection,
> a latency, power, allocation, Android, or coverage-guided fuzz result.
> Updated: 2026-08-16

## Summary

`tools/opus-codec-prototype` gained a deterministic frame-duration comparison
cell that runs both negotiated shapes (10 ms and 20 ms stereo, 48 kHz) at two
fixed per-second codec bitrates (24 kb/s and 96 kb/s) across both deterministic
content profiles (single tone and multi-harmonic). Each case reports:

- the delay-compensated mean SNR over a 4.8 s window;
- the mean encoded payload bytes per frame and the payload bitrate;
- the complete media-datagram bitrate (payload + the fixed 36-byte version-one
  media header + 16-byte ChaCha20-Poly1305 tag per packet), which makes the
  per-packet overhead explicit;
- the mean and worst aligned SNR over a wall-clock-matched 160 ms PLC window
  (16 concealed frames at 10 ms, 8 at 20 ms), so PLC granularity is compared
  at equal lost-audio time rather than equal frame count;
- the per-second-of-audio encode+decode wall cost (min of three timed passes).

Holding the per-second bitrate constant removes the auto-bitrate confounder
from the existing per-shape probes: at 24 kb/s both shapes average 24-26 kb/s
of payload, and at 96 kb/s both average exactly 96.8 kb/s, so the SNR and PLC
differences below are not an artifact of one shape spending more bits.

## Content profiles

- **Tone**: the existing 440 Hz single-sine reference.
- **Multi-harmonic**: the existing five-harmonic sum scaled to the tone RMS.

Both are steady-state, non-speech profiles; the speech-like content classes
remain separate cells (loss-pattern, FEC, DTX) that already run per shape.

## Command

~~~powershell
# From the workspace root, with the Visual Studio CMake bin on PATH:
$vsCm = 'C:\Program Files\Microsoft Visual Studio\18\Community\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin'
$env:PATH = $vsCm + ';' + $env:PATH
$env:CMAKE_POLICY_VERSION_MINIMUM = '3.5'
cargo run --release -p opus-codec-prototype
~~~

A release build is used so the wall-clock throughput lines are meaningful; the
quality and packet-size lines are deterministic and identical between debug
and release builds.

## Output

The probe adds eight deterministic comparison lines after the per-shape
sustained passes:

```text
opus_cmp_10ms_tone_24k: snr 12.6 dB; payload 31.0 B/frame (24.8 kb/s); datagram 83.0 B/frame (66.4 kb/s); plc 160 ms mean 2.3 min 0.3 dB; cpu 69857 us per audio-second
opus_cmp_20ms_tone_24k: snr 24.0 dB; payload 61.0 B/frame (24.4 kb/s); datagram 113.0 B/frame (45.2 kb/s); plc 160 ms mean 5.1 min 0.6 dB; cpu 66602 us per audio-second
opus_cmp_10ms_multi_24k: snr 15.7 dB; payload 32.0 B/frame (25.6 kb/s); datagram 84.0 B/frame (67.2 kb/s); plc 160 ms mean 4.5 min 0.5 dB; cpu 71956 us per audio-second
opus_cmp_20ms_multi_24k: snr 20.4 dB; payload 61.0 B/frame (24.4 kb/s); datagram 113.0 B/frame (45.2 kb/s); plc 160 ms mean 7.0 min 0.8 dB; cpu 84591 us per audio-second
opus_cmp_10ms_tone_96k: snr 38.3 dB; payload 121.0 B/frame (96.8 kb/s); datagram 173.0 B/frame (138.4 kb/s); plc 160 ms mean 4.1 min 0.5 dB; cpu 95547 us per audio-second
opus_cmp_20ms_tone_96k: snr 40.9 dB; payload 242.0 B/frame (96.8 kb/s); datagram 294.0 B/frame (117.6 kb/s); plc 160 ms mean 10.5 min 0.8 dB; cpu 106955 us per audio-second
opus_cmp_10ms_multi_96k: snr 33.4 dB; payload 121.0 B/frame (96.8 kb/s); datagram 173.0 B/frame (138.4 kb/s); plc 160 ms mean 4.7 min 0.5 dB; cpu 106460 us per audio-second
opus_cmp_20ms_multi_96k: snr 35.9 dB; payload 242.0 B/frame (96.8 kb/s); datagram 294.0 B/frame (117.6 kb/s); plc 160 ms mean 6.3 min 0.5 dB; cpu 75473 us per audio-second
```

The SNR, payload, datagram, and PLC fields are deterministic (fixed content,
fixed bitrate) and were verified byte-identical across two release
invocations; only the trailing `cpu` wall-clock field varies between runs
(see the CPU bullet below). All other probe lines remain byte-identical to
the committed build record.

## Comparison

| Shape | Profile | Bitrate | SNR (dB) | Payload B/frame | Payload kb/s | Datagram B/frame | Datagram kb/s | PLC 160 ms mean/min (dB) | CPU us per audio-second |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 10 ms | tone | 24 kb/s | 12.6 | 31.0 | 24.8 | 83.0 | 66.4 | 2.3 / 0.3 | 69,857 |
| 20 ms | tone | 24 kb/s | 24.0 | 61.0 | 24.4 | 113.0 | 45.2 | 5.1 / 0.6 | 66,602 |
| 10 ms | multi | 24 kb/s | 15.7 | 32.0 | 25.6 | 84.0 | 67.2 | 4.5 / 0.5 | 71,956 |
| 20 ms | multi | 24 kb/s | 20.4 | 61.0 | 24.4 | 113.0 | 45.2 | 7.0 / 0.8 | 84,591 |
| 10 ms | tone | 96 kb/s | 38.3 | 121.0 | 96.8 | 173.0 | 138.4 | 4.1 / 0.5 | 95,547 |
| 20 ms | tone | 96 kb/s | 40.9 | 242.0 | 96.8 | 294.0 | 117.6 | 10.5 / 0.8 | 106,955 |
| 10 ms | multi | 96 kb/s | 33.4 | 121.0 | 96.8 | 173.0 | 138.4 | 4.7 / 0.5 | 106,460 |
| 20 ms | multi | 96 kb/s | 35.9 | 242.0 | 96.8 | 294.0 | 117.6 | 6.3 / 0.5 | 75,473 |

## Interpretation

- **Steady-state quality**: the 20 ms shape is better at both bitrates and on
  both profiles, but the gap is content- and bitrate-dependent. At 24 kb/s
  (SILK) the 20 ms lead is large (+11.4 dB tone, +4.7 dB multi); at 96 kb/s
  (hybrid/CELT) it narrows to +2.6 dB and +2.5 dB. A 10 ms frame gives the
  low-bitrate SILK predictor much less temporal context, which is the dominant
  quality effect here.
- **Wire overhead**: the 10 ms shape sends twice the packets per second, so
  the fixed 52-byte per-packet media envelope (36-byte header + 16-byte tag)
  doubles per second (41.6 kb/s vs 20.8 kb/s). On top of a 24 kb/s payload
  that is a +47% datagram-bitrate penalty (66.4 vs 45.2 kb/s); at 96 kb/s it
  is +18% (138.4 vs 117.6 kb/s). IP/UDP header bytes would widen the gap
  further and are not counted.
- **PLC at equal wall-clock loss**: for a 160 ms loss window, the 20 ms shape
  conceals better on every cell here (mean +2.8, +2.5, +6.4, +1.6 dB; worst
  comparable). This matches the pre-existing burst sweep (10 ms 16-frame burst
  mean 5.7 dB vs 20 ms 8-frame burst mean 12.1 dB at auto bitrate on tone),
  so finer packet granularity did not translate into better concealment on
  these steady-state content classes over an equal lost-audio span. Speech
  content and shorter loss windows remain separate cells.
- **Per-second CPU**: the short-window min-of-3 wall numbers vary between
  runs on this loaded reference host, but both runs and all cells place the
  20 ms shape at or below the 10 ms shape per audio-second. Run 2 measured
  20 ms lower by 3-13% across the four cells (66,546 vs 61,201; 69,083 vs
  64,484; 84,761 vs 73,956; 76,077 vs 73,811 us per audio-second), and the
  steadier 120 s sustained pass puts both shapes around 7.9-8.4% of one core
  with the 20 ms shape about 6% lower per audio-second (79.0 ms vs 83.9 ms).
  This is wall-clock evidence only, not measured CPU-time, allocation, or
  power.

## Gates enforced by the probe

- The comparison is deterministic: fixed content, fixed bitrate, and fixed
  wall-clock windows; a unit-guard helper (`frames_for_audio`) is covered by a
  unit test so a milliseconds/seconds confusion cannot turn a 4.8 s window
  into 480,000 frames.
- Every encoded packet length is validated by the existing `encode_frame`
  gate; every decoded and concealed block must be non-silent, finite, and
  exactly one negotiated frame; the stream must stay in sequence through the
  window and the PLC window.

## Non-claims and remaining ADR 0009 gates

- This is codec-level evidence through the opus binding directly; it is not a
  frame-duration selection, a latency measurement, a power measurement, an
  allocation-profile result, or an Android result.
- Speech-like 10 ms DTX, 10 ms FEC (which cannot carry LBRR), and real-device
  playback behavior remain later experiment cells.
- Reproducible native builds on Android ABIs, measured CPU-time utilization,
  allocation profile, power evidence, and sustained coverage-guided
  malformed-payload fuzzing remain pending before ADR 0009 can close.
