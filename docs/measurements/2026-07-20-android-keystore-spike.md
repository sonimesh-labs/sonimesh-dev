# Android Keystore spike result - 2026-07-20

> Status: API-level feasibility observation only. This result does not select an
> Android identity format or enable pairing, trust storage, or capture.

## Environment

| Field | Observed value |
| --- | --- |
| Local time | 2026-07-20 22:52 +08:00 |
| Device | vivo V2309A, `arm64-v8a` |
| Android | Android 15 / API 35 |
| Build fingerprint | `vivo/PD2309/PD2309:15/AP3A.240905.015.A1/compiler08221644:user/release-keys` |
| Spike APK | version 0.1.0, version code 1, target SDK 36 |
| Key provider and algorithm | AndroidKeyStore EC signing key |
| Test runner | Direct `adb install -r` followed by declared AndroidJUnitRunner component |

## Observation

The instrumentation suite completed successfully on the device. The Keystore
test created a randomized alias, generated an EC signing key, signed and
verified fixed non-secret test bytes, observed no private-key encoding, deleted
the alias, and confirmed absence afterward.

| Check | Result |
| --- | --- |
| Generate/sign/verify | Passed |
| Private-key non-exportability | Passed |
| Delete and confirm absence | Passed |
| Controlled failure category | Not measured |
| Sensitive key, alias, signature, or test bytes in record | Not recorded |

`connectedDebugAndroidTest` previously stalled in the local Gradle/UTP install
stage on this device with `ShellCommandUnresponsiveException` while installing
split APKs. The same target and instrumentation APKs installed successfully
with direct ADB, and AndroidJUnitRunner executed the assertions above. This is
recorded as a local runner-install limitation, not an AndroidKeyStore result.

## Interpretation

This demonstrates the tested AndroidKeyStore lifecycle on this device only. It
does not establish hardware backing or authentication policy, select a
long-term identity encoding, implement TLS pairing, or prove migration,
revocation, or Windows interoperability.
