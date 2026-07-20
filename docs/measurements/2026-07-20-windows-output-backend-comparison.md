# Windows output backend comparison

> Result: silent-output and repeated-start/stop observations only. This record
> does not establish output or end-to-end latency.
>
> Recorded: 2026-07-20 12:04 +08:00

## Scope

This run follows the [Windows output backend comparison protocol](./windows-output-backend-comparison.md).
The CPAL feasibility adapter and independent direct-WASAPI prototype ran
sequentially during the same local boot against the current default render
endpoint. Both commands rendered silence for two seconds, then each lifecycle
command completed five 250 ms start/stop cycles.

The host reported `Windows 10 Home China`, version `2009`, build `26200`, x64.
This is not the Windows 11 reference baseline. The endpoint's display name and
opaque identifier are deliberately omitted from this record.

## Controlled configuration

| Field | CPAL | Direct WASAPI | Notes |
| --- | --- | --- | --- |
| Rust and binding versions | CPAL 0.18.1 | windows 0.62.2 | Workspace Rust 1.85 toolchain. |
| Endpoint category | Current default render endpoint | Current default render endpoint | Runs were sequential on the same local boot. |
| Mix/sample rate, channels, representation | 48,000 Hz, 8 channels, Float32 | 48,000 Hz, 8 channels, Float32 | Matching reported formats. |
| Requested frames | 480 | 480 | Both fixed requests started; no fallback. |
| Configured maximum frames | 8,192 | 8,192 | Independent source-capacity guard. |

## Two-second silent smoke

| Field | CPAL | Direct WASAPI | Notes |
| --- | ---: | ---: | --- |
| Backend endpoint buffer frames | Not exposed by CPAL smoke | 1,056 | CPAL callback maximum is not an endpoint buffer report. |
| Render/callback count | 199 callbacks | 200 renders | Observed after stream start. |
| Latest observed frames | 480 | 480 | Actual final worker quantum. |
| Largest observed frames | 1,056 | 480 | CPAL exceeded the fixed request without reaching capacity. |
| Latest callback/render offset | 2,110,685,000 ns | 2,070,045,900 ns | Separate backend-local software clocks. |
| Largest observed scheduling gap | 18,286,700 ns | 17,666,100 ns | Software scheduling observation only. |
| Priming silence frames | n/a | 1,056 | Direct-WASAPI initialization detail. |
| Capacity faults | 0 | 0 | Both remained within the 8,192-frame bound. |
| Stream errors | 0 | 0 | No asynchronous backend error observed. |

## Repeated start/stop smoke

Both lifecycle commands completed five cycles with fixed 480-frame requests.
Every cycle reported a healthy backend, zero capacity faults, and zero stream
errors. CPAL observed 24-25 callbacks per cycle with a 1,056-frame maximum;
direct-WASAPI observed 25 renders per cycle with a 480-frame maximum and 1,056
priming frames. The first-cycle largest software gaps were 17,207,500 ns
(CPAL) and 17,557,700 ns (direct WASAPI); later cycles ranged from 10,093,000
to 10,463,600 ns for CPAL and 10,073,900 to 10,401,300 ns for direct WASAPI.

## Interpretation and remaining work

This comparison shows different observed callback/render quanta under the same
silent smoke configuration: CPAL reached 1,056 frames, while direct WASAPI
rendered 480-frame quanta after a 1,056-frame priming buffer. These are not
interchangeable measurements and do not establish latency, quality, device
removal behavior, or recovery compatibility.

The Windows Phase 0B matrix still requires default-device and pinned-device
removal, default-role change with explicit recreation, sleep/wake with explicit
recreation, and a documented long-duration silent run. Physical/acoustic
latency remains a separate experiment requiring a common reference.
