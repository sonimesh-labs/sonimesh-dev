# Windows output backend recheck - 2026-08-13

> Status: local silent-output observation only. This does not establish
> callback, output, or end-to-end latency; it does not cover a device removal,
> default-role change, sleep/wake, or an eight-hour soak.

## Purpose

Repeat the CPAL and direct-WASAPI silent smoke on the current Windows 11
reference host and repeat the CPAL smoke against a pinned Realtek endpoint, the
explicitly named remaining evidence in
[2026-07-17-windows-cpal-smoke.md](./2026-07-17-windows-cpal-smoke.md). The
runs also extend the matrix to a pinned virtual endpoint whose shared-mode
default format is 44,100 Hz, the first non-48 kHz endpoint on this host.

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
| Virtual 44.1 kHz endpoint | Redacted local display name, 44,100 Hz, 2 channels, Float32, 441-frame reported range |
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

## Repeated lifecycle smoke (both backends, default and pinned Realtek)

The CPAL lifecycle smoke (`output_lifecycle_smoke`) and the direct-WASAPI
lifecycle smoke (`wasapi_lifecycle_smoke`) both accept
`SONIMESH_OUTPUT_LIFECYCLE_DEVICE_ID`; an unset variable selects the current
default render endpoint. Five 250 ms start/stop cycles ran against the default
endpoint and against the pinned Realtek endpoint on the same boot for each
backend. Every cycle reported a healthy stream with a 480-frame request and
zero capacity faults or stream errors.

| Field | CPAL default | WASAPI default | CPAL pinned | WASAPI pinned |
| --- | ---: | ---: | ---: | ---: |
| Cycles | 5 | 5 | 5 | 5 |
| Active duration per cycle | 250 ms | 250 ms | 250 ms | 250 ms |
| Device selection | default | default | pinned | pinned |
| Format | 48 kHz, 8 ch, F32 | 48 kHz, 8 ch, F32 | 48 kHz, 2 ch, F32 | 48 kHz, 2 ch, F32 |
| Latest observed frames | 480 | 480 | 512 | 512 |
| Largest observed frames | 1,056 | 480 | 1,126 | 512 |
| Primed/endpoint buffer | n/a | 1,056 | n/a | 1,126 |
| Oversized | 0 | 0 | 0 | 0 |
| Stream errors | 0 | 0 | 0 | 0 |

All four retained transcripts validate with `check_output_lifecycle_log.py`
(`--backend cpal` for the CPAL runs, `--backend wasapi` for the direct-WASAPI
runs) and record the exercised `device_selection` header. An unknown pinned
identifier was rejected with `DeviceUnavailable` before the first cycle on the
CPAL path, confirming the selection fails closed. The lifecycle runs repeat the
known backend difference across start/stop boundaries: CPAL received a
1,056/1,126-frame callback under a 480-frame request while direct WASAPI
rendered 480/512-frame quanta after a matching priming buffer. These are
backend-local software observations; they do not cover device removal,
default-role change, sleep/wake, or a long soak.

## Sixty-second pinned-Realtek silent smoke repeat

Both backends repeated the silent smoke for sixty seconds against the pinned
Realtek endpoint on the same boot. With a 512-frame render quantum, CPAL
reported 5,626 callbacks, 512-frame latest and 1,126-frame maximum callbacks,
a 61,226,803,700 ns latest local offset, an 11,334,900 ns largest scheduling
gap, and zero capacity faults or stream errors. Direct WASAPI reported 5,626
renders, 512-frame latest and maximum renders, a 1,126-frame endpoint and
priming buffer, a 61,253,429,700 ns latest local offset, a 17,304,600 ns
largest scheduling gap, and zero capacity faults or stream errors.

This extends the pinned-Realtek observation from the two-second smoke to a
sixty-second window on both backends. It is a silent repeat, not the required
eight-hour soak.

## Virtual 44.1 kHz endpoint silent smokes

The two-second, sixty-second, and lifecycle smokes were repeated against a
pinned virtual endpoint whose shared-mode default format is 44,100 Hz, 2
channels, Float32, with a 441-frame reported range. This is the first
non-48 kHz endpoint in the output evidence on this host. The fixed 480-frame
request was accepted by both backends, so neither backend used its fallback
request path.

### Sixty-second silent smoke (both backends)

| Field | CPAL | Direct WASAPI |
| --- | ---: | ---: |
| Callback/render count | 5,999 callbacks | 6,001 renders |
| Latest observed frames | 441 | 441 |
| Largest observed frames | 970 | 441 |
| Endpoint buffer frames | Not exposed by this CPAL smoke | 970 |
| Primed silence frames | n/a | 970 |
| Latest local offset | 60,123,783,900 ns | 60,065,154,100 ns |
| Largest local scheduling gap | 20,624,100 ns | 15,379,100 ns |
| Capacity faults | 0 | 0 |
| Stream errors | 0 | 0 |

Both backends rendered 441-frame quanta at 44,100 Hz (about 100 renders per
second) with zero faults or errors over sixty seconds. CPAL's largest callback
was 970 frames, which again equals the direct-WASAPI endpoint buffer size on
the same endpoint, matching the pattern seen on the 48 kHz default and Realtek
endpoints: CPAL's maximum callback tracks the endpoint buffer while direct
WASAPI renders the reported quantum after priming that buffer. The offsets and
gaps are backend-local software timing and are not latency results.

### Repeated lifecycle smoke (both backends)

Five 250 ms start/stop cycles ran against the pinned 44.1 kHz endpoint on the
same boot for each backend. Every cycle reported a healthy stream with a
480-frame request and zero capacity faults or stream errors.

| Field | CPAL | Direct WASAPI |
| --- | ---: | ---: |
| Cycles | 5 | 5 |
| Device selection | pinned | pinned |
| Format | 44.1 kHz, 2 ch, F32 | 44.1 kHz, 2 ch, Float32 |
| Callbacks/renders per cycle | 25 | 25-26 |
| Latest observed frames | 441 | 441 |
| Largest observed frames | 970 | 441 |
| Primed/endpoint buffer | n/a | 970 |
| Oversized | 0 | 0 |
| Stream errors | 0 | 0 |

Both retained lifecycle transcripts validate with
`check_output_lifecycle_log.py` (`--backend cpal` and `--backend wasapi`) and
record the exercised `device_selection=pinned` header. The 44.1 kHz runs repeat
the known backend difference at a non-48 kHz rate: CPAL received a 970-frame
maximum callback under a 480-frame request while direct WASAPI rendered
441-frame quanta after a 970-frame priming buffer. These are backend-local
software observations; they do not cover device removal, default-role change,
sleep/wake, or a long soak.

## Interpretation and remaining work

The default-endpoint runs reproduce the known backend difference: CPAL received
a 1,056-frame callback under a 480-frame request while direct WASAPI rendered
480-frame quanta after a 1,056-frame priming buffer. The pinned Realtek runs
add the first symmetric two-backend comparison on a 512-frame reported endpoint:
CPAL observed a 1,126-frame maximum callback while direct WASAPI rendered
512-frame quanta after a 1,126-frame priming buffer. CPAL's largest callback
therefore matched the direct-WASAPI endpoint buffer on both endpoints. The
five-cycle lifecycle runs repeated the same callback-size pattern on both
endpoints with zero faults across start/stop boundaries, and both backends
stayed healthy over the sixty-second pinned-Realtek repeat with the expected
512-frame quanta. The 44.1 kHz virtual-endpoint runs reproduce the same
cross-backend pattern at a non-48 kHz rate: the fixed 480-frame request was
accepted, both backends rendered 441-frame quanta, and CPAL's 970-frame
maximum callback again matched the direct-WASAPI 970-frame endpoint buffer.
All
silent runs stayed inside the 8,192-frame source bound with zero capacity
faults and zero stream errors.

The Phase 0B evidence still needs an observed default-role change, current
default and pinned-device removal, sleep/wake followed by explicit recreation,
and one uninterrupted eight-hour silent soak for each backend. A common
physical/acoustic reference remains required for any latency report.
