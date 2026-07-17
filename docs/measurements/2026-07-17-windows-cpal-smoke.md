# Windows CPAL silent-output smoke

> Status: feasibility evidence only
> Date: 2026-07-17

## Purpose

Verify that the optional `sonimesh-platform` CPAL adapter can enumerate the local
WASAPI endpoints, select the default endpoint through CPAL, start a silent stream,
and report real callback sizes without allocating or logging from the callback.

This is not an acoustic or end-to-end latency measurement.

## Environment

| Field | Observed value |
| --- | --- |
| Windows | Windows 11 Home, build 26200, 64-bit |
| Rust | 1.85.0 |
| CPAL | 0.18.1, default Windows host (WASAPI) |
| Default output | ToDesk Virtual Audio speaker endpoint |
| Default format | 44,100 Hz, 2 channels, Float32 |
| Reported buffer range | 441–441 frames |
| Render source | Silence for approximately 2 seconds |

The host also reported ASUS AI Noise-cancelling Output at 48 kHz/2ch/F32 with a
480-frame range and Realtek Audio at 48 kHz/2ch/F32 with a 512-frame range. Full
device identifiers are intentionally not copied into this report.

## Result: virtual endpoint

The smoke test requested 480 frames against the default endpoint and preallocated
for 8,192 frames. It completed with:

| Counter | Observed value |
| --- | ---: |
| Callback count | 201 |
| Latest callback | 882 interleaved samples (441 frames) |
| Largest callback | 1,940 interleaved samples (970 frames) |
| Oversized callbacks | 0 |
| CPAL stream errors | 0 |

The 970-frame callback is larger than both the 480-frame request and the endpoint's
reported 441-frame range. This validates the architecture decision to separate an
optional fixed request from a preallocated maximum callback bound. It does not
indicate a latency result, because no source/reference correlation or common clock
was used.

## Result: default-role change and USB endpoint

During the same validation session Windows changed the default output role to
MCHOSE V9 Turbo+. The probe reported 48 kHz, 8 channels, Float32, and a 480-frame
range. Re-running the same silent smoke with a 480-frame request and 8,192-frame
maximum produced:

| Counter | Observed value |
| --- | ---: |
| Callback count | 199 |
| Latest callback | 3,840 interleaved samples (480 frames) |
| Largest callback | 8,448 interleaved samples (1,056 frames) |
| Oversized callbacks | 0 |
| CPAL stream errors | 0 |

This second endpoint again delivered a callback larger than the reported fixed
range. It also confirms that default-device changes are observable at the control
boundary. The silent source filled all eight output channels; semantic speaker
routing has not been implemented or qualified by this smoke test.

## Remaining evidence

- repeat on a Realtek endpoint and qualify semantic source-to-device routing;
- record direct-WASAPI results on the same device/configuration;
- test device removal, default-role change, sleep/wake, and stream recreation;
- collect callback-gap/underrun counters under load and an 8-hour run;
- perform the physical or acoustic latency procedure from
  [Testing and Measurement](../testing-and-measurement.md).
