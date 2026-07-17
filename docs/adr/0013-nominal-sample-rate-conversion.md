# ADR 0013: Bounded nominal sample-rate conversion

- Status: Provisional
- Date: 2026-07-17

## Context

Android capture and Windows output devices may negotiate different sample rates.
The current host evidence includes both 48 kHz and 44.1 kHz endpoints. Passing
48 kHz PCM directly to a 44.1 kHz output would change playback speed and pitch.
The existing occupancy controller handles slow clock error decisions but is not
a sample-rate conversion stage.

## Decision

Add a bounded linear interpolation stage in `sonimesh-audio` and a pull-based
`SampleRateMappedRenderSource` in `sonimesh-app`.

- input and output rates are restricted to 8–192 kHz;
- one through eight interleaved channels are supported;
- the source adapter preallocates input storage before stream start;
- output is processed in chunks of at most 256 frames, keeping high conversion
  ratios from creating an unbounded callback scratch allocation;
- interpolation retains the final input frame needed by the next step and
  preserves phase across output callbacks and internal chunks;
- a separate control-domain handle can publish the controller's bounded ppm
  adjustment atomically, while hard-resync decisions remain stream-owner work;
- invalid alignment, rate, channel, and callback bounds fail closed to silence;
- the source factory already receives `OutputConfig`, so nominal conversion can
  be composed after channel mapping and before the platform stream; source
  construction errors are returned before the platform stream starts;
- the converter is a nominal-rate feasibility primitive, not a drift controller,
  quality-qualified resampler, or physical latency measurement.

## Reliability and realtime implications

The realtime adapter performs only bounded source pulls, fixed-buffer compaction,
and interpolation. It does not allocate, lock, wait, access the network, invoke
UI code, or enumerate devices from `RenderSource::render`. Underflow remains
owned by the wrapped source and produces initialized silence.

The linear filter may attenuate or alias content at difficult conversion ratios.
The final audio policy must compare a higher-quality nominal converter and the
separate bounded drift correction stage before making an audio-quality claim.

## Validation evidence

- deterministic tests cover equal-rate passthrough, upsampling interpolation,
  downsampling positions, input/output alignment, invalid bounds, callback
  tails, oversized callbacks, and phase continuity across 256-frame chunks;
- the implementation is covered by Rust fmt, Clippy, and workspace tests;
- physical 44.1/48 kHz quality, drift, acoustic latency, and long-running
  behavior remain hardware or measurement gates.

## Rollback plan

Remove the source adapter and retain the platform-neutral source contract if
quality or CPU measurements reject linear interpolation. A higher-quality
resampler can replace the audio primitive without changing network, protocol,
pairing, or platform device code.
