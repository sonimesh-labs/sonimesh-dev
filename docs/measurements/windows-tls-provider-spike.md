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
payload, receive an Android connection, persist an identity or trust record,
advertise discovery, or send media packets.

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

The run succeeds only when the two exporter values compare equal in memory, the
client and server negotiate the same TLS version, and the altered-pin
connection fails. The tool prints only `ok`/`rejected` stage categories, the
confirmed negotiated TLS version, and the requested length:

```text
tls13_handshake: ok
tls13_certificate_verify: ok
tls_version: TLSv1.3
exporter: matched (32 bytes)
altered_spki: rejected
```

The `tls_version` line is emitted only after both sides independently confirm
TLS 1.3 and the positive connection confirms equal versions. It contains no
certificate, SPKI, private-key, exporter, pairing, or endpoint material.

## In-memory handshake-composition stage

After the transport probe succeeds, the tool may drive the existing pure
desktop-side composition (`sonimesh-app::pairing_handshake`) entirely in
memory with the exporter it just exported:

1. render one invitation through `begin_from_rendered_payload`, using the
   generated SPKI as the pin and fixed non-secret lab fields;
2. derive the Android-role proof from the real exporter with
   `PairingBinding::local_proof(AndroidClient)` in memory;
3. admit that proof through `verify_scanned_proof` as if a scanner had read
   the rendered payload;
4. require the `Ready` outcome and require the derived media root to seal one
   datagram identical to an independent `media_root()` derivation.

This stage proves only that a live Rustls exporter satisfies the pure
composition on this host. It exchanges no network messages: both roles run in
one process over the loopback connection's shared exporter value. It must not
scan an image, spawn a second process, persist anything, or claim Android
interoperability; the altered-pin connection must still fail before this
stage runs, and every failure path above leaves the stage skipped rather than
half-run. Output adds at most these lines:

```text
handshake_composition: ok
handshake_media_root_seal: matched
```

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
| Handshake-composition outcome | `ok` or not reached; never partially run |
| Media-root seal equality | `matched` or not reached |
| Server thread/listener cleanup | — |
| Disk persistence observed | Must be none |
| Android/provider interoperability | `not measured` |

## Interpretation boundary

A successful result proves only that this Rustls/provider configuration can
complete the bounded local TLS 1.3 probe, bind a DER SPKI to the server's TLS
1.3 handshake signature, export matching 32-byte context-bound material, and
satisfy the pure desktop pairing composition in memory on this host. It does
not prove a production QR schema, certificate lifecycle, remote-network
behavior, Android TLS compatibility, transcript-proof exchange over a real
channel, trust storage, revocation, rate limiting, or pairing security as a
whole.
