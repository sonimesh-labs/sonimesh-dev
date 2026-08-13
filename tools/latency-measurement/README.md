# SoniMesh latency measurement tool

Deterministic test-signal generation and correlation-based latency offset
estimation for the SoniMesh acoustic timing workflow described in
[`docs/testing-and-measurement.md`](../../docs/testing-and-measurement.md).

The tool is intentionally dependency-free (standard library only).

## Build and run

```sh
cargo run -p sonimesh-latency-measurement -- --help
```

## Commands

### generate

Write a deterministic MLS reference WAVE file:

```sh
cargo run -p sonimesh-latency-measurement -- generate \
  --sample-rate 48000 --seconds 10 --seed 7 reference.wav
```

The reference is one full period of a degree-17 maximal-length sequence
(`x^17 + x^3 + 1`, period 131071 samples, about 2.7 seconds at 48 kHz) followed
by silence, normalized to ±1.0 and stored as 16-bit PCM by default.

`--bits` selects the WAVE sample layout the generator writes: `16`, `24`, `32`
(signed PCM), or `f32`, `f64` (IEEE float). The reader accepts every format the
generator writes.

The generator rejects sample rates outside 8–192 kHz, durations above one hour,
and combinations exceeding 115,200,000 samples before allocating any signal.

### measure

Estimate the offset between a reference and one or more recordings:

```sh
cargo run -p sonimesh-latency-measurement -- measure \
  --max-search-ms 1000 reference.wav capture1.wav capture2.wav
```

The reference and each recording must share the same sample rate. The reader
accepts PCM 16/24/32-bit and IEEE float 32/64-bit WAVE files, including
WAVE_FORMAT_EXTENSIBLE variants. Each run is printed as `offset_ms=...`, and
with more than one recording a `min/p50/p95/p99/max` summary is printed.

### info

Report the declared properties of one or more WAVE files without decoding
their samples:

```sh
cargo run -p sonimesh-latency-measurement -- info capture1.wav capture2.wav
```

Each file prints one `key=value` line with `file`, `sample_rate_hz`, `channels`,
`bits_per_sample`, `audio_format`, `frames`, and `duration_ms`. `audio_format`
is the declared WAVE tag (`1` = PCM, `3` = IEEE float;
WAVE_FORMAT_EXTENSIBLE is resolved to its subformat tag). Run this before
`measure` to confirm recordings share the sample rate and use a supported
layout. A file that cannot be read is reported on stderr while the remaining
files are still processed; the command exits non-zero if any file failed.

### summary

Summarize offsets read from stdin, one number per line:

```sh
cargo run -p sonimesh-latency-measurement -- summary
```

Each line must be a finite number; NaN and infinite values are rejected.

## Algorithm

`estimate_offset_ms` correlates the leading probe of the reference (default
32768 samples) against the recording over a bounded search window
(`--max-search-ms`):

1. Coarse pass: both signals are boxcar-downsampled by `--coarse-factor`
   (default 16) and correlated to localize the peak.
2. Fine pass: full-resolution correlation over `±2 × coarse-factor` samples
   around the coarse peak finds the integer lag.
3. A parabolic fit around the integer peak gives a sub-sample refinement.
4. The offset is accepted only when the best correlation score is at least
   `--distinctness-ratio` (default 3.0) times the best score away from the
   peak.

## Limitations

- This tool computes a software correlation offset between two recordings. It
  does not prove physical end-to-end latency; the acoustic setup (common audio
  interface or calibrated speakers and microphone) determines what the offset
  means.
- The recording must start before the reference arrives and must contain the
  reference signal within the search window.
- The estimate is sample-quantized (about 0.02 ms at 48 kHz) plus a parabolic
  sub-sample refinement; it is a measurement aid, not a certified instrument.
- Report metadata (device models, Windows version, AP, network band, sample
  rate, buffer mode, frame size, Opus bitrate, jitter mode, limiter
  configuration) belongs in the measurement report per
  `docs/testing-and-measurement.md`, not in this tool.