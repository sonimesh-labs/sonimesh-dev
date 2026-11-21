# Windows output soak, 30 seconds, 2026-11-22

> Status: same-host software scheduling observation; the eight-hour soak,
> device-event, backend-selection, and latency gates remain open.

## Environment and procedure

The Windows reference host ran 30-second silent soaks against the current
default render endpoint. Both backends used a five-second report interval, the
bounded 480-frame request, and an 8,192-frame maximum:

```powershell
$env:CARGO_HOME = Join-Path $env:TEMP 'sonimesh-cargo-home'
$env:CMAKE_POLICY_VERSION_MINIMUM = '3.5'
$env:SONIMESH_OUTPUT_SOAK_SECONDS = '30'
$env:SONIMESH_OUTPUT_SOAK_REPORT_SECONDS = '5'
cargo run --offline --locked -p sonimesh-platform --example output_soak --features cpal-output
cargo run --offline --locked -p windows-wasapi-prototype --bin wasapi_output_soak
```

The commands printed an endpoint display name during execution; it is omitted
here. Each transcript was checked with `tools/check_output_soak_log.py`.

## Observations

| Field | CPAL | direct-WASAPI |
| --- | ---: | ---: |
| Configured duration | 30 s | 30 s |
| Report statuses | 6 | 6 |
| Requested frames | 480 | 480 |
| Declared maximum frames | 8,192 | 8,192 |
| Final callback/render count | 2,999 | 3,000 |
| Latest frames | 480 | 480 |
| Maximum frames | 1,056 | 480 |
| Endpoint buffer frames | not applicable | 1,056 |
| Primed silence frames | not applicable | 1,056 |
| Largest observed gap | 18.3484 ms | 17.8713 ms |
| Oversized callbacks/renders | 0 | 0 |
| Stream errors | 0 | 0 |
| Final health | Healthy | Healthy |

The checker results were:

```text
output soak log: ok backend=CPAL duration_s=30 periodic_statuses=6 final_count=2999 max_frames=1056 max_gap_ns=18348400 device_selection=default
output soak log: ok backend=direct-WASAPI duration_s=30 periodic_statuses=6 final_count=3000 max_frames=480 max_gap_ns=17871300 device_selection=default
```

## Interpretation and limits

Both adapters completed the half-minute run with monotonic healthy snapshots,
zero stream errors, and no oversized callback/render observations. CPAL again
reported a 1,056-frame callback while direct WASAPI rendered 480 frames against
a 1,056-frame endpoint buffer. The longer run remains a scheduling observation,
not a latency or quality result.

This evidence does not establish eight-hour stability, Windows 11 compatibility,
physical device removal, default-role changes, sleep/wake recovery, power
behavior, acoustic latency, or a release backend. Those gates remain open under
ADR 0003.
