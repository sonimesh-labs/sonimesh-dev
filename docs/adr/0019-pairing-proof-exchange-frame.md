# ADR 0019: Pairing proof-exchange frame boundary

- Status: Provisional
- Date: 2026-08-22

## Context

[ADR 0008](./0008-first-pairing-and-identity.md) fixes the first-pair trust
inputs and the TLS-exporter binding: after the pinned TLS 1.3 handshake, each
peer derives one 32-byte transcript proof and must verify the opposite role's
proof before any media root is released. The pure `PairingBinding` owns that
derivation and constant-time verification, but nothing defined how a proof
travels between two independent implementations inside the established TLS
channel. The Windows provider spike drove both roles in one process by
passing raw byte arrays, so it proved the derivation but not an exchange over
a real channel representation. Without a fixed frame layout, each transport
would invent its own length prefix or delimiter, and an unbounded decoder
would violate the repository rule that every parser validates fixed limits
before copying.

## Decision

A single fixed-length version-one frame is defined in
[pairing-proof-frame-vectors.md](../protocol/pairing-proof-frame-vectors.md)
and implemented in `sonimesh-protocol` as `PairingProofFrame` (bounded encode)
and `parse_pairing_proof_frame` (allocation-free decode):

- exactly 55 bytes: `SMPE` magic, frame version, control protocol version,
  role byte, the 128-bit pairing ID, and one 256-bit transcript proof;
- fail-closed decoding: any length other than exactly 55 bytes, foreign
  magic, unsupported frame version, non-version-one control protocol
  version, or a role byte outside the two defined roles is rejected before
  any field is copied or used;
- redacted `Debug` output for the pairing ID and proof;
- no TLS, no derivation, no verification, no pairing-record consumption, and
  no trust storage. The platform pairing owner wraps its local
  [`PairingBinding`](../../crates/sonimesh-protocol/src/pairing_binding.rs)
  output in this frame, transmits exactly one frame per direction inside the
  pinned TLS channel, parses the received frame, checks that its pairing ID
  matches the scanned invitation, and feeds only the carried proof into the
  existing pure verification. A future manual-code path must not reuse this
  format (ADR 0008).

The frame is deliberately independent of the unauthenticated control-plane
envelope: pairing proofs flow only inside the freshly established pinned TLS
channel, never across the plain TCP framing boundary.

The desktop handshake composition in `sonimesh-app` consumes the frame at its
boundary: `verify_scanned_proof` now takes the peer's raw frame bytes, parses
them with the bounded decoder, and requires the Android-client role byte and
the scanned invitation's pairing ID before any HKDF work or attempt charging;
a verified attempt renders the exact server-role reply frame in its `Ready`
outcome, so a platform owner transmits only a frame derived from the
authenticated transcript and can never skip the identity checks by passing
bare proof bytes.

## Security implications

The frame carries a public transcript-proof value whose secrecy is irrelevant
but whose integrity is everything: substituting another connection's or
another role's proof must fail closed. The exact-length decoder rejects
appended or truncated frames before parsing, the role byte is validated at
the boundary so a client proof cannot be re-presented as a server proof
without an explicit decode error (defense in depth beneath the binding's own
reflection rejection), and the pairing-ID match binds the frame to the
scanned invitation before verification. Reflection protection, constant-time
comparison, attempt charging, and media-root release remain owned by the
existing ADR 0008 boundaries; this decision adds only their wire
representation.

## Validation evidence

- Rust unit tests pin the published client-role vector bytes, round-trip both
  role vectors byte-exactly through the encoder, drive the decoded proofs
  through real `PairingBinding` verification in both directions with a
  reflected proof failing closed, and reject short output buffers, wrong
  lengths (short, empty, trailing), foreign magic, unsupported frame and
  protocol versions, unknown role bytes, and verify redacted `Debug`.
- The desktop handshake composition (`verify_scanned_proof`) unit tests prove
  the boundary behavior: a foreign frame pairing ID, a server-role byte in
  the frame, or any noncanonical frame is rejected before an attempt is
  charged; a canonical client frame with a wrong proof still consumes
  exactly one attempt; and a successful exchange renders a reply frame whose
  carried proof verifies as the desktop server role under the same
  transcript binding.
- The Android host-JVM `PairingProofFrame` mirror and
  `PairingProofFrameVectorTest` reproduce the same golden bytes for both
  roles and the same rejection behavior, so a Kotlin sender cannot drift from
  the Rust receiver-side layout.
- `tools/check_pairing_proof_frame.py` reimplements the layout in pure
  standard-library Python, recomputes both frames from the published
  TLS-binding outputs, rejects mutated magic/version/role fields and
  noncanonical lengths, and cross-reads the pins from the document, the Rust
  source, and the Kotlin test; CI runs it in both modes next to the other
  vector verifiers.
- The byte-driven libFuzzer target `pairing_proof` proves arbitrary bytes
  cannot panic the decoder, every accepted view respects the documented fixed
  layout, and every accepted mutation of the published vector re-encodes
  byte-exactly; committed through the finite CI smoke.
- No network exchange is claimed yet: the spike composition still passes
  proofs as in-process values until its next stage sends these frames over
  the loopback TLS connection.

## Rollback plan

The module is additive and self-contained. Remove
`src/pairing_proof.rs`, its re-exports, the fuzz target registration, the
verifier, the Kotlin mirror and test, the vector document, and this record;
the pairing binding, QR payload codec, pairing record, coordinator, and TLS
provider spikes are unchanged.
