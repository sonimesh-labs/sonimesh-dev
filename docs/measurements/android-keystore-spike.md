# Android Keystore spike protocol

> Status: instrumentation protocol only; no Android identity store or pairing
> integration is enabled.

## Purpose

Exercise the minimum AndroidKeyStore API behavior required by ADR 0008 before
selecting a production identity-key format. The instrumentation test creates a
fresh randomized EC signing key, signs and verifies fixed non-secret test bytes,
checks that the private-key encoding is unavailable, deletes the alias, and
checks that the alias is absent afterward.

The spike must not generate or retain a QR secret, TLS key, media key, pairing
record, peer identity, or captured PCM. It runs separately from the capture
service and does not make an Android capture, pairing, or compatibility claim.

## Required command

With a documented device or emulator connected, run:

    .\gradlew.bat connectedDebugAndroidTest --no-daemon

The test requires Android API 31 or newer. It is not satisfied by
`testDebugUnitTest`, `assembleDebug`, or an instrumentation APK build without
execution on AndroidKeyStore.

## Required record

| Field | Recorded value |
| --- | --- |
| Date and local time | — |
| Device/emulator model and ABI | — |
| Android release/API/build fingerprint | — |
| App version/build and target SDK | — |
| Keystore provider and key algorithm | — |
| Generate/sign/verify/non-exportability/delete/absence result | — |
| Failure category | `not measured` unless a controlled failure was exercised |
| Alias, key, signature, and test bytes | Never record |

## Interpretation boundary

A successful run demonstrates only the tested AndroidKeyStore lifecycle on that
device or emulator. It does not select an identity encoding, prove hardware
backing or user-authentication policy, establish cross-device migration,
implement pairing, or replace Windows Credential Manager, QR-pinned TLS, or
transcript-proof validation.
