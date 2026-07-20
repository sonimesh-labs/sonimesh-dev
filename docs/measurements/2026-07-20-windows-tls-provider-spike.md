# Windows TLS provider spike result - 2026-07-20

> Status: local loopback observation only. This result does not enable QR
> pairing, Android interoperability, trust storage, or non-laboratory network
> operation.

## Environment

| Field | Observed value |
| --- | --- |
| Local time | 2026-07-20 13:49:27 +08:00 |
| Windows | Windows 11 Home China, version 10.0.26200, build 26200, 64-bit |
| Rust | rustc/cargo 1.85.0, host `x86_64-pc-windows-msvc` |
| Rustls/provider | Rustls 0.23.27 with the `ring` provider |
| Certificate generator | rcgen 0.13.2 |
| SPKI parser | x509-parser 0.18.1 |
| Listener | `127.0.0.1` only, ephemeral port |

## Procedure and result

Executed:

    cargo test -p windows-tls-provider-prototype --all-targets --all-features
    cargo run -p windows-tls-provider-prototype

The unit test suite passed 2/2. The local binary reported the following public
stage categories:

| Check | Result |
| --- | --- |
| Fresh in-memory certificate and key | Created for the process-local probe |
| Positive DER-SPKI-pinned handshake | Passed |
| Negotiated protocol | TLS 1.3 |
| TLS 1.3 `CertificateVerify` against the pinned SPKI | Passed |
| Exporter label/context call | Passed with a supplied fixed 16-byte public context |
| Requested exporter length | 32 bytes |
| Client/server exporter comparison | Equal in memory |
| Altered expected SPKI | Handshake rejected |
| Server thread/listener lifecycle | Joined before the process returned |
| Tool-level disk persistence | No filesystem write path exists in the tool; none observed |

No certificate DER, SPKI, private-key DER, exporter material, pairing secret,
or local identifier was printed by the tool.

## Interpretation

This confirms that the pinned-verifier construction used by this isolated
Rustls/ring probe can validate possession of the exact pinned SPKI in a fresh
TLS 1.3 loopback handshake, and that both peers can obtain matching 32-byte
exporter output with the ADR 0008 label and an explicit context.

It does not establish a production TLS provider selection, external network
behavior, a QR payload format, certificate lifecycle, Android provider support,
pairing-proof exchange, identity/trust persistence, revocation, or rate-limit
behavior. The required Android provider spike remains unmeasured.
