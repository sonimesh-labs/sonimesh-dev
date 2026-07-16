# Media Crypto Test Vector

This vector fixes the version-one media key derivation, nonce layout, header
encoding, associated data, ChaCha20-Poly1305 ciphertext, and tag in one
cross-language assertion. It uses an intentionally public test secret only.
It is not a pairing transcript, production key, or evidence of Android support.

## Inputs

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

HKDF uses the salt and info label defined by
[ADR 0005](../adr/0005-udp-media-protection.md). The resulting 96-bit nonce is
`0e0f00000a0b0c0d00000000`. The fixed header is the AEAD associated data.

## Expected protected datagram

```text
534e4d530102002401020304050607080a0b0c0d0000000000000000000003c0
00250e0f7cce62ea960fc27cf5a6ffce2aa59343a5f12997791396ca10bdfc7c
c060c9dfe49fd67131
```

The first 36 bytes are the header; the final 37 bytes are the 21-byte encrypted
plaintext followed by the 16-byte Poly1305 tag. Implementations must reject a
modified byte in either part and must not pass any plaintext to an Opus decoder
until authentication succeeds.
