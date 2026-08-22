# Pairing QR Payload Test Vector

This vector fixes the binary layout of the version-one pairing QR payload from
[ADR 0008](../adr/0008-first-pairing-and-identity.md) and pins one encoded
example. It contains intentionally public test values only. The pairing secret
here is not a production secret; a real invitation must draw its ID and secret
from a cryptographically secure random source and never reuse them.

The QR payload is presentation transport for the first-pair trust inputs. It is
scanned before any TLS connection exists, so every field length is bounded and
validated before use, and a decoded payload must be rejected unless its
declared lengths exactly match its total size. The desktop SPKI pin carried in
this payload is validated again by the TLS provider spike boundary during the
first connection; this encoding does not establish trust by itself.

## Wire layout

All multi-byte integers are big-endian. There is no length prefix for the
complete payload; the host and SPKI lengths below define it exactly.

| Offset | Size | Field | Notes |
| --- | ---: | --- | --- |
| 0 | 4 | Magic | ASCII `SMQR` |
| 4 | 1 | Payload version | `01` |
| 5 | 1 | Control protocol version | Must equal the supported control version |
| 6 | 1 | Control-host byte length | 1–255 |
| 7 | 2 | Control TCP port | Non-zero |
| 9 | 2 | Desktop TLS SPKI byte length | 32–512 |
| 11 | 16 | Pairing ID | One-time 128-bit value |
| 27 | 32 | Pairing secret | One-time 256-bit value |
| 59 | 8 | Monotonic expiry (ms) | Non-zero |
| 67 | variable | Control host | UTF-8, printable ASCII only (0x20–0x7E) |
| variable | variable | Desktop TLS SPKI | Exact DER bytes pinned for the first TLS 1.3 connection |

The fixed prefix ends at offset 67 (`QR_PREFIX_LEN`). The complete payload is
at most `67 + 255 + 512 = 834` bytes (`MAX_QR_PAYLOAD_LEN`), which keeps a
version-one QR image within the capacity of a low-error-correction code.
Decoding rejects an oversized payload, a foreign magic, an unsupported payload
or protocol version, a zero port or expiry, out-of-range host or SPKI lengths,
any mismatch between declared and actual sizes, and a host containing non-UTF-8
bytes or C0 control characters including DEL.

## Public vector inputs

| Field | Value |
| --- | --- |
| Protocol version | `01` |
| Pairing ID (hex) | `000102030405060708090a0b0c0d0e0f` |
| Pairing secret (hex) | `101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f` |
| Control host | `desktop-01.lab.local` |
| Control port (decimal) | `47610` |
| Desktop TLS SPKI (hex) | `404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f` |
| Monotonic expiry (ms) | `2000000` |

## Expected output

Encoding the inputs above with the documented layout must produce exactly this
byte string:

~~~text
534d5152010114b9fa0030000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f00000000001e84806465736b746f702d30312e6c61622e6c6f63616c404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f
~~~

Decoding that byte string must reproduce every input field exactly, and
re-encoding the decoded view must produce the same bytes. Both the Rust
`pairing_qr` tests (`matches_published_pairing_qr_vector`,
`decode_round_trips_the_published_vector`) and the Android host-JVM
`PairingQrVectorTest` pin these bytes through their mirrors, and
`tools/check_pairing_qr_vectors.py` independently re-encodes and decodes the
vector from the documented layout, so the document, the Rust implementation,
the Kotlin mirror, and the Python verifier cannot drift apart. Implementations
must reject a changed magic, versions, host, port, SPKI length, pairing ID,
secret, or expiry, and must never log a decoded pairing secret or SPKI.
