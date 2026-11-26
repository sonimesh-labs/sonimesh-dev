# Android MediaCodec input and Opus packet duration

On 2026-11-26, `OpusFrameDurationTest` ran on the isolated Android 17 API 37
`sdk_gphone64_x86_64` emulator, build `CE2A.260420.051.A2/16418371`. The
selected encoder was `c2.android.opus.encoder`, configured for 48 kHz stereo
PCM and a requested 96 kb/s. The test submitted 40 synthetic PCM buffers for
each input size and drained end-of-stream. It measured packet duration from the
Opus table of contents; it did not measure audio quality or latency.
To repeat, run `:audio-encoder:connectedDebugAndroidTest` with the instrumentation
class `com.sonimesh.sender.encoder.OpusFrameDurationTest`; the
`SoniMeshOpusDurations` log tag reports the codec and duration counts.

| PCM buffer duration | Output packets | Observed packet durations |
| --- | ---: | --- |
| 10 ms | 20 | 20 packets of 20 ms |
| 20 ms | 40 | 40 packets of 20 ms |

The test passed, including end-of-stream on both runs. On this emulator,
splitting the input into 10 ms buffers did not produce 10 ms packets. This is
one software encoder observation, not a claim about other Android versions,
OEM codecs, physical devices, or a configurable frame-duration setting. The
Android lab continues to negotiate 20 ms; a 10 ms sender requires an encoder
configuration or implementation whose output duration has been verified.
