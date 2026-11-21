# Android Conscrypt to desktop Rustls interoperability result

> Status: emulator and ADB reverse-loopback evidence; physical WLAN gate remains open
> Updated: 2026-11-22

This record covers one opt-in run of the ADR 0021 interoperability probe. The
desktop `interop-server` generated a fresh self-signed lab certificate and the
Android instrumentation test pinned the printed DER SPKI. The Android side used
an explicitly constructed Conscrypt 2.6.1 TLS context. After the TLS 1.3
handshake, each side derived its role-separated proof from its own exporter; no
exporter, media root, or pairing secret was sent or logged.

## Environment

| Field | Recorded value |
| --- | --- |
| Date and local time | 2026-11-22 (local, UTC+08:00) |
| Desktop host | Windows, `x86_64-pc-windows-msvc`, Rust 1.85.0 |
| Desktop provider | Rustls 0.23.27 with `ring`; `rcgen` 0.13.2 |
| Android target | `sdk_gphone64_x86_64` emulator, Android 17 / API 37, `x86_64` |
| Android provider | `conscrypt-android` 2.6.1, explicitly loaded for the test context |
| Transport route | ADB reverse `tcp:47610`; desktop listener bound to `127.0.0.1` |
| Test execution | Direct `adb install -r` and `adb shell am instrument` |

## Results

| Check | Result |
| --- | --- |
| TLS protocol | TLS 1.3 |
| Exact DER-SPKI pin | Passed |
| Conscrypt exporter length | 32 bytes |
| Role-separated Android proof | Accepted by desktop Rustls |
| Role-separated desktop proof | Verified by Android Conscrypt test |
| Published pairing-proof vector | Passed in the Android instrumentation suite |
| Android keystore and provider spike tests | Passed |
| Interop server result | `PROOFS verified on server`, `RESULT pass` |
| Secret/exporter logging | None observed |

The direct instrumentation run completed four tests successfully, including two
tests in `DesktopInteropTlsTest`. A normal connected test run without the three
interop arguments completed the suite and skipped only the opt-in network test,
as designed.

## Interpretation and limits

This run proves that the current Conscrypt 2.6.1 Android client and Rustls
0.23.27/ring desktop server can complete the pinned TLS 1.3 handshake and
exchange the fixed role proofs over a real TLS socket. It does not prove WLAN
routing, a physical Android device, Android OEM behavior, QR transport, identity
storage, discovery, production pairing, or media interoperability. ADB reverse
keeps the socket on the host/emulator loopback path, so ADR 0021 remains
**Provisional** until a physical-device WLAN run is recorded.

No product pairing capability is enabled by this probe.
