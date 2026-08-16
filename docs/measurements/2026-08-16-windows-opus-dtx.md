# Windows Opus DTX probe - 2026-08-16

> Status: deterministic codec-level evidence on the reference Windows host
> that libopus DTX (discontinuous transmission) reduces the average encoded
> packet size by 35% on pause-gapped speech-like content (249 to 163 bytes,
> 84 of 240 packets become one-byte DTX packets) with no measurable change to
> active-region SNR (14.1 dB with DTX on and off), and does not engage on a
> continuous non-speech tone (0 DTX packets, 250 to 250 bytes, 45.1 dB both
> ways). DTX remains disabled in every experiment cell and ADR 0009 stays
> Provisional.
> Updated: 2026-08-16

## Summary

`tools/opus-codec-prototype` encodes 240 stereo 20 ms frames of two
deterministic content profiles with DTX off and on and reports the average
packet size, the number of DTX packets, and the mean aligned SNR over active
frames. A packet is classified as a DTX packet when its encoded length is at
most one byte: libopus generalized DTX emits exactly the TOC byte
(`data[0] = gen_toc(...); return 1` in `opus_encode_native`). DTX engages
only after about 200 ms of continuous inactivity
(`NB_SPEECH_FRAMES_BEFORE_DTX` is 10 in `silk/define.h`, i.e. 10 x 20 ms),
so the pause profile uses silent gaps long enough to cross that window.

## Content profiles

- **Pauses** (new): the existing deterministic speech-like profile with
  every 24-frame (480 ms) silent pause replacing 16 frames (320 ms) of
  active speech, cycling over 240 frames. The pause is long enough for the
  SILK VAD to go inactive and for generalized DTX to emit one-byte packets.
- **Tone**: the existing 440 Hz single-sine reference, which is continuously
  active and should never trigger DTX.

## Command

~~~powershell
# From the workspace root, with the Visual Studio CMake bin on PATH:
$vsCm = 'C:\Program Files\Microsoft Visual Studio\18\Community\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin'
$env:PATH = $vsCm + ';' + $env:PATH
$env:CMAKE_POLICY_VERSION_MINIMUM = '3.5'
cargo run --release -p opus-codec-prototype
~~~

## Output

The probe adds two deterministic lines after the FEC CPU pass:

```text
opus_20ms_dtx: pauses avg 249 -> 163 B (35% smaller); dtx pkts 84/240; active snr 14.1 vs 14.1 dB
opus_20ms_dtx: tone avg 250 -> 250 B (0% smaller); dtx pkts 0/240; snr 45.1 vs 45.1 dB
```

The two DTX lines are byte-identical across two release invocations, and all
other quality lines remain identical to the committed build record.

## Interpretation

- Pause-gapped speech: DTX engages during each silent pause and turns 84 of
  240 packets into one-byte DTX packets, reducing the average packet size
  from 249 to 163 bytes (35%). The active-region SNR is unchanged at 14.1 dB
  with DTX on and off, so the bandwidth saving does not degrade the speech
  that is actually transmitted.
- Continuous tone: the tone never goes inactive, so DTX emits no DTX packets
  and both the packet size and the SNR are unchanged (45.1 dB). DTX is
  therefore content-dependent, not a flat saving.
- The 14.1 dB active-region figure reflects this deterministic dynamic
  content (hard speech-to-silence transitions), not a codec quality ceiling;
  the relevant comparison is DTX on versus off on identical content.
- This is codec-level evidence through the opus binding directly; the stable
  `sonimesh-audio` `FrameDecoder` boundary is not involved, the run is not a
  power measurement, and 10 ms DTX behavior is a later experiment cell.

## Gates enforced by the probe

- Pause content: DTX must reduce the average packet size by at least 20%
  (measured 35%).
- Pause content: DTX must keep the active-region SNR within 1.5 dB of the
  DTX-off baseline (measured 0.0 dB drop).
- Tone control: DTX must emit no DTX packets (measured 0) and must stay
  within 1.5 dB of the baseline (measured 0.0 dB drop).

## Remaining ADR 0009 gates

- reproducible native build on the Android ABIs (`arm64-v8a`, `x86_64`);
- measured CPU-time utilization, allocation profile, and power evidence for
  both frame durations;
- sustained coverage-guided malformed-payload fuzzing against the native
  decoder;
- DTX enablement itself: this record is evidence, not a policy decision; DTX
  remains disabled in every experiment cell until a reviewed configuration
  and measured bandwidth/power tradeoff are decided.
