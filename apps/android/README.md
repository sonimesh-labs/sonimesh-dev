# Android sender

This directory is reserved for the Kotlin Android sender from roadmap phase 0A.
The application project is intentionally not generated yet: the repository has
not selected Android Gradle Plugin, Compose, JNI/Opus ownership, or a minimum
tested SDK through ADR 0002.

The first implementation must keep these module boundaries:

- UI: consent explanation and low-rate state presentation only;
- foreground service: MediaProjection, AudioRecord, encoder, packet protection,
  UDP, and reconnect ownership;
- native boundary, if used: bounded direct buffers and no per-frame UI events;
- platform evidence: actual device format, buffer size, lifecycle, power, and
  silence observations recorded without claiming a source-specific cause.

See [the MVP baseline](../../docs/mvp-baseline.md) for the bootstrap gate.

