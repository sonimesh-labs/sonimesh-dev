# Windows Opus in-band FEC content-class probe - 2026-08-16

> Status: deterministic codec-level evidence on the reference Windows host
> that libopus in-band FEC recovers one dropped 20 ms frame with 22.3 dB
> aligned SNR versus 16.3 dB for decoder-side PLC on voiced speech-like
> content (+6.0 dB), and adds nothing for a single-tone non-speech reference
> (FEC equals PLC at 33.1 dB). FEC remains disabled in every experiment cell
> and ADR 0009 stays Provisional.
> Updated: 2026-08-16

## Summary

`tools/opus-codec-prototype` compares in-band FEC recovery against decoder-side
PLC for one deterministically dropped frame. libopus codes LBRR (low-bitrate
redundancy) only when a packet carries more than one frame, so the probe runs
at the 20 ms shape only; 10 ms packets cannot carry FEC and must rely on PLC.
For each content profile the probe encodes 240 stereo frames with
`set_inband_fec(true)` and `set_packet_loss_perc(10)`, drops frame 120, and
decodes the stream with the canonical one-packet-delay FEC flow from
`opus_demo.c`: the output of iteration `k` is frame `k - 1`, and when packet
`k - 1` was lost that frame is decoded from packet `k` with `fec = true`,
otherwise packet `k - 1` is decoded normally with `fec = false`. A fresh
decoder conceals the same dropped frame with PLC for comparison. Both
recovered blocks are scored against their source frame with the same
model-free aligned SNR used by the build record's concealed-block checks.

## Content profiles

- **Speech-like** (new): a deterministic voiced-speech model - a glottal pulse
  train at 120 Hz with exponential decay, shaped through two bandpass formant
  resonators (about 500 Hz and 2200 Hz) and multiplied by a 4 Hz syllabic
  amplitude modulation, scaled to the tone profile's peak amplitude. No
  randomness is used, so the quality lines are reproducible across runs.
- **Tone**: the existing 440 Hz single-sine reference.

## Command

~~~powershell
# From the workspace root, with the Visual Studio CMake bin on PATH:
$vsCm = 'C:\Program Files\Microsoft Visual Studio\18\Community\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin'
$env:PATH = $vsCm + ';' + $env:PATH
$env:CMAKE_POLICY_VERSION_MINIMUM = '3.5'
cargo run --release -p opus-codec-prototype
~~~

## Output

The probe adds two deterministic lines after the 20 ms sustained pass:

```text
opus_20ms_fec: speech fec 22.3 dB plc 16.3 dB gain +6.0 dB (loss 10% drop 120 avg 187 B)
opus_20ms_fec: tone fec 33.1 dB plc 33.1 dB gain +0.0 dB (loss 10% drop 120 avg 170 B)
```

The two FEC lines are byte-identical across two release invocations, and all
other quality lines remain identical to the committed build record.

## Interpretation

- Speech-like content: in-band FEC recovers the dropped frame at 22.3 dB
  aligned SNR versus 16.3 dB for PLC, a +6.0 dB content-class benefit.
- Non-speech reference: FEC equals PLC exactly (33.1 dB). Bundled libopus
  codes LBRR only when the SILK VAD reports speech activity above
  `LBRR_SPEECH_ACTIVITY_THRES` (0.3), so a pure tone carries no redundancy and
  the FEC decode falls back to PLC.
- The 10 ms shape cannot carry FEC: bundled libopus flushes LBRR data only
  when `nFramesPerPacket > 1` (`silk/enc_API.c`), so a 10 ms single-frame
  packet has no redundancy to recover. The probe therefore runs at 20 ms only.
- This is codec-level evidence through `opus::Decoder::decode_float(..., fec)`
  directly; the stable `sonimesh-audio` `FrameDecoder` boundary has no FEC
  flag and is not involved.

## Gates enforced by the probe

- Speech-like FEC gain must be at least 4.0 dB (measured +6.0, deterministic).
- Non-speech FEC must stay within 2.0 dB of PLC (measured +0.0).

## Remaining ADR 0009 gates

- reproducible native build on the Android ABIs (`arm64-v8a`, `x86_64`);
- measured CPU-time utilization, allocation profile, and power evidence for
  both frame durations;
- sustained coverage-guided malformed-payload fuzzing against the native
  decoder;
- FEC enablement itself: this record is evidence, not a policy decision; FEC
  remains disabled in every experiment cell until a reviewed configuration
  and loss/CPU/power tradeoff are decided.
