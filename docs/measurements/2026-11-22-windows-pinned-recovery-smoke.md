# Windows pinned output recovery smoke, 2026-11-22

> Status: same-host control-domain restart observation; physical device-removal,
> default-role, sleep/wake, backend-selection, and latency gates remain open.

## Environment and procedure

The Windows reference host ran the CPAL pinned-device recovery experiment against
the current default endpoint. The run used a one-second pre-stop wait and a
two-second recovered active interval:

```powershell
$env:SONIMESH_OUTPUT_PINNED_DEVICE_ID = "<opaque endpoint id>"
$env:SONIMESH_OUTPUT_DEVICE_EVENT_WAIT_SECONDS = "1"
$env:SONIMESH_OUTPUT_RECOVERY_SECONDS = "2"
cargo run --offline --locked -p sonimesh-platform --example output_pinned_recovery --features cpal-output
```

The endpoint identifier and display name were intentionally omitted from this
record. The retained transcript was checked with
`tools/check_output_recovery_log.py` using the `pinned` CPAL scenario.

## Observations

| Field | Before stop | Recovered |
| --- | ---: | ---: |
| Format | 48,000 Hz, 8 channels, Float32 | 48,000 Hz, 8 channels, Float32 |
| Requested frames | 480 | 480 |
| Callback count | 99 | 200 |
| Latest callback frames | 480 | 480 |
| Maximum callback frames | 1,056 | 1,056 |
| Largest observed gap | 17.8446 ms | 10.4910 ms |
| Oversized callbacks | 0 | 0 |
| Stream errors | 0 | 0 |
| Health | Healthy | Healthy |

The experiment also retained `pre_stop_healthy=true` and
`recovery_result=pinned_device_restarted`. The checker result was:

```text
output recovery log: ok scenario=pinned backend=cpal
```

## Interpretation and limits

The CPAL control domain stopped the pinned stream, re-probed the same endpoint,
and started a fresh bounded stream with healthy callback observations. This is
useful evidence for explicit stop/restart ownership and bounded status reporting.

The run did not remove or disable the endpoint, change the Windows default role,
exercise sleep/wake, or observe a callback-thread recovery. It therefore does not
close the physical lifecycle or latency gates in ADR 0003.

