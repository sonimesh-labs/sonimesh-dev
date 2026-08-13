# Windows output backend recheck - 2026-08-13

> Status: local silent-output observation only. This does not establish
> callback, output, or end-to-end latency; it does not cover a device removal,
> default-role change, sleep/wake, or an eight-hour soak.

## Purpose

Repeat the CPAL and direct-WASAPI silent smoke on the current Windows 11
reference host and repeat the CPAL smoke against a pinned Realtek endpoint, the
explicitly named remaining evidence in
[2026-07-17-windows-cpal-smoke.md](./2026-07-17-windows-cpal-smoke.md).

## Environment

| Field | Observed value |
| --- | --- |
| Local run date | 2026-08-13, Asia/Shanghai |
| Windows | Windows 11 Home (Chinese), version 10.0.26200, build 26200, 64-bit |
| Host | ASUS TUF Gaming A15 FA506QM |
| Rust | 1.85.0, `x86_64-pc-windows-msvc` |
| Backends | CPAL 0.18.1 / direct WASAPI via windows 0.62.2 |
| Default endpoint | Redacted local display name, 48,000 Hz, 8 channels, Float32, 480-frame reported range |
| Realtek endpoint | Redacted local display name, 48,000 Hz, 2 channels, Float32, 512-frame reported range |
| Requested frames | 480 for all runs; neither backend used its fallback request |
| Maximum source capacity | 8,192 frames for all runs |

Opaque endpoint identifiers and full display names are intentionally omitted.

## Default-endpoint two-second silent smoke

The CPAL and direct-WASAPI smoke commands ran sequentially against the current
default render endpoint on one local boot.

| Field | CPAL | Direct WASAPI |
| --- | ---: | ---: |
| Callback/render count | 199 callbacks | 200 renders |
| Latest observed frames | 480 | 480 |
| Largest observed frames | 1,056 | 480 |
| Endpoint buffer frames | Not exposed by this CPAL smoke | 1,056 |
| Primed silence frames | n/a | 1,056 |
| Latest local offset | 2,092,347,000 ns | 2,078,474,800 ns |
| Largest local scheduling gap | 18,864,100 ns | 17,153,700 ns |
| Capacity faults | 0 | 0 |
| Stream errors | 0 | 0 |

The offsets and gaps are separate backend-local software observations. They do
not share a clock or demonstrate physical latency.

## Realtek pinned-endpoint two-second silent smoke

The CPAL smoke now accepts `SONIMESH_OUTPUT_SMOKE_DEVICE_ID`; an unset variable
selects the current default render endpoint. With the Realtek endpoint pinned,
the smoke completed with:

| Field | Observed value |
| --- | ---: |
| Callback count | 188 |
| Latest observed frames | 512 |
| Largest observed frames | 1,126 |
| Latest local offset | 3,315,979,900 ns |
| Largest local scheduling gap | 11,127,300 ns |
| Capacity faults | 0 |
| Stream errors | 0 |

This is the first CPAL callback-size observation on a 512-frame reported
endpoint. The Realtek endpoint again delivered a callback larger than both the
480-frame request and its 512-frame reported range (1,126 frames), confirming
on a second endpoint the separation between a fixed request and the
preallocated maximum callback bound. The larger local offset is backend-local
software timing and is not a latency result.

## Pinned-device recovery command check

The CPAL-only `output_pinned_recovery` command was checked against the pinned
Realtek endpoint with a two-second event wait and a two-second recovered
stream. No device was unplugged, disabled, or otherwise changed, so this is not
pinned-device removal evidence and the identifier is intentionally omitted.

The initial stream reported 188 callbacks, a 512-frame latest callback, a
1,126-frame maximum callback, an 11,073,800 ns maximum scheduling gap, and zero
capacity faults or stream errors. After explicit control-domain stop, a fresh
probe found the same pinned endpoint and the recovered stream reported 188
callbacks, a 512-frame latest callback, a 1,126-frame maximum callback, an
11,112,600 ns maximum scheduling gap, and zero capacity faults or stream
errors. The command printed `pre_stop_healthy=true` and
`recovery_result=pinned_device_restarted`.

This validates only the command's fixed selection, redacted reporting, explicit
stop, re-probe, and fresh-start path against a non-default endpoint. A future
removal run must retain the old stream's status after the physical event.

## Direct-WASAPI pinned-endpoint two-second silent smoke

The direct-WASAPI prototype smoke now accepts the same
`SONIMESH_OUTPUT_SMOKE_DEVICE_ID` identifier as the CPAL smoke, with or without
the `wasapi:` prefix, and resolves it among the active render endpoints. With
the Realtek endpoint pinned, the smoke completed with:

| Field | Observed value |
| --- | ---: |
| Render count | 189 |
| Latest observed frames | 512 |
| Maximum observed frames | 512 |
| Endpoint buffer frames | 1,126 |
| Primed silence frames | 1,126 |
| Latest local offset | 3,239,035,600 ns |
| Largest local scheduling gap | 10,926,500 ns |
| Capacity faults | 0 |
| Stream errors | 0 |

Direct WASAPI on the Realtek endpoint therefore rendered 512-frame quanta after
a 1,126-frame priming buffer, matching the endpoint's 512-frame reported range.
An unknown pinned identifier was also rejected with `DeviceUnavailable` before
any stream started, confirming the selection fails closed.

## Sixty-second default-endpoint silent smoke repeat

Both backends repeated the default-endpoint silent smoke for sixty seconds on
the same boot. CPAL reported 5,999 callbacks, 480-frame latest and 1,056-frame
maximum callbacks, a 60,064,738,600 ns latest local offset, a 17,396,800 ns
largest scheduling gap, and zero capacity faults or stream errors. Direct WASAPI
reported 6,000 renders, 480-frame latest and maximum renders, a 1,056-frame
endpoint and priming buffer, a 60,068,546,700 ns latest local offset, a
17,432,500 ns largest scheduling gap, and zero capacity faults or stream
errors.

These repeat the same 60-second observations recorded on 2026-07-20 on this
host. This is an extended silent repeat, not the required eight-hour soak.

## Interpretation and remaining work

The default-endpoint runs reproduce the known backend difference: CPAL received
a 1,056-frame callback under a 480-frame request while direct WASAPI rendered
480-frame quanta after a 1,056-frame priming buffer. The pinned Realtek runs
add the first symmetric two-backend comparison on a 512-frame reported endpoint:
CPAL observed a 1,126-frame maximum callback while direct WASAPI rendered
512-frame quanta after a 1,126-frame priming buffer. CPAL's largest callback
therefore matched the direct-WASAPI endpoint buffer on both endpoints. All
silent runs stayed inside the 8,192-frame source bound with zero capacity
faults and zero stream errors.

The Phase 0B evidence still needs an observed default-role change, current
default and pinned-device removal, sleep/wake followed by explicit recreation,
and one uninterrupted eight-hour silent soak for each backend. A common
physical/acoustic reference remains required for any latency report.
