# Android host build verification - 2026-08-15

> Status: reference-host build verification only. This records that the
> checked-in Gradle wrapper and the documented SDK components produce a green
> Android CI command set on the current Windows host. It does not prove any
> device capture behavior, lifecycle survival, source capturability, or
> latency; the device-evidence gates in
> [ADR 0010](../adr/0010-android-capture-service-boundary.md) and the
> [Android capture matrix](./android-capture-matrix.md) remain open.

## Purpose

Record that the Android phase 0A bootstrap builds and passes its host JVM
tests on the current reference host after installing the SDK component that the
local SDK was missing (`build-tools;35.0.0`). Previously the app could not
compile locally because Android Gradle Plugin 8.9.1 defaults to Build Tools
35.0.0, which was not installed, so the Android verification axis was blocked
even though every platforms package was present. This record is the
Android-side companion to the Windows output rechecks: it labels what is
verified (host build reproducibility) and what is not (any device-side
observation).

## Environment

| Field | Observed value |
| --- | --- |
| Local run date | 2026-08-15, Asia/Shanghai |
| Host | Windows 11, x86_64 |
| Java | 21.0.8 LTS |
| Gradle wrapper | 8.11.1 |
| Android Gradle Plugin | 8.9.1 (root `build.gradle.kts`) |
| Kotlin | 2.0.21 |
| compileSdk / targetSdk / minSdk | 36 / 36 / 31 |
| Android SDK | platform-tools, platforms;android-36, build-tools 34.0.0, 35.0.0, 36.1.0 |
| Maven repositories | `edgedl.me.gvt1.com/android/maven2` mirror plus mavenCentral (`settings.gradle.kts`) |

## Command set

The exact Android job command set from
[CI](../../.github/workflows/ci.yml) ran locally:

~~~powershell
Push-Location apps/android
.\gradlew.bat lintDebug testDebugUnitTest assembleDebug :tls-provider-spike:assembleDebugAndroidTest --no-daemon
Pop-Location
~~~

Result: BUILD SUCCESSFUL (122 actionable tasks). Lint, the host JVM unit
tests, the debug APK, and the TLS-provider-spike debug-androidTest APK were all
produced.

## Host JVM unit tests

94 host JVM tests pass across the capture coordination helpers
(`com.sonimesh.sender.capture.*`), the bounded control-frame and negotiation
vectors (`com.sonimesh.sender.protocol.*`), and the pairing/media-crypto
vectors (`com.sonimesh.sender.security.*`). These tests do not add
networking, TLS, crypto, Opus, or NDK use to the production capture source set
and do not exercise any Android runtime provider or physical device.

## Interpretation and remaining work

The green host build is required but not sufficient evidence for phase 0A. It
records no physical-device observation: projection consent, lock/background
survival, MediaProjection `onStop()`, notification-stop teardown, power
behavior, and capturable-source classes are not claimed here. Those rows remain
open in the [Android capture matrix](./android-capture-matrix.md), and the
evidence gates in [ADR 0010](../adr/0010-android-capture-service-boundary.md)
are unchanged. When a local SDK is missing the required component, install the
same components the CI job installs (`platform-tools`,
`platforms;android-36`, `build-tools;35.0.0`) before running the command
set above.
