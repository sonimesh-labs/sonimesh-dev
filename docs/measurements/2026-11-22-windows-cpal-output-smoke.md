# Windows CPAL output smoke, 2026-11-22

> Status: same-host software scheduling observation; output backend and physical
> latency gates remain open.

## Environment and procedure

The Windows reference host ran the read-only CPAL/WASAPI output probe followed by
the two-second default-device silent smoke:

```powershell
cargo run -p sonimesh-platform --example output_probe --features cpal-output
cargo run -p sonimesh-platform --example output_smoke --features cpal-output
```

The run used CPAL 0.18.2 through the Windows WASAPI host. Device identifiers and
display names were intentionally omitted from this record.

## Observations

The probe enumerated five output endpoints. Four reported 48 kHz stereo Float32
defaults; one reported 44.1 kHz stereo Float32. The selected default endpoint
reported 48 kHz, eight channels, and a fixed 480-frame callback range.

| Field | Observed value |
| --- | ---: |
| Active duration | 2 s |
| Requested callback frames | 480 |
| Declared maximum callback frames | 8,192 |
| Requested channels | 8 |
| Callback count | 199 |
| Latest callback frames | 480 |
| Maximum callback frames | 1,056 |
| Latest interleaved samples | 3,840 |
| Maximum interleaved samples | 8,448 |
| Latest callback offset | 2.1102437 s |
| Largest callback gap | 19.5352 ms |
| Oversized callbacks | 0 |
| Stream errors | 0 |

## Interpretation and limits

This run shows that the current CPAL adapter can open the host's default output,
render bounded silence for two seconds, and expose callback observations without a
reported capacity fault or stream error. The observed 1,056-frame callback is
within the declared 8,192-frame source bound but exceeds the fixed 480-frame
request, matching the adapter's documented fallback behavior.

It does not select CPAL as the release backend and does not establish callback
latency, acoustic latency, device removal, default-role change, sleep/wake
recovery, power behavior, or Windows 11 compatibility. A like-for-like direct
WASAPI comparison and longer/lifecycle runs remain required by ADR 0003.
