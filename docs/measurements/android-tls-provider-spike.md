# Android TLS provider spike protocol

> Status: instrumentation protocol only; no QR pairing, trust store, media
> networking, or production TLS-provider selection is enabled.

## Purpose

Exercise the Android TLS-provider boundary required by ADR 0008 on an API 31+
device or emulator. The spike must prove a fresh loopback TLS 1.3 connection in
which the Android client accepts only the expected DER SubjectPublicKeyInfo
(SPKI), validates the server's proof of possession through the TLS handshake,
and calls the exact exporter required by the pairing-binding vector.

The test uses `org.conscrypt:conscrypt-android:2.6.1` only as an isolated
instrumentation dependency. Its Maven metadata identifies Apache-2.0 licensing;
its AAR contains `arm64-v8a`, `x86_64`, `armeabi-v7a`, and `x86` native
libraries and declares minimum API 21. The project test remains limited to API
31 or newer, matching the Android MVP baseline.

## Required command

With a documented device or emulator connected, run:

    .\gradlew.bat connectedDebugAndroidTest --no-daemon

`testDebugUnitTest`, an instrumentation APK build, or a host-only loopback does
not establish Android provider evidence.

## Tool boundary

The instrumentation test creates a fresh AndroidKeyStore EC signing key under
a randomized alias and requests its short-lived self-signed lab certificate
through `KeyGenParameterSpec` certificate fields. The server binds a listener
to loopback with an OS-selected port and uses that temporary AndroidKeyStore
entry; the client uses a test-only trust manager that compares the presented
end-entity DER SPKI to the expected pin. The test forces TLS 1.3 and uses the
temporary certificate/key only for that probe. The alias must be deleted in a
`finally` block and its absence asserted before the test returns.

After both sides complete the TLS 1.3 handshake, they each call Conscrypt's
public exporter with:

| Field | Required value |
| --- | --- |
| Exporter label | `EXPORTER-SoniMesh Pairing v1` ASCII |
| Exporter context | Fixed public 16-byte probe context |
| Context supplied | Yes |
| Requested output | 32 bytes |

The positive path succeeds only when the exporter values are equal in memory.
A second client connection using an altered expected SPKI must fail the
handshake. The test must not use a temporary root store, hostname-only trust,
an assertion-only trust manager, or the device's ambient default TLS provider.

It must not log or persist the certificate, SPKI, private key, exporter,
alias, pairing secret, QR payload, or local endpoint identifier. It must not
touch MediaProjection, audio capture, network discovery, pairing records,
identity/trust storage, proof exchange, media keys, Opus, or the output path.

## Required record

| Field | Recorded value |
| --- | --- |
| Date and local time | — |
| Device/emulator model and ABI | — |
| Android release/API/build fingerprint | — |
| App version/build and target SDK | — |
| Conscrypt version and AAR ABI | 2.6.1 / observed loaded ABI |
| TLS version | Must be TLS 1.3 |
| Positive DER-SPKI pin and handshake | — |
| Client/server exporter equality | — |
| Requested exporter length | Must be 32 |
| Altered SPKI outcome | Must be rejected |
| Keystore alias deletion/absence | — |
| Listener/thread cleanup | — |
| Windows-provider interoperability | `not measured` |
| Alias, certificate, SPKI, private key, exporter, pairing input | Never record |

## Interpretation boundary

A successful run proves only the tested AndroidKeyStore, Conscrypt, and local
TLS 1.3 behavior on that recorded device/emulator. It does not select a
production TLS provider, prove remote-network behavior or cross-language
interoperability, establish a QR schema, perform pairing, persist a trust
record, derive a media key, or replace the required Windows/provider and
pairing lifecycle evidence.
