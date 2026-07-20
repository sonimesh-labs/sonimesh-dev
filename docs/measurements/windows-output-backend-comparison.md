# Windows output backend comparison protocol

> Status: measurement protocol only; no direct-WASAPI observation is recorded yet.
> Updated: 2026-07-19

## Purpose

Compare the CPAL shared-mode feasibility adapter and the independent direct-WASAPI
shared-mode prototype without treating either backend's requested buffer size,
software scheduling observations, or endpoint-reported range as a latency claim.

The direct prototype lives in tools/windows-wasapi-prototype. It implements only
the platform output contracts and does not import CPAL code. Its use is
constrained by ADR 0003.

## Controlled configuration

Run both commands on the same Windows boot, default render endpoint, endpoint mix
format, requested duration, and bounded maximum. Do not compare a CPAL result
against a different endpoint, role, format, or callback request.

    cargo run -p sonimesh-platform --example output_smoke --features cpal-output
    cargo run -p windows-wasapi-prototype --bin wasapi_silence_smoke

Both commands default to two seconds. Set the same bounded duration for an
explicit long silent run; it changes only the control-domain wait interval:

    $env:SONIMESH_OUTPUT_SMOKE_SECONDS = "900"
    cargo run -p sonimesh-platform --example output_smoke --features cpal-output
    cargo run -p windows-wasapi-prototype --bin wasapi_silence_smoke

The accepted range is 1 through 28,800 seconds. Record the selected duration
and do not compare runs with different durations as equivalent scheduling data.

For the eight-hour silent soak, use the matching backend-specific commands.
They default to 28,800 seconds and emit a control-domain status snapshot every
60 seconds. The interval may be shortened for a command check, but only an
eight-hour result satisfies the soak cell:

    cargo run -p sonimesh-platform --example output_soak --features cpal-output
    cargo run -p windows-wasapi-prototype --bin wasapi_output_soak

Set `SONIMESH_OUTPUT_SOAK_SECONDS` to 1 through 28,800 and
`SONIMESH_OUTPUT_SOAK_REPORT_SECONDS` to a value no larger than that duration
only when the recorded experiment requires a different configuration. Preserve
every periodic status line and the final status in the measurement record.

For the repeated start/stop baseline, run:

    cargo run -p sonimesh-platform --example output_lifecycle_smoke --features cpal-output
    cargo run -p windows-wasapi-prototype --bin wasapi_lifecycle_smoke

The CPAL smoke accepts a 480-frame request and falls back explicitly to the
backend default when that request is rejected. The direct prototype follows the
same policy. Record which path actually started; a fallback result is not
like-for-like with a fixed-request result.

## Required record for one comparison row

| Field | CPAL | Direct WASAPI | Notes |
| --- | --- | --- | --- |
| Date and local time | — | — | Do not include stable endpoint IDs. |
| Windows edition/build | — | — | Include architecture. |
| Rust and binding versions | — | — | CPAL and windows crate versions. |
| Endpoint category | — | — | Redact local display name if exported. |
| Mix/sample rate, channels, representation | — | — | Must be identical for direct comparison. |
| Requested frames or backend default | — | — | Record fallback explicitly. |
| Configured maximum frames | — | — | Independent memory guard. |
| Backend endpoint buffer frames | — | — | CPAL range is not an observed callback size. |
| Render/callback count | — | — | Two-second smoke only unless duration changes. |
| Latest observed frames | — | — | Actual worker quantum. |
| Largest observed frames | — | — | Actual worker quantum. |
| Latest callback/render offset | — | — | Same-clock offset from backend status creation; software timeline only. |
| Largest observed scheduling gap | — | — | Same-clock software observation only. |
| Priming silence frames | n/a | — | Direct-WASAPI initialization detail. |
| Capacity faults | — | — | Must remain zero for a successful smoke. |
| Stream errors | — | — | Must remain zero for a successful smoke. |
| Soak periodic snapshots | — | — | Required for an eight-hour run; control-domain atomic observations only. |

## Lifecycle extensions

Use the same row shape for these separate scenarios. Each is a lifecycle fact,
not a latency qualification:

- current-default endpoint unplugged while rendering;
- default-role change while rendering, then stop and recreation;
- sleep/wake followed by explicit recreation;
- repeated start/stop cycles; and
- long silent output with callback/render gaps and stream errors retained.

The direct prototype only targets the current default endpoint. If it changes
between enumeration and start, record the resulting unavailable-device failure
and repeat after refreshing control-domain state. It must not silently bind a
stale opaque identifier to a different endpoint.

## Interpretation boundary

This template contains no results. Complete rows establish only observed stream
lifecycle and software scheduling facts. A physical/acoustic procedure with a
common reference is still required before reporting end-to-end or output latency.
