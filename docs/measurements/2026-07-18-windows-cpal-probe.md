# Windows CPAL output probe

> Status: read-only feasibility evidence only
> Date: 2026-07-18

## Purpose

Confirm that the optional CPAL adapter can enumerate the current Windows WASAPI
output endpoints and read their default format and reported buffer range. This
probe does not open an output stream.

## Environment

| Field | Observed value |
| --- | --- |
| Windows | Windows 11 Home, build 26200, 64-bit |
| Rust | 1.85.0 |
| CPAL | 0.18.1, default Windows host (WASAPI) |
| Command | `cargo run -p sonimesh-platform --example output_probe --features cpal-output` |

## Observations

The probe enumerated five output endpoints. The current default was an
eight-channel USB endpoint with a 48,000 Hz Float32 default format and a
reported 480-frame buffer range.

| Endpoint category | Default format | Reported buffer range |
| --- | --- | --- |
| Current default USB endpoint | 48,000 Hz, 8 channels, Float32 | 480-480 frames |
| Virtual audio endpoint | 44,100 Hz, 2 channels, Float32 | 441-441 frames |
| Noise-processing endpoint | 48,000 Hz, 2 channels, Float32 | 480-480 frames |
| HDMI/display-audio endpoint | 48,000 Hz, 2 channels, Float32 | 480-480 frames |
| Integrated audio endpoint | 48,000 Hz, 2 channels, Float32 | 512-512 frames |

Full WASAPI device IDs are intentionally excluded. Device display names are
local presentation data and are not retained in this record.

## Scope and Remaining Evidence

This confirms only read-only endpoint enumeration and default-format probing.
It does not validate stream start, callback cadence, device removal, speaker
routing, audio quality, or latency. The two-second silent stream observations
remain documented separately in
[2026-07-17 Windows CPAL smoke](./2026-07-17-windows-cpal-smoke.md).
