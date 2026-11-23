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
supplied. On 2026-07-19, the reference workspace had no CMake executable in
the conventional Program Files or per-user locations and no Android SDK NDK
directory. The command host could not execute a PATH probe, so this is an
incomplete preflight observation rather than a claim that no tool exists.
On 2026-08-16 the reference host had Visual Studio 18 Community (CMake
4.2.3-msvc3, MSVC 14.50) available; the candidate built natively with the
documented `CMAKE_POLICY_VERSION_MINIMUM=3.5` workaround (see the build
record below). No Android SDK NDK is installed, so Android ABI packaging
remains unverified, and no production dependency is added until the full
gate is met.

## Decision

An isolated Android instrumentation spike may encode synthetic PCM with the
platform MediaCodec Opus encoder and export bounded, length-prefixed packets for
desktop libopus interoperability checks. This is a lab fixture, not a transport,
production encoder selection, Android capture, or evidence of device coverage.
The capture application's production boundary remains unchanged.

On a normal paired-lab stop, the Android spike gives MediaCodec a bounded
opportunity to accept end-of-stream and emit pending output before the UDP
sender is drained and StopStream is sent. An incomplete final PCM frame is
zero-padded to the lab's 20 ms frame size. If the encoder does not finish within
the deadline, shutdown continues; this is best effort and does not prove tail
delivery or physical playback.

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

The codec-neutral `OpusFrameConfig` now validates the selected 48 kHz stereo
10/20 ms shape and its 960/1,920 interleaved Float32 PCM requirement before a
native binding is introduced. It may validate a larger reserved block for a
shorter selected frame, but does not relax decoder output-length checks.
The deterministic A-F simulator also constructs mock `StreamDecodeWorker`
instances through `new_for_opus` for both fixed capacities. Those runs prove
worker/PLC/queue control flow only; they are not a libopus, quality, power, or
latency result.
A native initial-path implementation must construct its stream worker through
`StreamDecodeWorker::new_for_opus`, so insufficient fixed PCM capacity fails in
the control/per-stream creation domain before decoder work is scheduled.
The receiver and UDP receiver compositions can accept a preconstructed worker,
so an isolated lab can use that validated worker with protected UDP ingress
and an output endpoint. The codec binding itself remains confined to the
existing prototype until the production gate is met.
The UDP and local audio labs reset native decoder state when the worker retires a
playout epoch, including output recovery. A reset failure is surfaced on the
next decode or PLC operation; it is not silently treated as a successful reset.

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
The build gate and proposed experiment cells are maintained in
[Opus build and experiment matrix](../measurements/opus-build-and-experiment-matrix.md).
An initial [toolchain preflight](../measurements/2026-07-20-opus-native-preflight.md) records the
read-only availability probe; it is an observation, not a build or quality claim.

A [Windows native build record](../measurements/2026-08-16-windows-opus-native-build.md)
documents the successful `opus` 0.3.1 build on the reference host and the
`tools/opus-codec-prototype` decode/PLC probe through
`StreamDecodeWorker::new_for_opus` for both negotiated 10 ms and 20 ms shapes,
plus a deterministic malformed-payload sweep and decode/PLC RMS sanity
checks; that record is native-build and basic decode/PLC evidence only and
does not close the Android, quality, or coverage-guided fuzz gates.
Physical latency and Android capture claims remain separate tests.

A [deterministic in-band FEC content-class record](../measurements/2026-08-16-windows-opus-inband-fec.md)
documents that 20 ms in-band FEC recovers a dropped speech-like frame at
22.3 dB versus 16.3 dB for PLC (+6.0 dB) and adds nothing for non-speech
references: a tone profile (FEC equals PLC at 33.1 dB) and a
multi-harmonic music-like profile (FEC equals PLC at 2.4 dB). A
speech-like loss-rate sweep shows
the benefit appears only once the announced loss is high enough for the
encoder to leave CELT-only mode and code LBRR (measured +6.0 dB at 10% and
+4.3 dB at 20%, with FEC equal to PLC at 5%), and a wall-clock pass shows
enabling FEC costs about +214-232% direct codec-level encode+decode time at
the auto bitrate (hybrid mode) and +108-116% at 24 kb/s (SILK mode). It is
codec-level evidence through
the opus binding directly; the `FrameDecoder` boundary has no FEC flag, the
10 ms shape cannot carry LBRR redundancy, and FEC remains disabled.

A [DTX content-class record](../measurements/2026-08-16-windows-opus-dtx.md)
documents that libopus DTX reduces the average packet size by 35% on
pause-gapped speech-like content (249 to 163 bytes, 84 of 240 one-byte
DTX packets) with no measurable active-region SNR change (14.1 dB), and
does not engage on a continuous tone (0 DTX packets, 45.1 dB). The same
35% reduction (129 to 84 bytes) and 84/240 DTX-packet count now also hold
at 10 ms, with no active-region SNR change (15.4 dB) and no DTX packets on
tone (41.2 dB). It is codec-level evidence through the opus binding
directly and not a power measurement; DTX remains disabled.

A [10 ms vs 20 ms frame-duration record](../measurements/2026-08-16-windows-opus-10ms-vs-20ms.md)
documents that at equal per-second codec bitrates the 20 ms shape keeps a
steady-state SNR lead on the tone and multi-harmonic profiles (largest at
24 kb/s: +11.4 dB tone, +4.7 dB multi-harmonic), conceals a
wall-clock-matched 160 ms PLC window better, and costs no more per
audio-second, while the 10 ms shape raises complete-datagram bitrate by
about +47% at 24 kb/s and +18% at 96 kb/s because it doubles the
per-packet media-header and AEAD-tag overhead. This partially addresses
the 10 ms vs 20 ms quality, CPU, and loss/PLC evidence gate; it is
codec-level evidence only and does not select a frame duration, and power,
Android ABI, and sustained coverage-guided fuzz evidence remain open.

## Rollback plan

The optional feature remains behind `FrameDecoder`. It can be disabled or
replaced with another reviewed libopus binding without changing protocol, network,
realtime queue, or UI contracts.
