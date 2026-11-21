# Android 17 Discovery Spike

This isolated instrumentation target exercises the Android 17 `NsdManager`
request shape for ADR 0014. It is not production sender code: it has no
MediaProjection, audio capture, TLS, pairing, or media transport.
The module declares `INTERNET` because Android's `NsdManager` service rejects
clients without it; the permission is confined to this test-only process.

The default connected test run only checks request construction and reports the
declared broad-permission state. The picker callback probe is opt-in because it
exercises the system-mediated picker path and should be run interactively:

```powershell
.\gradlew.bat :discovery-spike:connectedDebugAndroidTest --no-daemon
adb shell am instrument -w `
  -e class com.sonimesh.discoveryspike.Android17DiscoveryProbeTest `
  -e runDiscoveryPicker true `
  com.sonimesh.discoveryspike.test/androidx.test.runner.AndroidJUnitRunner
```

A passing probe records callback behavior only. Service discovery remains
unauthenticated, and no discovered address or name may establish trust.
