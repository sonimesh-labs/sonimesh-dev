# Windows output soak smoke, 2026-11-22

> Status: short same-host software scheduling observation; the eight-hour soak,
> device-event, backend-selection, and latency gates remain open.

## Environment and procedure

The Windows reference host ran five-second silent soaks against the current
default render endpoint. The runs used a one-second report interval and the
existing bounded 480-frame request with an 8,192-frame maximum:

```powershell
$env:SONIMESH_OUTPUT_SOAK_SECONDS = "5"
$env:SONIMESH_OUTPUT_SOAK_REPORT_SECONDS = "1"
cargo run --offline --locked -p sonimesh-platform --example output_soak --features cpal-output
cargo run --offline --locked -p windows-wasapi-prototype --bin wasapi_output_soak
```

The commands printed an endpoint display name during execution; it is omitted
here. The retained CPAL and direct-WASAPI transcripts were each checked with
`tools/check_output_soak_log.py` before recording these observations.

## Observations

| Field | CPAL | direct-WASAPI |
| --- | ---: | ---: |
| Configured duration | 5 s | 5 s |
| Report statuses | 5 | 5 |
| Requested frames | 480 | 480 |
| Declared maximum frames | 8,192 | 8,192 |
| Final callback/render count | 499 | 500 |
| Latest frames | 480 | 480 |
| Maximum frames | 1,056 | 480 |
| Endpoint buffer frames | not applicable | 1,056 |
| Primed silence frames | not applicable | 1,056 |
| Largest observed gap | 17.7057 ms | 18.1811 ms |
| Oversized callbacks/renders | 0 | 0 |
| Stream errors | 0 | 0 |
| Final health | Healthy | Healthy |

The checker results were:

```text
output soak log: ok backend=CPAL duration_s=5 periodic_statuses=5 final_count=499 max_frames=1056 max_gap_ns=17705700 device_selection=default
output soak log: ok backend=direct-WASAPI duration_s=5 periodic_statuses=5 final_count=500 max_frames=480 max_gap_ns=18181100 device_selection=default
```

## Interpretation and limits

Both adapters completed the configured short run and retained complete,
monotonic healthy control-domain snapshots. CPAL again observed a 1,056-frame
callback while direct WASAPI rendered 480 frames against a 1,056-frame endpoint
buffer. This is a scheduling observation, not a latency or quality result.

The run does not establish eight-hour stability, Windows 11 compatibility,
physical device removal, default-role changes, sleep/wake recovery, power
behavior, acoustic latency, or a release backend. Those gates remain open under
ADR 0003.

