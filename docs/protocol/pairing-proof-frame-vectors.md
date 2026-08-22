# Pairing Proof-Frame Test Vector

This vector fixes the bounded, fixed-length wire frame that carries one
32-byte transcript proof across the pinned TLS 1.3 channel during the ADR
0008 first pairing. It contains intentionally public test values only. It is
not a QR payload, pairing secret, TLS exporter, or evidence that a network
pairing exchange has occurred; the proofs below are the published TLS-binding
vector outputs wrapped in this frame layout.

## Layout

The frame is exactly 55 bytes with no variable-length field:

| Offset | Length | Field |
| --- | --- | --- |
| 0 | 4 | Magic `SMPE` |
| 4 | 1 | Frame version (`01`) |
| 5 | 1 | Control protocol version (`01`) |
| 6 | 1 | Role (`00` Android/client, `01` desktop/server) |
| 7 | 16 | Pairing ID bound into the proof |
| 23 | 32 | The role's transcript proof |

Encoding and decoding are allocation-free and fail closed. Any length other
than exactly 55 bytes, a foreign magic, an unsupported frame version, a
non-version-one control protocol version, or a role byte outside the two
defined roles is rejected before any field is used. `Debug` output redacts
the pairing ID and proof like every other pairing material.

## Expected outputs

The inputs are the published [TLS-binding](./pairing-tls-binding-vectors.md)
vector: pairing ID `000102030405060708090a0b0c0d0e0f` and its two published
role proofs.

Client-role frame (`00`):

~~~text
534d5045010100000102030405060708090a0b0c0d0e0fc274dfe72135ae558034ca1e8a27a4eab7aac9b87a0cd93c17bf17126e27f4f2
~~~

Server-role frame (`01`):

~~~text
534d5045010101000102030405060708090a0b0c0d0e0ff347ab32006e8acb220f91b0e826bbd58257c55773d7284f8794217f896b4601
~~~

A role-swapped mutation of either frame is rejected by the decoder before any
proof comparison, so a client proof cannot be presented as a server proof at
the frame boundary.

## Scope and validation

Each peer sends only its own frame inside the pinned TLS channel after the
handshake; the receiver parses it, checks that the carried pairing ID matches
the scanned invitation, and feeds the proof into the existing pure
`PairingBinding` verification for its local role. The frame adds no trust by
itself: reflection protection, constant-time verification, attempt charging,
and media-root derivation remain owned by the ADR 0008 boundaries.

- The Rust `matches_published_pairing_proof_frame_vector` and
  `decode_round_trips_the_published_vector` tests pin the client frame above,
  round-trip it byte-exactly, and reject wrong lengths, foreign magic,
  unsupported versions, unknown roles, and short output buffers.
- The Kotlin host-JVM `PairingProofFrameVectorTest` reproduces both frames
  through the `PairingProofFrame` mirror with the same rejection behavior.
- The `tools/check_pairing_proof_frame.py` verifier reimplements the layout in
  pure standard-library Python, recomputes both frames from the documented
  inputs, rejects mutated header fields and noncanonical lengths, and
  cross-reads the pins from this document, the Rust source, and the Kotlin
  test; CI runs it in both modes next to the other vector verifiers.
- The byte-driven libFuzzer target `pairing_proof` proves arbitrary bytes
  cannot panic the decoder and every accepted mutation of the published
  vector re-encodes byte-exactly; committed through the finite CI smoke.
