# Android sender

This directory is reserved for the Kotlin Android sender from roadmap phase 0A.
The application project is intentionally not generated yet: the service and
native ownership boundary is recorded in [ADR 0010](../../docs/adr/0010-android-capture-service-boundary.md),
but Android Gradle tooling and capture behavior have not been validated on the
required device matrix.

The first implementation must keep these module boundaries:

- UI: consent explanation and low-rate state presentation only;
- foreground service: MediaProjection, AudioRecord, encoder, packet protection,
  UDP, and reconnect ownership;
- native boundary, if used: bounded direct buffers and no per-frame UI events;
- platform evidence: actual device format, buffer size, lifecycle, power, and
  silence observations recorded without claiming a source-specific cause.

See [the MVP baseline](../../docs/mvp-baseline.md) for the bootstrap gate.
