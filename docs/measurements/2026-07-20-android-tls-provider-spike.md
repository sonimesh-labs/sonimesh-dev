# Android TLS provider spike result - 2026-07-20

> Status: local loopback observation only. This result does not select a
> production provider or enable QR pairing, trust persistence, or media
> networking.

## Environment

| Field | Observed value |
| --- | --- |
| Local time | 2026-07-20 22:52 +08:00 |
| Device | vivo V2309A, `arm64-v8a` |
| Android | Android 15 / API 35 |
| Build fingerprint | `vivo/PD2309/PD2309:15/AP3A.240905.015.A1/compiler08221644:user/release-keys` |
| Spike APK | version 0.1.0, version code 1, target SDK 36 |
| TLS provider | `conscrypt-android` 2.6.1, loaded `arm64-v8a` AAR ABI |
| Test runner | Direct `adb install -r` followed by declared AndroidJUnitRunner component |

## Observation

The instrumentation suite completed successfully on the device. The TLS probe
generated one randomized AndroidKeyStore EC alias and short-lived lab
certificate, used an explicit Conscrypt TLS context on a loopback listener, and
cleaned up in the test's `finally` block.

| Check | Result |
| --- | --- |
| Positive exact DER-SPKI pin and proof-of-possession handshake | Passed |
| Negotiated protocol | TLS 1.3 |
| Exporter label/context call | Passed with supplied fixed public 16-byte context |
| Requested exporter length | 32 bytes |
| Client/server exporter comparison | Equal in memory |
| Altered expected SPKI | Client rejected handshake |
| Keystore alias deletion and absence | Passed |
| Listener/thread cleanup | Passed |
| Windows-provider interoperability | Not measured |

No alias, certificate, SPKI, private key, exporter, or pairing input was
printed or recorded. The temporary TLS key authorized both `NONE` and SHA-256
digests: Conscrypt passes TLS 1.3 `CertificateVerify` data to AndroidKeyStore
through `NONEwithECDSA`, while SHA-256 remains required for the requested
self-signed lab certificate.

`connectedDebugAndroidTest` previously stalled in the local Gradle/UTP install
stage on this device with `ShellCommandUnresponsiveException` while installing
split APKs. The same target and instrumentation APKs installed successfully
with direct ADB, and AndroidJUnitRunner executed the assertions above. This is
recorded as a local runner-install limitation, not TLS evidence.

## Interpretation

This demonstrates only the tested AndroidKeyStore and explicit Conscrypt
behavior in a fresh local TLS 1.3 loopback connection. It does not select a
production provider, establish remote-network or Windows interoperability,
define a QR payload, perform a proof exchange, persist trust, or derive media
keys.
