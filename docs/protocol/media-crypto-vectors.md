# Media Crypto Test Vectors

These vectors fix the version-one media key derivation, nonce layout, header
encoding, associated data, ChaCha20-Poly1305 ciphertext, and tag in one
cross-language assertion. Each vector uses an intentionally public test secret
only. They are not pairing transcripts, production keys, or evidence of Android
support.

HKDF uses the salt and info label defined by
[ADR 0005](../adr/0005-udp-media-protection.md). The resulting 96-bit nonce and
the fixed 36-byte header, which is the AEAD associated data, are listed for each
vector below.

## Vector 1: documented pairing-style constants

### Inputs

| Field | Value |
| --- | --- |
| Control-session secret | `000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f` |
| Protocol version | `1` |
| Direction | Android-to-desktop (`0`) |
| Session ID | `0x0102030405060708` |
| Stream ID | `0x0a0b0c0d` |
| Key epoch | `0x0e0f` |
| Sequence | `0` |
| Flags | `0x02` |
| Sample timestamp | `960` |
| Plaintext UTF-8 | `SoniMesh media vector` |

The 96-bit nonce is `0e0f00000a0b0c0d00000000`.

### Expected protected datagram

```text
534e4d530102002401020304050607080a0b0c0d0000000000000000000003c0
00250e0f7cce62ea960fc27cf5a6ffce2aa59343a5f12997791396ca10bdfc7c
c060c9dfe49fd67131
```

The first 36 bytes are the header; the final 37 bytes are the 21-byte encrypted
plaintext followed by the 16-byte Poly1305 tag.

## Vector 2: canonical cross-language packet vector

### Inputs

| Field | Value |
| --- | --- |
| Control-session secret | `0404040404040404040404040404040404040404040404040404040404040404` |
| Protocol version | `1` |
| Direction | Android-to-desktop (`0`) |
| Session ID | `7` |
| Stream ID | `9` |
| Key epoch | `3` |
| Sequence | `0` |
| Flags | `0x2a` |
| Sample timestamp | `0x1122334455667788` |
| Plaintext UTF-8 | `cross-language packet vector` |

The 96-bit nonce is `000300000000000900000000`.

### Expected protected datagram

```text
534e4d53012a0024000000000000000700000009000000001122334455667788
002c0003337e8052f30b4e20479a0e4080a74511aa39c2deabafa132dbfd6b85
c7c85145360f62f4361efe335f16149a
```

The first 36 bytes are the header; the final 44 bytes are the 28-byte encrypted
plaintext followed by the 16-byte Poly1305 tag.

## Verification status

Implementations must reject a modified byte in either part of either datagram
and must not pass any plaintext to an Opus decoder until authentication
succeeds.

The Rust integration tests `matches_published_cross_language_media_vector`
(vector 1) and `matches_published_cross_language_aead_packet_vector` (vector 2)
in `crates/sonimesh-protocol/tests/media_crypto.rs` reproduce the HKDF info
layout, nonce, fixed header, associated data, ciphertext, and Poly1305 tag of
both vectors. The Kotlin/JVM `MediaCryptoVectorTest` pins vector 1 and also
decrypts the published datagram and rejects independently tampered header and
ciphertext bytes. The `tools/check_media_crypto_vectors.py` verifier
independently reseals both vectors from the documented constants using a pure
standard-library implementation of RFC 8439 ChaCha20-Poly1305 that is itself
validated against the RFC vectors, so a label, salt, key, nonce, or header
change on either side fails CI.

The Rust integration suite additionally pins the rejection matrix at the
protected-media boundary: a datagram that still parses with fewer than 16
ciphertext bytes is rejected as a truncated tag before any plaintext copy, an
empty plaintext round-trips as a tag-only ciphertext exactly on that threshold,
and oversized plaintext or caller-owned output is rejected before decode.

The Kotlin test runs as a host JVM unit test with the configured JDK security
provider. It does not prove Android runtime provider behavior, device support,
ABI packaging, production key handling, or an authenticated pairing source. The
Android production source set remains capture-only.
