# Android capture feasibility matrix

> Status: partial physical-device evidence; remaining lifecycle matrix required
> Updated: 2026-08-05

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

## Physical Run Procedure

Use this procedure for every remaining device/build/scenario row. It produces
operator-observed evidence only; shell output and the Compose surface are not a
replacement for a device-side capture result.

1. Record the local time, device/OEM/model, Android release/API/build
   fingerprint, app version, battery state, and whether thermal, CPU, and
   memory observations are available. `adb devices -l`, `adb shell getprop
   ro.build.fingerprint`, and `adb shell dumpsys battery` may assist local lab
   notes but their output format is not a stable product interface.
   Before the first row for a connected device, the read-only helper may collect
   the stable preparation fields without printing the ADB serial:

       python tools/android_capture_preflight.py --serial <local-device-serial>

   It reports device/build/ABI metadata and whether the capture package is
   installed. It does not install, launch, grant, revoke, capture, or validate
   the device; record only its displayed metadata alongside an actual operator
   observation.
2. Start from `Ready to request playback capture consent`. For every capture
   attempt, use fresh MediaProjection consent, select only the required source
   category, and record the displayed `CaptureFormat` fields: sample rate,
   channels, `audioSource`, channel mask, actual buffer, platform-minimum
   buffer. Record the configured read block separately as the fixed 20 ms,
   960-frame / 1,920-sample Phase 0A request; it is not an observed UI field.
3. Record both an initial active snapshot and a final or terminal snapshot:
   `captureDurationMillis`, `lastReadCompletionOffsetMillis`, `capturedFrames`,
   `silentFrames`, `silentReads`, and `readErrors`. Copy raw displayed values
   and units rather than inferring sample cadence or audio latency from them.
4. During a lifecycle scenario, observe the foreground notification and active
   projection/service state before and after the event. Local `dumpsys` output
   can supplement the transcript when available, but retain the event that the
   operator actually performed and do not interpret an unavailable shell view
   as a platform result.
5. End by recording the exact non-sensitive terminal text or active state,
   then restore the prior permission/device state. Do not preserve PCM,
   consent intents, projection tokens, source names, keys, or opaque device
   identifiers in an exported record.

### Structured record validation

After a physical run, operators may place a closed-schema JSON record outside
the repository or in an approved redacted measurement location, then validate
its evidence fields locally:

    python tools/check_android_capture_record.py path/to/android-capture-record.json

The current record format is schema version 3. The validator continues to
accept legacy versions 1 and 2, but new records must use version 3. Version 2
added completed silent-frame duration so it can be checked independently from
non-blocking read count. Version 3 retains the version-2 fields and adds
stricter lifecycle consistency: an active session must retain observable
foreground service evidence, while no-capture records cannot claim a suppressed
teardown wake-up. Legacy records continue to use their original validation
semantics. All versions have these required top-level fields. `recorded_at` must
be an ISO 8601 timestamp with an explicit UTC offset (for example,
`2026-07-20T20:30:00+08:00` or a `Z` suffix) so records remain independently
auditable across operator time zones:

```text
schema_version, recorded_at, scenario, device, app, observed_source_category,
capture_format, initial_snapshot, final_snapshot, terminal_state, lifecycle,
device_conditions
```

`capture_format`, `initial_snapshot`, and `final_snapshot` are `null` only for
the permission-denied and cancelled-consent scenarios, where no capture session
may start. For a real capture scenario, `capture_format` retains
`sample_rate_hz`, `channels`, `audio_source`, `channel_mask`,
`pcm_format`, `buffer_size_bytes`, `minimum_buffer_bytes`, and `read_samples`.
Each schema-version-2 snapshot retains `capture_duration_ms`,
`last_read_completion_offset_ms`, `captured_frames`, `silent_frames`,
`silent_reads`, and `read_errors`. Version 1 omits `silent_frames` only for
backward compatibility. The validator requires final counters and local offsets
to be nondecreasing, requires `silent_frames` not to exceed `captured_frames`,
requires `silent_reads` not to exceed `silent_frames`, and requires each
completion offset to be at most its snapshot's local duration. It checks the
same 20 ms read-size calculation and 64 KiB buffer bound
as the capture bootstrap.

For schema version 3, `terminal_state` is `active` only while a session
remains observable: the record must mark `notification`, `service`, and
`projection` as `present`, and it cannot report a suppressed teardown wake-up.
The pre-capture terminal values `permission_denied` and `consent_cancelled` are
valid only for their matching no-capture scenarios, which also cannot report a
suppressed teardown wake-up. Across all schema versions, the terminal teardown
values `user_requested`, `projection_revoked`, `service_destroyed`, and
`failed` require `notification`, `service`, and `projection` to be recorded as
`absent`; this records the common teardown invariant instead of assuming that
an old stream recovered.

`observed_source_category` is limited to `allowed`, `idle`, `silent`, or
`protected_unknown` for a capture session. It cannot contain an application
name. `lifecycle` records only `present`, `absent`, or `not_measured` for the
notification, service, and projection, plus the event duration and whether a
teardown wake-up was suppressed. `device_conditions` records battery, thermal,
CPU, and memory observations as a non-sensitive value or `not_measured`.
The closed schema deliberately excludes free-form source identifiers, PCM,
consent data, ADB serials, keys, and opaque device IDs.

The validator proves only the internal consistency and redaction boundary of a
record supplied by an operator. It neither runs the device scenario nor turns
silent-frame or silent-read counts into a source-policy explanation, support
claim, or latency measurement. Its synthetic self-test is not physical-device
evidence:

    python tools/check_android_capture_record.py --self-test

Run the following scenarios one at a time. Do not combine a lock, source
change, or process recreation with another scenario, because the terminal
observation would no longer identify the relevant lifecycle event.

| Scenario | Operator action | Required record |
| --- | --- | --- |
| Permission denied | Revoke `RECORD_AUDIO`, request capture, and deny the system prompt. Restore the prior permission after observing the terminal UI. | Failure text plus absence/presence observations for service and projection; no active capture snapshot is expected. |
| Cancelled consent | Request capture and dismiss the MediaProjection consent surface. | Cancellation text plus service/projection observation; do not reuse the returned result. |
| Allowed source | Begin fresh consent while an operator-selected allowed media source is active. | Initial/final snapshots and full observed `CaptureFormat`; source category only. |
| Idle or silent source | Begin fresh consent with an idle or silent source and wait for the no-audio state. | Initial/final snapshots and the non-definitive no-audio wording. Do not name a source as the cause. |
| Protected or policy-limited source | Begin fresh consent while the operator treats the source as protected/unknown. | Initial/final snapshots, `protected/unknown` category, and terminal or active state. Silence alone cannot establish the source's policy. |
| Background | Start capture, send the activity to background, wait a recorded duration, then return. | Before/after snapshots, duration, notification/service/projection visibility, and device condition observations. |
| Lock screen | Start capture, lock for a recorded duration, unlock, and inspect the same session. | Before/after snapshots, lock duration, notification/service/projection visibility, and battery/thermal/CPU/memory observations or `not measured`. |
| Notification stop | Start capture, invoke the actual notification action, and wait for teardown. | Final snapshot, `user requested` terminal state, and service/notification/projection absence after teardown. |
| Projection revocation | Start capture, use the system sharing/recording control to stop projection. | Final snapshot, `projection revoked` terminal state, and teardown observations. Record any suppressed teardown wake-up separately from `readErrors`. |
| Process recreation | Start capture, force-stop or otherwise destroy the process, then reopen it. | Pre-destruction snapshot, service/projection absence, fresh ready state, and no restored counters or consent session. |

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

### Locked-screen resource observation, 2026-07-20

With a fresh active `TYPE_SCREEN_CAPTURE` session, the operator locked and then
unlocked the Vivo device. The service-age observations bracketed approximately
10 minutes between the pre-lock and post-unlock shell checks; the exact
screen-off interval was not independently instrumented. The same process PID,
foreground-service instance, ongoing notification, and active MediaProjection
entry remained present after unlock.

| Field | Before lock | After unlock |
| --- | ---: | ---: |
| Service age at observation | 45.6 s | 11 m 16.5 s |
| Battery level | 100% | 100% |
| Battery temperature | 34.6 C | 34.6 C |
| App total PSS | 115.9 MB | 134.8 MB |
| App total RSS | 285.2 MB | 303.7 MB |
| Active MediaProjection | Present | Present |
| Foreground service and notification | Present | Present |

After unlock, the process remained foreground-service eligible with one active
foreground service. A one-sample host-side `top` observation reported 5% CPU;
it is an instantaneous post-unlock observation, not an average or a locked
screen CPU measurement. The device was connected by USB and Android reported a
full battery throughout, so this observation cannot establish discharge rate,
power consumption, thermal stability, or a sustained memory bound. No PCM,
projection token, source name, or raw ADB serial was retained.

After the operator ended this capture session, Android reported no active
`CaptureService`, no SoniMesh MediaProjection entry, and no active SoniMesh
foreground notification record. The remaining application process was in
background state (`curProcState=16`) with no foreground-service marker. This
confirms resource retirement after the locked-screen observation, but it does
not identify the operator's stop entry point or replace the separate
notification-stop and projection-revocation scenarios above.

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

### Protected/unknown source-category observation, 2026-07-20

The operator selected the `protected_unknown` matrix category, started a fresh
full-screen projection session, and observed an active foreground service,
ongoing notification, and `TYPE_SCREEN_CAPTURE` MediaProjection entry. The
operator did not provide a source name, and none was queried or retained.

| Counter | Observed value |
| --- | ---: |
| Capture duration | 226.5 s |
| Captured frames | 7,000,000 |
| Silent reads | 10 |
| Read errors | 0 |
| Latest completed read | +226.3 s |
| Displayed signal state | Signal observed |

The device was USB powered at 100% battery. Shell observations during the run
reported 35.5 C battery temperature and a single 0% process-CPU sample; those
values are not a power, thermal, or average CPU result. After the operator
paused the session, Android reported no active SoniMesh `CaptureService`, no
SoniMesh MediaProjection entry, and no active SoniMesh foreground notification.
The application UI process remained foreground because it was open.

This is an observed `protected_unknown` category result with a capturable
signal. It does not establish whether any particular media source was protected,
policy-limited, or permitted by Android playback-capture policy.

### Not Yet Measured

- CPU, memory, thermal behavior, and battery impact during an extended
  locked-screen run;
- protected or policy-limited playback behavior;
- physical or end-to-end latency.

## Evidence fields

Each run should record:

- device model, OEM, Android release/API level, build fingerprint, and app version;
- capture format, channel mask, actual AudioRecord buffer, and minimum buffer;
- capture duration, captured frames, silent frames, silent reads, read errors,
  and terminal state;
- screen/lock state, notification visibility, CPU, memory, thermal state, and battery impact;
- source category as observed by the test operator: allowed, idle, silent, or protected/unknown;
- whether teardown was user requested, projection revoked, process destroyed, or an error.

## Per-run record template

Create one filled record for every device/build/scenario execution. Copy the
fields below into a dated measurement entry; retain the raw app snapshot values
and units instead of calculating a source-specific explanation for silence.

| Field | Recorded value |
| --- | --- |
| `recorded_at` | ISO 8601 timestamp with explicit UTC offset, for example `2026-07-20T20:30:00+08:00` or `2026-07-20T12:30:00Z` |
| Scenario | Permission denied, cancelled consent, allowed, idle/silent, protected/unknown, lock/background, notification stop, projection `onStop()`, or process recreation |
| Device/OEM/model | — |
| Android release/API/build fingerprint | — |
| App version/build and target SDK | — |
| Projection scope and fresh-consent result | — |
| Observed source category | Allowed, idle, silent, or protected/unknown only; do not name an application as the cause of silence |
| AudioRecord metadata | `audioSource` and `channelMask` from the active `CaptureFormat`; record them as reported integers, not a source-policy explanation |
| Capture format | `sampleRateHz`, `channels`, PCM representation, and `readSamples` from the active `CaptureFormat` |
| AudioRecord buffers | `bufferSizeBytes` actual and `minimumBufferBytes` from the active `CaptureFormat` |
| Initial active snapshot | `captureDurationMillis`, `lastReadCompletionOffsetMillis`, `capturedFrames`, `silentFrames`, `silentReads`, and `readErrors` |
| Final or terminal snapshot | `captureDurationMillis`, `lastReadCompletionOffsetMillis`, `capturedFrames`, `silentFrames`, `silentReads`, and `readErrors` |
| Terminal state | Active at observation end, user requested, projection revoked, service destroyed, or exact non-sensitive failure text |
| Lifecycle observation | Lock/background duration, notification visibility, service/projection presence, or process recreation result |
| Device condition | Battery, thermal, memory, and CPU observation when the scenario requires it; otherwise `not measured` |

For an allowed, idle, silent, or protected/unknown observation,
`silentFrames`, `silentReads`, and `lastReadCompletionOffsetMillis` describe only
completed local reads. Snapshot integers must fit Android signed 64-bit storage;
cumulative counters saturate at that maximum instead of wrapping negative. The UI
changes to “no capturable audio detected” only after two seconds worth of
completed silent frames at the observed sample rate; partial non-blocking reads
do not shorten that frame-duration threshold. A nonzero silent count does not
identify why a particular source produced silence. Likewise, the reported
`audioSource` and `channelMask` identify only the
AudioRecord configuration observed by the device; they do not identify a source
application or explain a policy-limited capture result.
Record a negative `AudioRecord.read` result in `readErrors` only when teardown
was not already admitted; report a suppressed teardown wake-up separately in
the lifecycle observation.

Do not record PCM, projection tokens, keys, raw device identifiers in diagnostic
exports, or a claim about why a particular application returned silence. These
results remain phase 0A evidence and do not validate discovery, pairing, Opus,
Windows output, or end-to-end latency.
