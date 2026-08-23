# Windows TLS provider spike result

> Status: local lab run; composition evidence for this host only
> Updated: 2026-08-23

One run of `cargo run -p windows-tls-provider-prototype` on the reference
Windows host, following the
[spike protocol](./windows-tls-provider-spike.md) including the in-memory
handshake-composition stage. The run was repeated on 2026-08-23 after the
desktop composition began consuming the ADR 0019 proof frames; the recorded
output below is from that repeat and matches the original stage-for-stage.

| Field | Recorded value |
| --- | --- |
| Date and local time | 2026-08-23 (local) |
| Windows edition/build and architecture | Windows 10 Home China, build 26200 (NT 10.0.26200.0), 64-bit |
| Rust version and host target | rustc 1.85.0 (4d91de4e4 2025-02-17), x86_64-pc-windows-msvc |
| Rustls, provider, and rcgen versions | Rustls 0.23.27 / ring / rcgen 0.13.2 |
| Listener scope | Loopback only, ephemeral port |
| Positive TLS 1.3 handshake | ok |
| Pinned TLS 1.3 signature validation | ok (`CertificateVerify` verified against the generated SPKI pin) |
| Client/server exporter equality | matched, 32 bytes, label `EXPORTER-SoniMesh Pairing v1`, fixed public 16-byte context |
| Requested exporter length | 32 |
| Altered SPKI outcome | rejected before completing the handshake |
| Handshake-composition outcome | ok — rendered invitation, real-exporter Android-role proof admitted by `verify_scanned_proof`, session reached `Ready` |
| Media-root seal equality | matched — the handshake root sealed a datagram identical to an independent derivation |
| Proof-frame exchange outcome | ok — client frame admitted and verified by the desktop, server frame verified by the Android role over one fresh pinned TLS connection (ADR 0019 stage) |
| Server thread/listener cleanup | joined; listener dropped with the owning thread |
| Disk persistence observed | none |
| Android/provider interoperability | not measured |

Full tool output for this run:

~~~text
tls13_handshake: ok
tls13_certificate_verify: ok
tls_version: TLSv1.3
exporter: matched (32 bytes)
handshake_composition: ok
handshake_media_root_seal: matched
proof_frame_exchange: ok (both roles verified)
altered_spki: rejected
~~~

## Interpretation

This run proves that this Rustls/ring configuration can complete the bounded
local TLS 1.3 probe, bind the DER SPKI to the server's TLS 1.3 handshake
signature, export matching 32-byte context-bound material, and satisfy the
pure desktop pairing composition from ADR 0018 in memory on this host: the
live exported bytes drove `begin_from_rendered_payload` /
`verify_scanned_proof` to the trusted outcome with a media root that seals
identically to an independent derivation. The same run also exchanged both
transcript proofs as bounded ADR 0019 frames through one fresh pinned TLS
connection between loopback endpoints, so the fixed frame layout is proven
across a real channel boundary on this host.

It does not prove production QR rendering or scanning, certificate lifecycle,
remote-network behavior, Android TLS compatibility, trust storage, revocation,
rate limiting, or pairing security as a whole. Both frame-exchange roles still
run in one process over loopback. No pairing capability is enabled.
