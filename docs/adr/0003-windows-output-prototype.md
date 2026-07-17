# ADR 0003: Windows output feasibility prototype

- Status: Provisional
- Date: 2026-07-15

## Context

Roadmap phase 0B needs a minimal Windows output path, while the architecture
requires CPAL to be measured against direct WASAPI before any latency promise or
final backend selection. Device enumeration, callback format, callback size,
removal, default-role changes, and stream recreation all cross the platform
boundary but must not leak into audio, network, or UI crates.

CPAL 0.18 exposes the Windows WASAPI host, persistent platform `DeviceId` values,
device descriptions, supported configurations, and callback streams. It does not
turn a requested buffer size into proof of the actual shared-mode callback cadence.

## Provisional decision

Add an optional `cpal-output` feature to `sonimesh-platform` for the first Windows
11 shared-mode feasibility path.

- enumerate output devices with CPAL `DeviceId`, not display-name matching;
- keep device enumeration independent from default-format probing so one endpoint
  with an unavailable or unsupported format does not hide other devices;
- keep the backend out of shared audio, network, protocol, diagnostics, and UI
  crates;
- validate sample rate, channels, and callback frames against explicit hard bounds
  before opening a stream;
- request a fixed callback size when supported, allow an explicit backend-default
  fallback, record actual callback samples, and keep a separate memory bound;
- render Float32 directly and convert signed 16-bit output through scratch memory
  allocated before stream start;
- if a callback exceeds its preallocated bound, write silence and atomically expose
  a fault for the control domain;
- callback errors set atomic status only; they do not log or call UI IPC;
- stop, drop, device enumeration, and stream recreation remain outside realtime
  execution;
- keep CPAL behind SoniMesh-owned adapter contracts so a direct-WASAPI adapter can
  be compared without changing the audio engine.

This decision does not select the release backend. It authorizes a measured CPAL
prototype only.

## Security, privacy, and realtime implications

Device IDs and descriptions are untrusted local presentation data and must not be
used as identities or written into diagnostic exports without redaction policy.
The output callback receives PCM already owned by the local process and never sees
network packets, keys, Opus payloads, or UI commands.

No callback path may allocate, lock, wait, enumerate devices, recreate a stream,
format log text, or call Tauri. Atomic status uses relaxed ordering because it is
diagnostic state, not lifecycle synchronization.

## Required evidence before acceptance

- enumerate the default and representative USB/Bluetooth output devices on the
  Windows 11 reference host;
- record requested versus observed callback sizes, sample format, rate, and channel
  count;
- exercise default-device removal, pinned-device removal, stop/restart, and
  sleep/wake outside the callback;
- run continuous output and record callback gaps/underruns;
- compare CPAL shared-mode callback behavior and latency with a minimal direct-
  WASAPI adapter on the same device and configuration;
- publish physical/acoustic latency separately from software timestamps.

## Rollback plan

Disable or remove the optional feature and retain the platform-neutral
`AudioOutput` and `RenderSource` contracts. A direct-WASAPI backend can replace the
adapter without moving platform code into `sonimesh-audio` or the desktop UI.
