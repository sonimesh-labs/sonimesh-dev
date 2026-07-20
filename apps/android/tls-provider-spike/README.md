# Android TLS Provider Spike

This is an isolated Android instrumentation-test target for the ADR 0008
Keystore and TLS-provider feasibility probes. It has `INTERNET` permission and
the Conscrypt runtime only so its own process can bind a loopback TLS listener
during the test.

It contains no activity, MediaProjection, audio capture, pairing protocol, QR
payload handling, persistent identity storage, or media transport. The
capture-only `:app` module intentionally remains without network permissions or
TLS/Keystore dependencies.

Run the device-only evidence probe with a connected API 31+ device or emulator:

```powershell
.\gradlew.bat :tls-provider-spike:connectedDebugAndroidTest --no-daemon
```

On a device where Gradle/UTP times out while installing an already-built test
APK, use the direct AndroidJUnitRunner fallback described in the measurement
protocol and record the UTP failure category with the result. The fallback does
not replace device execution or alter the test assertions.

The result must be recorded using the requirements in
[`docs/measurements/android-keystore-spike.md`](../../../docs/measurements/android-keystore-spike.md)
and
[`docs/measurements/android-tls-provider-spike.md`](../../../docs/measurements/android-tls-provider-spike.md).
