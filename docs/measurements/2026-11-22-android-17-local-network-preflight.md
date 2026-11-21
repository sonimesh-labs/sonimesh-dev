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

## Interpretation and limits

This run proves only that an API 37 emulator and the relevant permission names
are available for a future discovery spike. It does not validate Android 17
permission enforcement, picker UX, broad-permission denial behavior, mDNS
discovery, manual fallback, or any network interoperability. The production
capture app remains unchanged and discovery stays outside its source boundary.
