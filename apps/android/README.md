# Android sender

This directory contains the capture-only Kotlin/Compose bootstrap for roadmap
phase 0A. It is a feasibility prototype, not a sender release: it requests
MediaProjection consent, starts a media-projection foreground service, creates a
bounded `AudioRecord`, and discards PCM after recording low-rate evidence
counters. It does not encode, pair, protect, transmit, or persist audio.

The service and native ownership boundary is recorded in [ADR 0010](../../docs/adr/0010-android-capture-service-boundary.md).
The project builds against the checked-in Gradle wrapper, but no physical device
matrix or capture behavior claim is implied until the runs in the
[Android capture matrix](../../docs/measurements/android-capture-matrix.md)
are recorded.

The first implementation must keep these module boundaries:

- UI: consent explanation and low-rate state presentation only;
- foreground service: MediaProjection, AudioRecord, encoder, packet protection,
  UDP, and reconnect ownership;
- native boundary, if used: bounded direct buffers and no per-frame UI events;
- platform evidence: actual device format, buffer size, lifecycle, power, and
  silence observations recorded without claiming a source-specific cause.

The prototype deliberately keeps the consent `Intent` in memory and routes both
the notification stop action and `MediaProjection.Callback.onStop()` through
one idempotent teardown path. See [the MVP baseline](../../docs/mvp-baseline.md)
for the remaining bootstrap gates.
