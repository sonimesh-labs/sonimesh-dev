# Windows output backend recheck - 2026-07-20

> Status: local silent-output and repeated start/stop observation only. This
> does not establish callback, output, or end-to-end latency; it does not cover
> a device removal, default-role change, sleep/wake, or an eight-hour soak.

## Environment

| Field | Observed value |
| --- | --- |
| Local run date | 2026-07-20, +08:00 |
| Windows | Windows 11 Home China, version 10.0.26200, build 26200, 64-bit |
| Rust | 1.85.0, `x86_64-pc-windows-msvc` |
| Backends | CPAL 0.18.1 / direct WASAPI via windows 0.62.2 |
| Endpoint category | Current default render endpoint, redacted local display name and identifier |
| Mix format | 48,000 Hz, 8 channels, Float32 for both runs |
| Requested frames | 480 for both runs; neither backend used its fallback request |
| Maximum source capacity | 8,192 frames for both runs |

The commands ran sequentially during one local boot. The direct-WASAPI smoke
used `--bin windows-wasapi-prototype`, the actual default binary name.

## Sixty-second silent smoke

| Field | CPAL | Direct WASAPI |
| --- | ---: | ---: |
| Active duration | 60 s | 60 s |
| Callback/render count | 5,999 callbacks | 6,000 renders |
| Latest observed frames | 480 | 480 |
| Largest observed frames | 1,056 | 480 |
| Endpoint buffer frames | Not exposed by this CPAL smoke | 1,056 |
| Primed silence frames | n/a | 1,056 |
| Latest local offset | 60,079,376,600 ns | 60,068,431,200 ns |
| Largest local scheduling gap | 17,435,700 ns | 17,733,200 ns |
| Capacity faults | 0 | 0 |
| Stream errors | 0 | 0 |

The offsets and gaps are separate backend-local software observations. They do
not share a clock or demonstrate physical latency.

## Repeated start/stop baseline

Each backend completed five sequential 250 ms active cycles with its 480-frame
request. Every cycle remained `Healthy`, had zero capacity faults and stream
errors, and observed a 480-frame latest quantum.

| Field | CPAL | Direct WASAPI |
| --- | --- | --- |
| Callback/render count by cycle | 25, 25, 25, 25, 25 | 25, 25, 26, 25, 25 |
| Largest observed frames by cycle | 1,056 in every cycle | 480 in every cycle |
| Largest scheduling gap range | 10,135,200-10,301,400 ns | 10,073,100-17,827,700 ns |
| Endpoint/priming frames | n/a | 1,056 / 1,056 in every cycle |

## Interpretation and remaining work

This recheck repeats the known fact that CPAL can receive a 1,056-frame
callback despite the same 480-frame request used by direct WASAPI, whose
post-priming render quantum stayed at 480 frames. Both paths stayed inside the
8,192-frame source bound for these silent runs.

The Phase 0B evidence still needs an observed default-role change, current
default and pinned-device removal, sleep/wake followed by explicit recreation,
and one uninterrupted eight-hour silent soak for each backend. A common
physical/acoustic reference remains required for any latency report.
