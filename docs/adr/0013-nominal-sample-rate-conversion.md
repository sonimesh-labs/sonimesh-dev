# ADR 0013: Bounded nominal sample-rate conversion

- Status: Provisional
- Date: 2026-07-17
- Revised: 2026-08-17 (the audio primitive moved from linear interpolation to a
  33-tap Blackman-Harris windowed-sinc polyphase kernel; network, protocol,
  security, and platform boundaries are unchanged)

## Context

Android capture and Windows output devices may negotiate different sample rates.
The current host evidence includes both 48 kHz and 44.1 kHz endpoints. Passing
48 kHz PCM directly to a 44.1 kHz output would change playback speed and pitch.
The existing occupancy controller handles slow clock error decisions but is not
a sample-rate conversion stage.

The first feasibility primitive was a two-point linear interpolator. It is
bounded and allocation-free, but linear interpolation folds out-of-band images
and aliasing back into the passband, which is audible on difficult ratios (for
example 48 kHz to 44.1 kHz) and prevents an audio-quality claim.

## Decision

Add a bounded windowed-sinc polyphase resampler in `sonimesh-audio` and use it
in the pull-based `StreamFormatAdapter` in `sonimesh-app`.

- each output sample is a normalized 33-tap Blackman-Harris windowed-sinc
  convolution (16 past, center, and 16 future input frames); the kernel cutoff
  is the lower of the two Nyquist rates, so upsampling and downsampling are
  both bandlimited;
- input and output rates are restricted to 8–192 kHz and one through eight
  interleaved channels are supported, matching the earlier linear bounds;
- the kernel keeps 16 future input frames of lookahead; the caller retains the
  unconsumed input tail, so `process` has no internal history and performs no
  allocation;
- the source adapter preallocates input storage before stream start and the
  pump runs in the decode/control domain, never inside the output callback;
- a separate control-domain handle publishes the controller's bounded ppm
  adjustment atomically; hard-resync decisions remain stream-owner work;
- invalid alignment, rate, channel, and callback bounds fail closed to silence;
- the converter is a nominal-rate feasibility primitive, not a drift controller,
  a physical latency measurement, or a hardware/acoustic qualification.

## Reliability and realtime implications

The realtime adapter performs only bounded source pulls, fixed-buffer
compaction, and convolution. It does not allocate, lock, wait, access the
network, invoke UI code, or enumerate devices from `RenderSource::render`.
Underflow remains owned by the wrapped source and produces initialized silence.

The 16-frame lookahead adds a bounded start-up transient at stream admission
(the first output block is silence until the window fills) and 16 input frames
of retained storage per stream. The adapter sizes its scratch for the worst
positive drift bound so every later pump stays within preallocated storage.

Software quality evidence is strong (see below), but the final audio policy
must still compare the converter against the separate bounded drift-correction
stage and validate on physical hardware before making a latency or acoustic
quality claim.

## Validation evidence

- deterministic unit tests cover equal-rate passthrough, upsampling
  interpolation, downsampling positions, input/output alignment, invalid
  bounds, phase continuity, drift-bound behavior, and the windowed-sinc
  start-up transient;
- permanent SNR regressions feed a 997 Hz sine through 48→44.1 kHz and
  44.1→48 kHz conversion and assert SNR above 100 dB (measured 119.7 dB and
  121.9 dB over a 0.25 s deterministic window with f64 math and f32 output),
  and that the linear interpolator stays at least 20 dB below the polyphase
  result (measured 56.2 dB);
- passband gain at 100 Hz, 1 kHz, and 5 kHz through 48→44.1 kHz conversion
  stays within 0.001 dB of unity (permanent test enforces 0.1 dB);
- an 8 kHz tone through 48→8 kHz conversion (two octaves above the 4 kHz
  cutoff) measures −231 dB attenuation with a residual peak below 1e-11 of
  full scale;
- the `polyphase_resampler` LibFuzzer target feeds arbitrary rate, channel,
  drift, and buffer metadata into the convolution loop and asserts the bounded
  consumed/produced and initialized-output guarantees; CI compiles and runs it
  as a finite smoke gate;
- the implementation is covered by Rust fmt, Clippy, and workspace tests;
- physical 44.1/48 kHz quality, drift, acoustic latency, and long-running
  behavior remain hardware or measurement gates.

## Rollback plan

Remove the source adapter and retain the platform-neutral source contract if
quality or CPU measurements reject the windowed-sinc kernel. Another nominal
converter can replace the audio primitive without changing network, protocol,
pairing, or platform device code.
