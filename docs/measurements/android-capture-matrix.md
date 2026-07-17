# Android capture feasibility matrix

> Status: test protocol and evidence template; no physical-device runs recorded
> Updated: 2026-07-17

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
| AudioRecord buffer | Platform minimum, capped at 64 KiB |
| Ownership | Foreground service after consent; UI receives low-rate state only |
| Evidence publication | Summarized counters at no more than 2 Hz |
| PCM persistence | None; samples are scanned for silence and discarded |
| Transport | None; no encoder, keys, pairing, or UDP |
| Silence wording | "No capturable audio detected"; no source-specific cause |

## Required run matrix

Record one row per device, build, and scenario. “Pass” means the observed
behavior matches the lifecycle invariant, not that audio is necessarily present.

| Scenario | Required observation | Result |
| --- | --- | --- |
| Microphone permission denied | Capture does not start; no service retains consent data | Not run |
| Projection consent cancelled | Capture does not start; UI reports cancellation | Not run |
| Allowed media source | Record actual format, buffer, frames, and silent periods | Not run |
| Idle or silent source | Keep running and report non-definitive no-audio wording | Not run |
| Protected or policy-limited source | Do not identify a source-specific denial | Not run |
| Lock screen/background | Service and notification remain observable; record power behavior | Not run |
| Notification stop | Same teardown as projection revocation; resources are released | Not run |
| Projection `onStop()` | AudioRecord, projection, and capture thread are retired once | Not run |
| Process recreation | No consent token or capture session is reused | Not run |

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
