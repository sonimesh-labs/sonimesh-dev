# Windows credential-store spike protocol

> Status: protocol only; no production credential store or pairing integration
> is enabled.

## Purpose

Exercise the Windows Credential Manager API boundary required by ADR 0008 before
selecting a production format for desktop identity keys or pairing records. The
spike verifies one temporary Generic Credential lifecycle in the current user
session: write, read, byte-for-byte validation, delete, and read-after-delete.
Both write and read paths reject a credential blob over the fixed 64 KiB
prototype bound before copying it.

The test is not a pairing implementation, cryptographic key test, or proof that
Credential Manager is the final store. It must not accept, generate, print, or
persist a QR payload, pairing secret, private key, trust record, session ticket,
or media key.

## Tool boundary

Run the standalone Windows-only experiment:

    cargo run -p windows-credential-prototype

The tool creates a fresh opaque target from 128 random bits (16 bytes)
supplied by the Windows system-preferred CNG RNG, then stores a fixed non-secret
test blob with
`CRED_PERSIST_SESSION`. It fails before any Credential Manager operation when
that RNG call fails; it does not fall back to a timestamp, process counter, or
other predictable target. It prints only stage success/failure, byte length, and
the post-delete error category. It must not print the target, username,
credential bytes, or any stable local identifier.

The tool first removes a stale credential with its generated target, then uses a
cleanup guard so every path after a successful write attempts deletion. A run is
successful only when the readback matches, cleanup succeeds, and the final read
returns the Windows `not found` category.

## Required record

| Field | Recorded value |
| --- | --- |
| Date and local time | — |
| Windows edition/build and architecture | — |
| Rust and windows crate versions | — |
| API path | Credential Manager Generic Credential, session persistence only |
| Test blob length | — |
| Write/read/validation/delete result | — |
| Read-after-delete category | Must be `not found` for a successful run |
| Cleanup result | — |
| Store/profile failure observation | `not measured` unless an explicit failure-path run was performed |

## Interpretation boundary

A successful result proves only that the current Windows user session allowed
this bounded temporary Credential Manager lifecycle. It does not prove
long-term persistence semantics, cross-user access, backup/restore behavior,
DPAPI protection details, Android Keystore compatibility, identity migration,
revocation, QR-pinned TLS, transcript binding, or pairing security.
