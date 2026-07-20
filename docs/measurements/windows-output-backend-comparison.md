# Windows output backend comparison protocol

> Status: measurement protocol with dated local observations in
> `2026-07-20-windows-output-backend-comparison.md` and
> `2026-07-20-windows-output-backend-recheck.md`; Phase 0B remains incomplete.
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
    cargo run -p windows-wasapi-prototype --bin windows-wasapi-prototype

Both commands default to two seconds. Set the same bounded duration for an
explicit long silent run; it changes only the control-domain wait interval:

    $env:SONIMESH_OUTPUT_SMOKE_SECONDS = "900"
    cargo run -p sonimesh-platform --example output_smoke --features cpal-output
    cargo run -p windows-wasapi-prototype --bin windows-wasapi-prototype

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
After a completed run, verify the retained transcript before deriving a
measurement row:

    python tools/check_output_soak_log.py --backend cpal output/cpal-output-soak.log
    python tools/check_output_soak_log.py --backend wasapi output/wasapi-output-soak.log

The verifier checks the command's declared duration and interval, the expected
number of periodic snapshots, monotonic callback/render and local-offset
observations, zero capacity faults/errors, and one final healthy status. It
does not inspect the callback, establish physical latency, or substitute for a
manual device, default-role, or power-transition scenario.

For the repeated start/stop baseline, run:

    cargo run -p sonimesh-platform --example output_lifecycle_smoke --features cpal-output
    cargo run -p windows-wasapi-prototype --bin wasapi_lifecycle_smoke

Both commands default to five cycles with a 250 ms active stream. For an
explicit lifecycle experiment, set the same bounded values for both backends:

    $env:SONIMESH_OUTPUT_LIFECYCLE_CYCLES = "500"
    $env:SONIMESH_OUTPUT_LIFECYCLE_ACTIVE_MILLIS = "250"
    cargo run -p sonimesh-platform --example output_lifecycle_smoke --features cpal-output
    cargo run -p windows-wasapi-prototype --bin wasapi_lifecycle_smoke

`SONIMESH_OUTPUT_LIFECYCLE_CYCLES` accepts 1 through 500 and
`SONIMESH_OUTPUT_LIFECYCLE_ACTIVE_MILLIS` accepts 1 through 1,000. Retain every
cycle line or a redacted aggregation derived from every line. This exercises
only repeated output start/stop in the control domain. It is not a network
connection-loop result, device-removal evidence, or sleep/wake recovery.

For a manual default-role-change and explicit recreation experiment, run one
backend at a time, switch the Windows default render endpoint during the wait,
then retain both the pre-stop and recovered status lines:

    cargo run -p sonimesh-platform --example output_default_recovery --features cpal-output
    cargo run -p windows-wasapi-prototype --bin wasapi_default_recovery

The commands require an endpoint change by default. They accept
`SONIMESH_OUTPUT_DEFAULT_CHANGE_WAIT_SECONDS` from 1 through 600 and
`SONIMESH_OUTPUT_RECOVERY_SECONDS` from 1 through 60. Setting
`SONIMESH_OUTPUT_REQUIRE_DEFAULT_CHANGE=false` is only for command validation;
it must never be recorded as a successful default-role transition.

For a CPAL pinned-device removal experiment, first obtain the opaque identifier
from the local `output_probe` command. Do not paste that identifier into an
exported record. Start the dedicated command with the identifier only in the
local environment:

    $env:SONIMESH_OUTPUT_PINNED_DEVICE_ID = "<local opaque device id>"
    cargo run -p sonimesh-platform --example output_pinned_recovery --features cpal-output

The command starts only the selected device, records the pre-stop status, stops
outside the callback, and probes that same ID again. It prints either
`recovery_result=pinned_device_unavailable` or a status for one fresh bounded
start. A fault on the old stream is an observation, not a command failure; only
the fresh stream must be healthy for the restarted result. The unavailable
result is also an observation and is not a command failure; retain either result
with the physical removal event and any old-stream error.

## Manual device and power recovery procedure

Run one backend at a time. These scenarios require a physical endpoint or a
real Windows power transition, so the commands are intentionally not presented
as automatic tests. The existing default-recovery command provides the bounded
control-domain sequence; set `SONIMESH_OUTPUT_REQUIRE_DEFAULT_CHANGE=false`
only because these scenarios do not require a default-role change, not to claim
that no event occurred.

1. Record the selected backend, endpoint category, current format, request,
   source bound, Windows build, and the exact physical event to be performed.
2. Start the command and wait until its stream is active. For CPAL pinned-device
   removal, use `output_pinned_recovery` with an enumerated device ID; retain
   that ID only in the non-exported lab transcript.
3. During the wait, unplug or disable the chosen endpoint, or put the host to
   sleep and wake it. Do not alter callback code or invoke recovery from an
   audio thread.
4. Retain the pre-stop atomic status and output health, then let the command
   stop the old stream in the control domain, re-probe the current default, and
   attempt one fresh bounded start.
5. Record whether the old stream reported an error, whether the endpoint/default
   changed, the fresh-start result, and the recovered status. A continued old
   stream is an observation, not proof that removal or sleep/wake is supported.

The direct-WASAPI prototype covers only the current-default endpoint. It must
record `DeviceUnavailable` if its previously selected identifier is stale; it
must not use that result as pinned-device evidence. A failed re-probe or fresh
start is a valid negative result and must remain in the measurement record.

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
- CPAL pinned endpoint unplugged while rendering;
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
