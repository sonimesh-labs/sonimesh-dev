# Windows Opus in-band FEC content-class probe - 2026-08-16

> Status: deterministic codec-level evidence on the reference Windows host
> that libopus in-band FEC recovers one dropped 20 ms frame with 22.3 dB
> aligned SNR versus 16.3 dB for decoder-side PLC on voiced speech-like
> content (+6.0 dB), and adds nothing for a single-tone non-speech reference
> (FEC equals PLC at 33.1 dB). A speech-like loss-rate sweep shows the
> benefit appears at 10% and 20% announced loss but not at 5% (FEC equals
> PLC), and a wall-clock pass shows enabling FEC costs about +213-232%
> direct codec-level encode+decode time at the auto bitrate and +108-116%
> at 24 kb/s. FEC remains disabled in every experiment cell and ADR 0009
> stays Provisional.
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

The same probe then sweeps the announced loss rate on the speech-like
profile (`set_packet_loss_perc(5)` and `set_packet_loss_perc(20)`, each
with the same single dropped frame 120) so the recovery benefit is
recorded as a function of the loss the encoder is told to expect.
Finally it times three direct codec-level encode+decode scenarios - no
FEC, FEC with no actual loss, and FEC with every 10th packet recovered -
at the auto bitrate and at 24 kb/s, repeating each pass three times and
reporting the minimum wall time, so the cost of enabling FEC is
recorded per bitrate mode.

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

The probe adds two deterministic FEC recovery lines, a deterministic
loss-rate sweep on the speech-like profile, and two wall-clock FEC CPU lines
after the 20 ms sustained pass:

```text
opus_20ms_fec: speech fec 22.3 dB plc 16.3 dB gain +6.0 dB (loss 10% drop 120 avg 187 B)
opus_20ms_fec: tone fec 33.1 dB plc 33.1 dB gain +0.0 dB (loss 10% drop 120 avg 170 B)
opus_20ms_fec_loss: speech loss 5% fec 18.6 dB plc 18.6 dB gain +0.0 dB (drop 120 avg 245 B)
opus_20ms_fec_loss: speech loss 20% fec 21.5 dB plc 17.2 dB gain +4.3 dB (drop 120 avg 190 B)
opus_20ms_fec_cpu: auto no-fec 1484-1536 us/frame; fec no-loss +214-224%; fec loss +222-232% (speech, min of 3)
opus_20ms_fec_cpu: 24k no-fec 1174-1176 us/frame; fec no-loss +108%; fec loss +109-116% (speech, min of 3)
```

The four deterministic quality lines are byte-identical across two release
invocations, and all other quality lines remain identical to the committed
build record. The two `fec_cpu` lines are wall-clock and vary within the
ranges shown above across runs; they are codec-level estimates and are not
comparable to the worker-path `opus_*ms_cpu` numbers.

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
- Loss-rate sweep (speech-like): at 5% announced loss the encoder stays in
  CELT-only mode and codes no LBRR, so the FEC decode falls back to PLC and
  the sweep reports FEC equal to PLC at 18.6 dB with no gain. At 10% and
  20% the encoder leaves CELT-only mode and codes redundancy, recovering
  +6.0 dB (22.3 dB FEC versus 16.3 dB PLC) and +4.3 dB (21.5 dB FEC versus
  17.2 dB PLC) respectively. The benefit is therefore tied to the
  encoder's mode decision, not merely to the presence of a drop.
- Wall-clock FEC CPU pass: enabling in-band FEC at the auto bitrate forces
  the encoder into hybrid mode (CELT plus SILK LBRR), which triples direct
  codec-level encode+decode wall time (about +214-232% depending on loss);
  at 24 kb/s the encoder switches to SILK mode and the same pass costs
  about +108-116%. These numbers are wall-clock min-of-3 estimates from
  direct `opus` calls, not a measured CPU-time trace and not the
  worker-path `StreamDecodeWorker` cost; the real in-stream overhead
  depends on the negotiated bitrate and frame shape.

## Gates enforced by the probe

- Speech-like FEC gain must be at least 4.0 dB at the 10% announced-loss
  cell (measured +6.0) and at the 20% sweep cell (measured +4.3). The 5%
  sweep cell is report-only: the encoder codes no LBRR there, so FEC equals
  PLC and the gain is 0.0 dB by design.
- Non-speech FEC must stay within 2.0 dB of PLC (measured +0.0).
- The FEC CPU pass is report-only: wall-clock min-of-3 estimates are not
  gated because they vary run to run; the recorded mode-dependent overhead
  is evidence for a later loss/CPU/power tradeoff decision.

## Remaining ADR 0009 gates

- reproducible native build on the Android ABIs (`arm64-v8a`, `x86_64`);
- measured CPU-time utilization, allocation profile, and power evidence for
  both frame durations;
- sustained coverage-guided malformed-payload fuzzing against the native
  decoder;
- FEC enablement itself: this record is evidence, not a policy decision; FEC
  remains disabled in every experiment cell until a reviewed configuration
  and loss/CPU/power tradeoff are decided.
