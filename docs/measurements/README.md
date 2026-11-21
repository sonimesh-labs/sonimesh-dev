# Measurement evidence

Use the [testing and measurement plan](../testing-and-measurement.md) for the
general method and the [baseline](../mvp-baseline.md) for open gates. Undated
protocols and matrices specify what to record; dated records describe particular
runs. Read the environment and limitations in each record before generalizing.

## Android capture and identity

- [Capture matrix](./android-capture-matrix.md) — capture and lifecycle evidence.
- [Android 17 local-network preflight, 2026-11-22](./2026-11-22-android-17-local-network-preflight.md) — API 37 emulator and permission-catalog check only; discovery remains unvalidated.
- [Keystore protocol](./android-keystore-spike.md) and [2026-07-20 run](./2026-07-20-android-keystore-spike.md).
- [Android host build verification, 2026-08-15](./2026-08-15-android-host-build-verification.md).

## TLS and credential storage

- [Android TLS protocol](./android-tls-provider-spike.md) and [2026-07-20 run](./2026-07-20-android-tls-provider-spike.md).
- [Android to desktop TLS interoperability, 2026-11-22](./2026-11-22-android-rustls-tls-interoperability.md) — emulator/ADB reverse-loopback result; physical WLAN remains open.
- [Windows TLS protocol](./windows-tls-provider-spike.md), [2026-07-20 run](./2026-07-20-windows-tls-provider-spike.md), and [2026-08-22 rerun](./2026-08-22-windows-tls-provider-spike.md).
- [Windows credential-store protocol](./windows-credential-store-spike.md) and [2026-07-20 run](./2026-07-20-windows-credential-store-spike.md).
- [ADR 0021](../adr/0021-production-tls-provider.md) tracks the provisional provider choice and cross-platform gate.

## Windows output

- [Backend comparison protocol](./windows-output-backend-comparison.md), [comparison](./2026-07-20-windows-output-backend-comparison.md), and [recheck](./2026-07-20-windows-output-backend-recheck.md).
- CPAL [2026-07-17 smoke](./2026-07-17-windows-cpal-smoke.md), [2026-07-18 probe](./2026-07-18-windows-cpal-probe.md), and [2026-07-18 smoke](./2026-07-18-windows-cpal-smoke.md).
- [2026-08-13 output recheck](./2026-08-13-windows-output-recheck.md).

## Opus and UI

- [Opus experiment matrix](./opus-build-and-experiment-matrix.md), [native preflight](./2026-07-20-opus-native-preflight.md), and [native build](./2026-08-16-windows-opus-native-build.md).
- Codec experiments: [10 ms versus 20 ms](./2026-08-16-windows-opus-10ms-vs-20ms.md), [DTX](./2026-08-16-windows-opus-dtx.md), and [in-band FEC](./2026-08-16-windows-opus-inband-fec.md).
- [Desktop preview viewport check](./2026-08-16-desktop-ui-viewport-check.md).

When adding evidence, link the dated run from its protocol or matrix and this
index, identify the environment and procedure, report failures and limitations,
and update the applicable ADR or baseline if the evidence changes a gate.
