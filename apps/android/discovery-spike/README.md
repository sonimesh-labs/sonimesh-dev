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

To exercise service appearance and removal on a device that grants broad local
network access, grant `ACCESS_LOCAL_NETWORK` through the system permission UI
and opt into the local-service probe:

```powershell
adb shell am instrument -w `
  -e class com.sonimesh.discoveryspike.Android17DiscoveryProbeTest `
  -e runLocalService true `
  com.sonimesh.discoveryspike.test/androidx.test.runner.AndroidJUnitRunner
adb shell pm revoke com.sonimesh.discoveryspike android.permission.ACCESS_LOCAL_NETWORK
```

The probe logs the permission state before running. A denied permission or a
missing registration, discovery, or removal callback is recorded as an explicit
JUnit assumption skip after cleanup; it is not treated as evidence that the
platform supports the flow. A passing probe records callback behavior only.
Service discovery remains unauthenticated, and no discovered address or name
may establish trust.
