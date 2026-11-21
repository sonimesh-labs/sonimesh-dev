# Android 17 local-network preflight, 2026-11-22

> Status: emulator platform preflight only; Android discovery remains unvalidated

## Environment

- Device: `emulator-5554`, `Medium_Phone_API_37.0`, x86_64
- Android release/API: Android 17, API 37
- Production sender package: `com.sonimesh.sender`, `targetSdk=36`

## Procedure and result

Read-only ADB inspection confirmed that the emulator is online and exposes both
`android.permission.ACCESS_LOCAL_NETWORK` and
`android.permission.NEARBY_WIFI_DEVICES` in the platform permission catalog.
The installed production sender does not request either permission, does not
request `INTERNET`, and remains the capture-only APK described by ADR 0010.

The inspection also confirmed that the current sender target is SDK 36. No
Android 17 target-SDK discovery build, `NsdManager` picker, browse flow,
service-removal handling, or stale-candidate test was run.

The repeatable command for this metadata collection is:

```powershell
python tools/android_discovery_preflight.py --serial emulator-5554
```

The tool is read-only and rejects unbounded or non-printable ADB metadata before
rendering it. CI runs its `--self-test` mode; normal mode remains device-only.

An isolated target-SDK 37 discovery spike was then built and installed on the
same emulator. Its default connected run completed the request-shape and
permission-state tests while leaving both opt-in probes skipped. The explicit
picker probe was run through the Android JUnit runner with
`runDiscoveryPicker=true`; the three non-local-service tests passed after the
test module declared the `INTERNET` permission required by `NsdManager`.
The probe stopped discovery from its callback and recorded the terminal
`Stopped` callback; it did not claim a discovered peer or trust establishment.
The broad `ACCESS_LOCAL_NETWORK` state was `PERMISSION_DENIED` in the probe
process, so this run does not claim a granted broad-network path.

An opt-in local-service probe was also attempted after an ADB permission grant
and an AppOps allow on the same emulator. The probe logged
`targetPackage=com.sonimesh.discoveryspike` with
`ACCESS_LOCAL_NETWORK` still denied and skipped before registration. An earlier
attempt that reached `NsdManager.registerService` recorded system-side
`MdnsAdvertiser` add/remove messages but did not receive the registration
callback before its bound timeout. The probe now treats missing registration,
discovery, or removal callbacks as an explicit unsupported-platform assumption
skip after cleanup; no service-found or service-lost evidence is claimed.

## Interpretation and limits

This run and the isolated probe establish emulator-only request and callback
evidence. They do not validate Android 17 permission enforcement on a physical
device, broad-permission denial UX, service removal, stale-candidate handling,
manual fallback, or WLAN interoperability. The production capture app remains
unchanged and discovery stays outside its source boundary.
