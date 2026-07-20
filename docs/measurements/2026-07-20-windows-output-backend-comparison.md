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

## Sixty-second silent smoke

The same endpoint and fixed request were then run sequentially with
`SONIMESH_OUTPUT_SMOKE_SECONDS=60`. CPAL reported 5,999 callbacks, latest and
maximum frames of 480 and 1,056 respectively, a latest status offset of
60,074,357,200 ns, and a largest gap of 17,408,000 ns. Direct WASAPI reported
6,000 renders, latest and maximum frames of 480, 1,056 endpoint and priming
frames, a latest status offset of 60,067,263,000 ns, and a largest gap of
17,701,600 ns. Both backends reported zero capacity faults and zero stream
errors.

This is an extended silent smoke, not the required eight-hour soak. It adds no
claim about callback recovery, latency, sustained thermal behavior, or output
behavior after a device or power transition.

## Soak command check

The new backend-specific soak commands were verified sequentially on the same
endpoint with `SONIMESH_OUTPUT_SOAK_SECONDS=15` and
`SONIMESH_OUTPUT_SOAK_REPORT_SECONDS=5`. This is a command check, not a soak
result. CPAL reported three healthy control-domain snapshots at 5, 10, and 15
seconds with 499, 999, and 1,499 callbacks; the maximum callback remained
1,056 frames, the largest gap was 17,826,900 ns, and faults/errors remained
zero. Direct WASAPI reported 500, 1,000, and 1,500 renders at the same
intervals; maximum renders remained 480 frames, endpoint/priming frames were
1,056, the largest gap was 17,541,200 ns, and faults/errors remained zero.

The commands printed each observation after reading existing atomic status from
the control domain. They do not add logging, allocation, locks, IPC, or
recovery work to either audio callback. A separate uninterrupted eight-hour
run must retain every periodic line and its final status before the soak cell is
considered complete.

## Default-recovery command check

The CPAL and direct-WASAPI default-recovery commands were separately checked
with a two-second wait and two-second recovered stream, with
`SONIMESH_OUTPUT_REQUIRE_DEFAULT_CHANGE=false`. No default device was changed,
so this is not a default-role transition observation. It verifies only that each
command safely captures the active stream status, stops outside the callback,
re-probes the current default, and starts a fresh bounded stream.

CPAL reported 199 callbacks before stop and 200 after recreation, both healthy
with zero faults/errors and a 1,056-frame maximum callback. Direct WASAPI
reported 200 renders before stop and 201 after recreation, both healthy with
zero faults/errors, a 480-frame maximum render, and 1,056 endpoint/priming
frames. Both commands printed `default changed: false`, as required by this
no-change validation mode.

A valid default-role evidence row must leave the change requirement enabled,
observe `default changed: true`, and retain the two status lines after an
operator changes the Windows default render endpoint during the wait.

## Interpretation and remaining work

This comparison shows different observed callback/render quanta under the same
silent smoke configuration: CPAL reached 1,056 frames, while direct WASAPI
rendered 480-frame quanta after a 1,056-frame priming buffer. These are not
interchangeable measurements and do not establish latency, quality, device
removal behavior, or recovery compatibility.

The Windows Phase 0B matrix still requires default-device and pinned-device
removal, default-role change with explicit recreation, sleep/wake with explicit
recreation, and an eight-hour silent soak. Physical/acoustic latency remains a
separate experiment requiring a common reference.
