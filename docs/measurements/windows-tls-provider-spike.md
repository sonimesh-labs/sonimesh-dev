# Windows TLS provider spike protocol

> Status: protocol only; no production TLS provider, QR pairing, trust store,
> or network pairing is enabled.

## Purpose

Exercise the Windows-side TLS provider boundary required by ADR 0008 before a
pairing implementation or provider selection. The standalone loopback tool
must prove one fresh TLS 1.3 handshake where the client authenticates the
server from a supplied DER SubjectPublicKeyInfo (SPKI) pin and both sides call
the exact exporter defined by the pairing-binding vector.

The probe is deliberately not a pairing flow. It must not encode or scan a QR
payload, receive an Android connection, exchange pairing proofs, persist an
identity or trust record, derive media keys, advertise discovery, or send media
packets.

## Tool boundary

Run the isolated workspace tool on Windows:

    cargo run -p windows-tls-provider-prototype

The tool uses Rustls `0.23.27` with its `ring` provider and rcgen `0.13.2`,
each under `MIT OR Apache-2.0`, for an in-memory loopback probe only. It creates
a fresh self-signed lab certificate/key, binds a TCP listener to loopback with
an OS-selected port, and joins the accepting server thread before returning.
Neither a certificate, SPKI, private-key DER, exporter value, pairing secret,
nor identifier may be printed or written to disk.

The client verifier has exactly one trust anchor: the expected DER SPKI. It
must reject a changed pin before completing the handshake and must validate the
TLS 1.3 `CertificateVerify` signature using that same pinned SPKI. A temporary
root certificate store, hostname-only trust, or a verifier that returns TLS
signature assertions without verification is not an acceptable substitute.

On the positive connection, client and server each request exactly 32 bytes
using:

| Field | Required value |
| --- | --- |
| TLS version | TLS 1.3 only |
| Exporter label | `EXPORTER-SoniMesh Pairing v1` ASCII |
| Exporter context | Fixed public 16-byte probe context |
| Context supplied | Yes |
| Exporter output | 32 bytes |

The run succeeds only when the two exporter values compare equal in memory and
the altered-pin connection fails. The tool prints only `ok`/`rejected` stage
categories, the negotiated TLS version, and the requested length.

## Required record

| Field | Recorded value |
| --- | --- |
| Date and local time | — |
| Windows edition/build and architecture | — |
| Rust version and host target | — |
| Rustls, provider, and rcgen versions | Rustls 0.23.27 / ring / rcgen 0.13.2 unless changed by a reviewed update |
| Listener scope | Loopback only, ephemeral port |
| Positive TLS 1.3 handshake | — |
| Pinned TLS 1.3 signature validation | — |
| Client/server exporter equality | — |
| Requested exporter length | Must be 32 |
| Altered SPKI outcome | Must be rejected |
| Server thread/listener cleanup | — |
| Disk persistence observed | Must be none |
| Android/provider interoperability | `not measured` |

## Interpretation boundary

A successful result proves only that this Rustls/provider configuration can
complete the bounded local TLS 1.3 probe, bind a DER SPKI to the server's TLS
1.3 handshake signature, and export matching 32-byte context-bound material.
It does not prove a production QR schema, certificate lifecycle, remote-network
behavior, Android TLS compatibility, transcript-proof exchange, trust storage,
revocation, rate limiting, or pairing security as a whole.
