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
  fallback, record actual callback samples and frames, and keep a separate memory
  bound;
- publish the latest and largest callback frame counts, latest monotonic
  callback offset, and largest callback gap through atomics for low-rate
  comparison; these are software scheduling observations, not a latency
  measurement;
- render Float32 directly and convert signed 16-bit output through scratch memory
  allocated before stream start;
- if a callback exceeds its preallocated bound, write silence and atomically expose
  a fault for the control domain;
- callback errors set atomic status only; they do not log or call UI IPC;
- stop, drop, device enumeration, and stream recreation remain outside realtime
  execution;
- output recovery creates a fresh bounded PCM producer/consumer pair and source,
  starts the platform callback, then installs the matching producer and resets
  only the affected receiver stream's playout state; failed endpoint creation or
  platform start leaves the existing producer installed for a later retry;
- output loss invalidates pending platform or media completions before recovery,
  so a late operation cannot commit across fresh output endpoints;
- keep CPAL behind SoniMesh-owned adapter contracts so a direct-WASAPI adapter can
  be compared without changing the audio engine.

This decision does not select the release backend. It authorizes a measured CPAL
prototype only.

## Direct-WASAPI comparison boundary

The direct-WASAPI comparison is a separate Windows-only experimental tool at
`tools/windows-wasapi-prototype`. It implements the platform-neutral
`AudioOutput` and `RenderSource` contracts but does not depend on, wrap, or
share implementation details with the CPAL adapter.

- keep `sonimesh-platform` under the workspace `unsafe_code = forbid` rule;
  the tool is the narrow reviewed exception required to call Windows COM and use
  the audio-engine-owned render buffer;
- confine COM initialization, endpoint activation, event waiting, and all raw
  buffer access to the tool's worker thread; no raw Windows handle or COM object
  crosses into the shared crate;
- use a shared-mode, event-driven default-render-endpoint path only; the tool
  accepts a pinned identifier only when it still matches that current default;
- use the endpoint mix format only when it exactly matches the bounded
  `OutputConfig`; unsupported formats fail before the worker starts;
- record endpoint buffer frames, source-render frame counts, latest monotonic
  render offset, maximum render gap, priming silence, capacity faults, and
  backend errors with atomics; these software observations are comparable with
  CPAL status but are not latency measurements;
- stop requests wake the worker, which stops the audio client and releases COM
  resources on its owning thread before control-domain join returns;
- keep one-shot silent smoke and repeated start/stop smoke as explicit lab commands
  for each backend; they report faults but do not attempt callback-thread recovery
  or establish device-removal compatibility.

The matching silent-smoke commands run for two seconds by default. For an
explicit long-duration experiment, each accepts `SONIMESH_OUTPUT_SMOKE_SECONDS`
from 1 through 28,800 seconds (eight hours). The selected duration is a
control-domain parameter only; it does not alter the requested callback frames,
maximum source capacity, endpoint selection, or callback behavior. Results must
record the actual duration and retain capacity faults and backend errors.

Separate CPAL and direct-WASAPI silent-soak commands default to eight hours.
They read only each adapter's existing atomic status from the control domain at
a bounded 60-second interval, then print the accumulated observation. The
commands accept `SONIMESH_OUTPUT_SOAK_SECONDS` (1 through 28,800) and
`SONIMESH_OUTPUT_SOAK_REPORT_SECONDS` (1 through the selected duration) for
explicit lab configuration. Reporting changes neither callback scheduling nor
the output source. A capacity fault or backend error ends the soak after its
final control-domain status is recorded.

Separate default-role recovery commands exercise one manual lifecycle sequence:
start a default-endpoint silent stream, wait for the operator to change the
Windows default render role, capture the old stream's status, stop it in the
control domain, re-probe the current default, and start a fresh silent stream
with that new endpoint's bounded configuration. They do not attempt to migrate
or recreate a stream from the callback. The commands redact endpoint identifiers
and require an observed endpoint change by default; a local command check may
explicitly disable that assertion but cannot count as default-role evidence.

The direct tool is not a selected backend and must not be linked by the desktop
app. Its evidence record may compare only like-for-like endpoint, mix format,
and requested duration configurations.

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
