# ADR 0010: Android capture service and native boundary

- Status: Accepted design; capture implementation remains gated by device evidence
- Date: 2026-07-17

## Context

Phase 0A requires a capture-only Android prototype, but playback capture is
policy- and lifecycle-limited. Android 12-17 is the active validation range;
Android 14+ adds MediaProjection foreground-service and single-consent-session
requirements, while Android 17 local-network behavior needs its own discovery
validation. A UI-owned capture loop would be destroyed by ordinary activity
lifecycle changes, while allowing a service or native layer to own consent UI
would make permission and teardown behavior difficult to verify.

The architecture requires the sender to own MediaProjection, AudioRecord,
fixed PCM buffers, encoding, packet protection, socket I/O, and reconnect
behavior outside its UI. No Android SDK, Gradle project, MediaProjection run, or
native Opus build has yet been validated in this repository.

## Decision

The future Android application uses Kotlin and Jetpack Compose for its activity
UI. The activity owns only user interaction and the platform consent launch:

- explain capture limits before requesting consent and present low-rate service
  state after the request;
- transfer the one current consent result directly to the foreground service for
  immediate setup; never persist, cache, or reuse the projection token;
- emit no PCM, encoded frames, keys, or high-rate metering events to Compose.

The foreground service is the sole lifecycle owner once a capture request has
been accepted. It owns:

- MediaProjection creation and a registered `MediaProjection.Callback`;
- AudioRecord construction, reads, fixed PCM buffers, and observed capture
  format/buffer diagnostics;
- the bounded encoder, packet-protection, socket, and reconnect workers;
- the visible foreground notification and its explicit stop action;
- service state publication to the activity through a bounded, low-rate state
  model.

`MediaProjection.Callback.onStop()` and the notification stop action use the
same idempotent teardown path. That path stops capture admission, releases
AudioRecord and projection resources, retires PCM/encoded buffers, destroys
session credentials, closes transport resources, and publishes a terminal
state. Process recreation, a revoked projection, or a stale control session
fails closed and requires a fresh consent result and fresh session material.

Any future JNI/NDK component is limited to the codec and bounded buffer boundary
defined by ADR 0009. Kotlin remains responsible for Android permission,
MediaProjection, AudioRecord, foreground-service, notification, and network
lifecycle. The native layer cannot call UI APIs, retain a projection token, or
own an unbounded queue. Its caller supplies fixed direct buffers and receives
only bounded result/error codes.

The first Android implementation is capture-only: it records actual platform
behavior and may report “no capturable audio detected” with possible causes. It
does not claim source-specific capture denial, pair devices, send a production
audio stream, or make a latency promise.

## Security, privacy, and realtime implications

The consent result, captured PCM, encoded audio, pairing secrets, session keys,
and raw device identifiers are never logged, serialized into UI state, or placed
in diagnostics exports. Display names and observed source behavior remain
untrusted presentation data. Capture, encode, and transport workers must have
fixed capacity and explicit overflow behavior; the UI only observes summaries.

Projection revocation is an authorization boundary, not a recoverable pause.
The service must not continue using a pre-revocation AudioRecord, media key, or
socket state after teardown begins. Silence after a valid consent result is an
observable symptom, not evidence about a particular source application.

## Required evidence before capture implementation is accepted

- capture-only runs on the reference Android 12-17 matrix, recording actual
  AudioRecord format, channels, buffer sizes, read errors, and silent periods;
- lock, backgrounding, notification stop, process recreation, and
  `onStop()` each exercise the same resource-retirement invariants;
- Android 14+ foreground-service and one-consent-session behavior is observed
  on real devices;
- power, CPU, memory, thermal state, and foreground-notification behavior are
  recorded during a locked-screen run;
- allowed, idle, and silent/protected-source cases are reported with the
  product's non-definitive wording;
- any JNI codec prototype proves direct-buffer bounds, ABI packaging, and no
  ownership of Android lifecycle APIs before it is connected to capture.

These results are phase 0A evidence only. They do not validate local-network
discovery, TLS pairing, Opus quality, Windows output, or physical latency.

## Rollback plan

Keep `apps/android/` as documentation-only and retain the Rust core/platform
contracts. A failed capture feasibility result removes the Android prototype
without changing protocol, desktop UI, receiver audio, or pairing boundaries.
