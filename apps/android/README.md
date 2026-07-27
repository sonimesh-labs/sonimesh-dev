# Android sender

This directory contains the capture-only Kotlin/Compose bootstrap for roadmap
phase 0A. It is a feasibility prototype, not a sender release: it requests
MediaProjection consent, starts a media-projection foreground service, creates a
bounded `AudioRecord`, and discards PCM after recording low-rate total-frame,
silent-frame, read-error, and timing evidence counters. Non-teardown negative
read results, thrown read exceptions, and invalid positive read sizes are
counted before any out-of-request sample access. It does not
encode, pair, protect, transmit, or persist audio.

The service and native ownership boundary is recorded in [ADR 0010](../../docs/adr/0010-android-capture-service-boundary.md).
The project builds against the checked-in Gradle wrapper, but no physical device
matrix or capture behavior claim is implied until the runs in the
[Android capture matrix](../../docs/measurements/android-capture-matrix.md)
are recorded.

The first implementation must keep these module boundaries:

- UI: consent explanation and low-rate state presentation only;
- foreground service: Phase 0A MediaProjection, AudioRecord, bounded read
  buffers, and teardown ownership only; encoder, packet protection, UDP, and
  reconnect work remain excluded until their later evidence gates;
- native boundary, if introduced after the capture-only gate: bounded direct
  buffers and no per-frame UI events;
- platform evidence: actual device format, buffer size, lifecycle, power, and
  silence observations recorded without claiming a source-specific cause.

The prototype deliberately keeps the consent `Intent` in memory and routes both
the notification stop action and `MediaProjection.Callback.onStop()` through
one idempotent teardown path. Its atomic reader gate refuses launch after
teardown, rolls back a failed thread start, blocks cleanup until the terminal
outcome and stop requests are published, and permits exactly one final claim
after the reader exits. A fresh consent result delivered to that same retiring
service generation is rejected with an actionable retry state instead of being
silently consumed; active duplicate starts remain no-ops. Final cleanup retires
service references first and attempts every bounded release step despite an
ordinary platform exception. A fatal JVM failure is rethrown after those
attempts instead of being swallowed. Ordinary cleanup failures produce one
redacted count-only diagnostic without exception text. Host JVM tests also verify these pure coordination helpers and the intentionally
public pairing-binding, media-crypto, and
bounded control-frame protocol vectors. Those
tests do not add networking or crypto to the production capture source set,
implement an authenticated control channel, or prove Android runtime provider
behavior. See [the MVP baseline](../../docs/mvp-baseline.md) for the remaining
bootstrap gates.
