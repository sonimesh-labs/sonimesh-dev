# ADR 0018: Pairing QR payload encoding boundary

- Status: Provisional
- Date: 2026-08-22

## Context

[ADR 0008](./0008-first-pairing-and-identity.md) fixes the first-pair trust
inputs: a one-time pairing ID, a one-time pairing secret, the desktop control
endpoint, and the desktop TLS SPKI pin. The pairing TLS-binding vector pins
how those inputs become proofs and the media root after a pinned TLS 1.3
handshake, and the bounded pairing record owns admission. The remaining
unimplemented step in that chain was the binary payload itself: how an Android
scanner turns a desktop-rendered QR image into exactly those validated bytes.
Nothing constrained the on-QR representation, so two implementations could not
agree on a scanned invitation, and an unbounded decoder would have violated
the repository rule that every parser validates fixed limits before copying.

## Decision

A single versioned QR wire format is defined in
[pairing-qr-vectors.md](../protocol/pairing-qr-vectors.md) and implemented in
`sonimesh-protocol` as `PairingQrPayload` (bounded encode) and
`parse_pairing_qr_payload` (zero-copy decode):

- a 67-byte fixed prefix (`SMQR` magic, payload version, control protocol
  version, host and SPKI length fields, port, pairing ID, pairing secret,
  monotonic expiry) followed by the UTF-8 printable host and the exact DER
  SPKI;
- hard bounds of 255 host bytes and 32–512 SPKI bytes with a maximum complete
  payload of 834 bytes, so a low-error-correction QR image stays within its
  capacity;
- fail-closed decoding: foreign magic, unsupported versions, zero port or
  expiry, out-of-range lengths, any declared-versus-actual size mismatch, and
  non-printable or non-UTF-8 hosts are all rejected before any field is used;
- the decoded view borrows the caller's buffer for the secret and SPKI, so
  ownership and zeroization stay with the scanner's storage; `Debug` output
  redacts every sensitive field.

The codec performs no TLS, no pairing-record consumption, no proof exchange,
and no trust storage. The platform pairing owner feeds a decoded view into the
existing boundaries: the SPKI pin goes to the TLS provider spike verifier, the
ID/secret go to the pure pairing binding, and the expiry seeds the pairing
record. A future manual-code path must not reuse this format (ADR 0008).

## Security implications

The QR payload carries the one-time pairing secret through a visible channel,
so screen visibility remains security-sensitive exactly as ADR 0008 states.
Bounding and validating before use prevents a hostile or corrupted scan from
driving unbounded allocation or from presenting control characters as a host.
Borrowing the secret keeps it out of additional copies the codec might
otherwise retain, and redacted `Debug` output keeps it out of logs and crash
reports. The payload confers no trust by itself: the SPKI pin is enforced by
the TLS handshake verifier and the secret is consumed only inside the pinned
TLS channel through the transcript-bound proof.

## Validation evidence

- Rust unit tests cover the published-vector encode, decode round trip,
  oversized/truncated rejection, foreign magic, both version fields, zero
  port/expiry, short and long SPKI declarations, length mismatch, non-UTF-8
  hosts, boundary lengths (1-byte host at minimum SPKI, 255-byte host at
  maximum SPKI filling exactly 834 bytes), short output buffers, and redacted
  `Debug`.
- The Android host-JVM `PairingQr` mirror and `PairingQrVectorTest` reproduce
  the same golden bytes and the same rejection behavior, so a Kotlin sender
  cannot drift from the Rust receiver-side layout.
- The Android host-JVM `PairingHandshake` test-source model pins the client-role
  composition over that decoder and binding: a canonical invitation derives
  the published Android proof, the desktop server proof unlocks the published
  media root, reflected or wrong proofs and changed exporters fail closed, and
  foreign scans, corrupted scans, or noncanonical exporters are rejected before
  proof derivation. It remains isolated from TLS, camera capture, attempt
  limiting, trust storage, production code paths, and any pairing capability.
- `tools/check_pairing_qr_vectors.py` reimplements the layout in pure
  standard-library Python, re-encodes and decodes the published vector,
  rejects mutated magic/version/length fields, and cross-reads the pins from
  the document, the Rust source, and the Kotlin test; CI runs it in both
  modes next to the other vector verifiers.
- The byte-driven libFuzzer target `pairing_qr` proves arbitrary bytes cannot
  panic the decoder, every accepted view respects the documented bounds, and
  every accepted mutation of the published vector re-encodes byte-exactly;
  committed through the finite CI smoke.
- The `sonimesh-app` pairing handshake composition
  (`begin_from_rendered_payload` / `verify_scanned_proof`) now drives the
  encoded payload through the real decoder, the real `PairingBinding`
  transcript, and the real invitation owner: unit tests prove the rendered
  payload round-trips, a correct client proof reaches `Ready` with a media
  root that seals identical datagrams to an independent derivation, a
  reflected server proof is rejected without consuming the invitation, a
  foreign scan, corrupted scan, short exporter, or expired invitation each
  fail closed before charging an attempt, and integration tests carry the
  resulting root through `MediaSealer`/`MediaOpener` and the
  `MediaSessionAllocator` no-reuse record. The exporter is modeled in the
  crate tests; the Windows TLS provider spike additionally drives this same
  composition with a real exported value from its pinned TLS 1.3 loopback
  connection (see the spike protocol's in-memory handshake-composition
  stage), so the composition is proven against a live provider export on
  that host.
- No physical device, TLS provider, or camera result is claimed: this ADR
  covers only the bounded byte representation between a rendered QR image and
  the existing pure pairing boundaries.

## Rollback plan

The module is additive and self-contained. Remove `src/pairing_qr.rs`, its
re-exports, the fuzz target registration, the verifier, the Kotlin mirror and
test, and this record; the pairing binding, pairing record, coordinator, and
TLS provider spikes are unchanged.
