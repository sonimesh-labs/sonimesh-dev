# Windows CPAL silent-output smoke

> Status: feasibility evidence only
> Date: 2026-07-18

## Purpose

Verify the current default Windows WASAPI endpoint can start the optional CPAL
adapter with a silent source and report bounded callback observations. This is
not an acoustic or end-to-end latency measurement.

## Environment

| Field | Observed value |
| --- | --- |
| Windows | Windows 11 Home, build 26200, 64-bit |
| Rust | 1.85.0 |
| CPAL | 0.18.1, default Windows host (WASAPI) |
| Default endpoint category | Eight-channel USB endpoint |
| Default format | 48,000 Hz, 8 channels, Float32 |
| Probe-reported buffer range | 480-480 frames |
| Command | `cargo run -p sonimesh-platform --example output_smoke --features cpal-output` |
| Render source | Silence for approximately 2 seconds |

Full WASAPI device IDs and local display names are intentionally excluded.

## Result

The smoke requested 480 frames and preallocated for 8,192 frames. It completed
without stream errors:

| Counter | Observed value |
| --- | ---: |
| Callback count | 199 |
| Latest callback | 3,840 interleaved samples (480 frames) |
| Largest callback | 8,448 interleaved samples (1,056 frames) |
| Oversized callbacks | 0 |
| CPAL stream errors | 0 |

The largest callback was greater than both the 480-frame request and the
endpoint's reported 480-frame range. This supports retaining the independent
preallocated maximum callback bound. It does not establish callback cadence,
device compatibility, semantic speaker routing, or latency.

## Repeat Observation

Re-running the same command on 2026-07-18 produced 198 callbacks. The latest
and largest callback observations remained 3,840 and 8,448 interleaved samples,
respectively; oversized callbacks and stream errors remained zero. This confirms
the same bounded observation on this host only, not a latency or compatibility
claim.

## 2026-07-19 Reconfirmation

A further run against the current default endpoint produced 199 callbacks. The
latest callback contained 3,840 interleaved samples (480 frames), and the
largest contained 8,448 samples (1,056 frames). Oversized callbacks and stream
errors both remained zero. This is another observation of the same host and
configuration, not evidence of callback cadence, latency, or compatibility.

## Remaining Evidence

- exercise device removal, default-role changes, sleep/wake, and stream
  recreation outside the callback;
- record callback gaps and underruns under sustained load;
- compare CPAL shared-mode behavior with a direct-WASAPI adapter on the same
  endpoint; and
- perform physical or acoustic latency measurement with a common reference.
