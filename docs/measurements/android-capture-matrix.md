# Android capture feasibility matrix

> Status: partial physical-device evidence; remaining lifecycle matrix required
> Updated: 2026-07-19

This document is the evidence boundary for the capture-only Android bootstrap.
A successful Gradle build proves only that the application and its bounded
lifecycle code compile. It does not prove that any source is capturable on a
device, that a projection survives backgrounding, or that a latency promise is
possible.

## Fixed prototype configuration

| Item | Value |
| --- | --- |
| Capture format | PCM16, 48,000 Hz, stereo |
| Read block | 20 ms, 960 frames, 1,920 interleaved samples |
| AudioRecord buffer | Actual and platform-minimum values, each capped at 64 KiB |
| Ownership | Foreground service after consent; UI receives low-rate state only |
| Evidence publication | Duration, frames, silent reads, and errors at no more than 2 Hz |
| PCM persistence | None; samples are scanned for silence and discarded |
| Transport | None; no encoder, keys, pairing, or UDP |
| Silence wording | "No capturable audio detected"; no source-specific cause |

## Required run matrix

Record one row per device, build, and scenario. “Pass” means the observed
behavior matches the lifecycle invariant, not that audio is necessarily present.

| Scenario | Required observation | Result |
| --- | --- | --- |
| Microphone permission denied | Capture does not start; no service retains consent data | Not run |
| Projection consent cancelled | Capture does not start; UI reports cancellation | Observed |
| Allowed media source | Record actual format, buffer, frames, and silent periods | Observed nonzero signal; source not recorded |
| Idle or silent source | Keep running and report non-definitive no-audio wording | Observed; no source-specific conclusion |
| Protected or policy-limited source | Do not identify a source-specific denial | Not run |
| Lock screen/background | Service and notification remain observable; record power behavior | Partial: background observed; lock/power not run |
| Notification stop | Same teardown as projection revocation; resources are released | Not run; action registration observed only |
| Projection `onStop()` | AudioRecord, projection, and capture thread are retired once | Not run |
| Process recreation | No consent token or capture session is reused | Not run |

## Physical Run: Vivo PD2309, 2026-07-19

> Result: capture and background feasibility evidence only. This is not a
> compatibility, source-policy, power, or latency result.

### Environment

| Field | Observed value |
| --- | --- |
| Device | Vivo V2309A (PD2309) |
| Android | 15 / API 35, build AP3A.240905.015.A1/compiler08221644 |
| App | `com.sonimesh.sender` 0.1.0 debug, target SDK 36 |
| Projection scope | User-selected entire screen; fresh consent for this run |
| AudioRecord source | `REMOTE_SUBMIX` / source 8, as reported by Android audio policy |
| Actual capture format | PCM16, 48,000 Hz, stereo (channel mask `0xC`) |
| AudioRecord buffer | 38,400 B actual; 7,680 B platform minimum |
| Read configuration | 20 ms target, 960 frames / 1,920 interleaved samples |
| PCM persistence | None; samples were scanned for silence and discarded |

### Active Capture Observation

The full-screen projection session produced a nonzero signal. No source package,
title, PCM, or source-policy conclusion was recorded. At explicit app stop, the
low-rate UI snapshot reported:

| Counter | Observed value |
| --- | ---: |
| Capture duration | 193.9 s |
| Captured frames | 9,300,992 |
| Silent reads | 6 |
| Read errors | 0 |
| Latest completed read | +193.8 s |
| Terminal state | User requested stop |

While the activity was backgrounded for approximately 15 seconds, Android kept
the media-projection foreground service and its ongoing notification present.
On return, the capture duration increased from 30.2 s to 69.7 s and frame
count increased from 1,449,984 to 3,342,272; no read error was observed.

The app's explicit Stop control retired both the service and the active
`MediaProjection` entry as observed through Android system state. The service
notification registered a `Stop capture` action. Vivo's system capture notice
hid that action from the automation hierarchy, so notification-UI dispatch was
not exercised in this run.

### Cancelled Consent Observation

From a stopped state, the app displayed the system screen-capture consent page.
Cancelling that page returned the UI to `Capture failed: Screen capture consent
was not granted`. Android reported neither an active SoniMesh service nor an
active `MediaProjection` entry afterward. No capture session was started.

### Supplemental Silent Observation

An earlier capture-only session on the same device produced 1,696,768 frames
over 35.4 s, with 3,314 silent reads and no read errors. The prototype displayed
its non-definitive no-audio wording. This indicates an observed silent capture
condition only; it does not identify why any particular application was silent.

### Not Yet Measured

- lock-screen continuation, CPU, memory, thermal behavior, and battery impact;
- microphone-permission denial and projection-consent cancellation;
- protected or policy-limited playback behavior;
- notification action dispatch and `MediaProjection.Callback.onStop()`;
- process recreation and any attempt to reuse stale consent; and
- physical or end-to-end latency.

## Evidence fields

Each run should record:

- device model, OEM, Android release/API level, build fingerprint, and app version;
- capture format, channel mask, actual AudioRecord buffer, and minimum buffer;
- capture duration, captured frames, silent reads, read errors, and terminal state;
- screen/lock state, notification visibility, CPU, memory, thermal state, and battery impact;
- source category as observed by the test operator: allowed, idle, silent, or protected/unknown;
- whether teardown was user requested, projection revoked, process destroyed, or an error.

Do not record PCM, projection tokens, keys, raw device identifiers in diagnostic
exports, or a claim about why a particular application returned silence. These
results remain phase 0A evidence and do not validate discovery, pairing, Opus,
Windows output, or end-to-end latency.
