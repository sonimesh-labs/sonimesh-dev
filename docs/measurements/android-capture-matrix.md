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
| Microphone permission denied | Capture does not start; no service retains consent data | Observed |
| Projection consent cancelled | Capture does not start; UI reports cancellation | Observed |
| Allowed media source | Record actual format, buffer, frames, and silent periods | Observed nonzero signal; source not recorded |
| Idle or silent source | Keep running and report non-definitive no-audio wording | Observed; no source-specific conclusion |
| Protected or policy-limited source | Do not identify a source-specific denial | Not run |
| Lock screen/background | Service and notification remain observable; record power behavior | Partial: lock and background observed; power not recorded |
| Notification stop | Same teardown as projection revocation; resources are released | Observed |
| Projection `onStop()` | AudioRecord, projection, and capture thread are retired once | Observed |
| Process recreation | No consent token or capture session is reused | Observed |

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

### Lifecycle Observations

The microphone permission was revoked for the debug package before the operator
requested capture. The system permission prompt was denied by the operator. The
app then displayed `Capture failed: Microphone permission was not granted`, with
no active SoniMesh service or `MediaProjection` entry. The original permission
state was restored after this test.

For a locked-screen run, the initial snapshot was 23.1 s, 1,106,944 frames,
2,162 silent reads, and zero read errors. After at least 30 seconds locked and
subsequent unlock, the same session reported 114.7 s, 5,506,048 frames, 10,754
silent reads, and zero read errors. The foreground service, application
notification, and active `MediaProjection` entry remained present. This run did
not measure CPU, memory, thermal state, or battery impact.

The operator used the `Stop capture` action from the SoniMesh notification. The
UI reported `Capture stopped: user requested` with 7,613,440 frames over
158.7 s and zero read errors. The service, notification, and
`MediaProjection` entry were all absent afterward.

The operator separately stopped capture through the Android system
recording/sharing control. The UI reported `Capture stopped: projection
revoked`, and the service and `MediaProjection` entry were absent afterward.
The first observation recorded one negative `AudioRecord.read` result while
teardown was already active. This exposed a diagnostic race: the reader counted
the platform wake-up as an error after revocation. The capture service now
suppresses a negative read after its teardown gate is set. A follow-up
system-stop observation reported 129,024 frames over 3.7 s, 252 silent reads,
and zero read errors while retaining the same `projection revoked` terminal
state.

For process recreation, an active 32.8 s capture session was terminated with
`adb shell am force-stop com.sonimesh.sender`. Android then reported no active
service or `MediaProjection` entry. After the operator reopened the app, it
showed `Ready to request playback capture consent` with no restored evidence
counters. This verifies that the observed process recreation does not resume the
prior capture session; it does not inspect any platform-private token storage.

### Supplemental Silent Observation

An earlier capture-only session on the same device produced 1,696,768 frames
over 35.4 s, with 3,314 silent reads and no read errors. The prototype displayed
its non-definitive no-audio wording. This indicates an observed silent capture
condition only; it does not identify why any particular application was silent.

### Not Yet Measured

- CPU, memory, thermal behavior, and battery impact during an extended
  locked-screen run;
- protected or policy-limited playback behavior;
- physical or end-to-end latency.

## Evidence fields

Each run should record:

- device model, OEM, Android release/API level, build fingerprint, and app version;
- capture format, channel mask, actual AudioRecord buffer, and minimum buffer;
- capture duration, captured frames, silent reads, read errors, and terminal state;
- screen/lock state, notification visibility, CPU, memory, thermal state, and battery impact;
- source category as observed by the test operator: allowed, idle, silent, or protected/unknown;
- whether teardown was user requested, projection revoked, process destroyed, or an error.

## Per-run record template

Create one filled record for every device/build/scenario execution. Copy the
fields below into a dated measurement entry; retain the raw app snapshot values
and units instead of calculating a source-specific explanation for silence.

| Field | Recorded value |
| --- | --- |
| Date and local time | — |
| Scenario | Permission denied, cancelled consent, allowed, idle/silent, protected/unknown, lock/background, notification stop, projection `onStop()`, or process recreation |
| Device/OEM/model | — |
| Android release/API/build fingerprint | — |
| App version/build and target SDK | — |
| Projection scope and fresh-consent result | — |
| Observed source category | Allowed, idle, silent, or protected/unknown only; do not name an application as the cause of silence |
| AudioRecord metadata | `audioSource` and `channelMask` from the active `CaptureFormat`; record them as reported integers, not a source-policy explanation |
| Capture format | `sampleRateHz`, `channels`, PCM representation, and `readSamples` from the active `CaptureFormat` |
| AudioRecord buffers | `bufferSizeBytes` actual and `minimumBufferBytes` from the active `CaptureFormat` |
| Initial active snapshot | `captureDurationMillis`, `lastReadCompletionOffsetMillis`, `capturedFrames`, `silentReads`, and `readErrors` |
| Final or terminal snapshot | `captureDurationMillis`, `lastReadCompletionOffsetMillis`, `capturedFrames`, `silentReads`, and `readErrors` |
| Terminal state | Active at observation end, user requested, projection revoked, service destroyed, or exact non-sensitive failure text |
| Lifecycle observation | Lock/background duration, notification visibility, service/projection presence, or process recreation result |
| Device condition | Battery, thermal, memory, and CPU observation when the scenario requires it; otherwise `not measured` |

For an allowed, idle, silent, or protected/unknown observation, `silentReads`
and `lastReadCompletionOffsetMillis` describe only completed local reads. A
nonzero silent count does not identify why a particular source produced silence.
Likewise, the reported `audioSource` and `channelMask` identify only the
AudioRecord configuration observed by the device; they do not identify a source
application or explain a policy-limited capture result.
Record a negative `AudioRecord.read` result in `readErrors` only when teardown
was not already admitted; report a suppressed teardown wake-up separately in
the lifecycle observation.

Do not record PCM, projection tokens, keys, raw device identifiers in diagnostic
exports, or a claim about why a particular application returned silence. These
results remain phase 0A evidence and do not validate discovery, pairing, Opus,
Windows output, or end-to-end latency.
