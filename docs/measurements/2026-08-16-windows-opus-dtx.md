# Windows Opus DTX probe - 2026-08-16

> Status: deterministic codec-level evidence on the reference Windows host
> that libopus DTX (discontinuous transmission) reduces the average encoded
> packet size by 35% on pause-gapped speech-like content at both negotiated
> shapes (249 to 163 bytes at 20 ms, 129 to 84 bytes at 10 ms; 84 of 240
> packets become one-byte DTX packets in both cases) with no measurable
> change to active-region SNR (14.1 dB at 20 ms, 15.4 dB at 10 ms, DTX on
> and off), and does not engage on a continuous non-speech tone (0 DTX
> packets at both shapes). DTX remains disabled in every experiment cell and
> ADR 0009 stays Provisional.
> Updated: 2026-08-16

## Summary

`tools/opus-codec-prototype` encodes 240 stereo frames of each negotiated
shape (10 ms and 20 ms) of two deterministic content profiles with DTX off
and on and reports the average packet size, the number of DTX packets, and
the mean aligned SNR over active frames. A packet is classified as a DTX
packet when its encoded length is at most one byte: libopus generalized DTX
emits exactly the TOC byte (`data[0] = gen_toc(...); return 1` in
`opus_encode_native`). The DTX engage counter is about 10 packets, so DTX
starts after about 200 ms of continuous inactivity at 20 ms
(`NB_SPEECH_FRAMES_BEFORE_DTX` is 10 in `silk/define.h`) and after about
100 ms at 10 ms; the pause profile uses silent gaps long enough for both
shapes (24 pause frames: 480 ms at 20 ms, 240 ms at 10 ms).

## Content profiles

- **Pauses** (new): the existing deterministic speech-like profile with
  every 24-frame silent pause replacing 16 frames of active speech, cycling
  over 240 frames. At 20 ms a pause is 480 ms and the active block 320 ms;
  at 10 ms the same frame counts give a 240 ms pause and a 160 ms active
  block. The pause is long enough for the SILK VAD to go inactive and for
  generalized DTX to emit one-byte packets on both shapes.
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

The probe adds four deterministic lines after the FEC CPU pass:

```text
opus_20ms_dtx: pauses avg 249 -> 163 B (35% smaller); dtx pkts 84/240; active snr 14.1 vs 14.1 dB
opus_20ms_dtx: tone avg 250 -> 250 B (0% smaller); dtx pkts 0/240; snr 45.1 vs 45.1 dB
opus_10ms_dtx: pauses avg 129 -> 84 B (35% smaller); dtx pkts 84/240; active snr 15.4 vs 15.4 dB
opus_10ms_dtx: tone avg 129 -> 129 B (0% smaller); dtx pkts 0/240; snr 41.2 vs 41.2 dB
```

The 20 ms lines are byte-identical to the committed build record, and the
10 ms lines use the identical deterministic pipeline (fixed content and auto
bitrate), so their quality and packet-size fields are deterministic too.

## Interpretation

- Pause-gapped speech at 20 ms: DTX engages during each silent pause and
  turns 84 of 240 packets into one-byte DTX packets, reducing the average
  packet size from 249 to 163 bytes (35%). The active-region SNR is
  unchanged at 14.1 dB with DTX on and off, so the bandwidth saving does not
  degrade the speech that is actually transmitted.
- Pause-gapped speech at 10 ms: the same 84 of 240 packets become one-byte
  DTX packets and the average size drops from 129 to 84 bytes (35%). The
  engage counter is about 10 packets, so at 10 ms DTX starts after roughly
  100 ms of inactivity instead of 200 ms, which lets the shorter shape
  harvest the same saving from the same 24-frame pause. Active-region SNR is
  unchanged at 15.4 dB with DTX on and off.
- Continuous tone: the tone never goes inactive, so DTX emits no DTX packets
  at either shape and both the packet size and the SNR are unchanged
  (45.1 dB at 20 ms, 41.2 dB at 10 ms). DTX is therefore content-dependent,
  not a flat saving.
- The 10 ms tone baseline (41.2 dB) is below the 20 ms baseline (45.1 dB) on
  identical auto-bitrate content, consistent with the frame-duration
  comparison record: shorter frames carry less temporal context at equal
  per-second bitrate.
- This is codec-level evidence through the opus binding directly; the stable
  `sonimesh-audio` `FrameDecoder` boundary is not involved, and the run is
  not a power measurement.

## Gates enforced by the probe

The reduction and SNR gates are calibrated for and enforced on the 20 ms
cell; the 10 ms cell reports the same numbers without gating them. The
tone-control invariant (no DTX packets on continuous tone) is enforced on
both shapes.

- Pause content at 20 ms: DTX must reduce the average packet size by at
  least 20% (measured 35%).
- Pause content at 20 ms: DTX must keep the active-region SNR within 1.5 dB
  of the DTX-off baseline (measured 0.0 dB drop).
- Tone control: DTX must emit no DTX packets at either shape (measured 0)
  and, at 20 ms, must stay within 1.5 dB of the baseline (measured 0.0 dB
  drop; the 10 ms on/off delta is also 0.0 dB).

## Remaining ADR 0009 gates

- reproducible native build on the Android ABIs (`arm64-v8a`, `x86_64`);
- measured CPU-time utilization, allocation profile, and power evidence for
  both frame durations;
- sustained coverage-guided malformed-payload fuzzing against the native
  decoder;
- DTX enablement itself: this record is evidence, not a policy decision; DTX
  remains disabled in every experiment cell until a reviewed configuration
  and measured bandwidth/power tradeoff are decided.
