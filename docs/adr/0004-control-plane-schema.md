# ADR 0004: Control-plane envelope and schema evolution

- Status: Accepted for controlled-lab protocol work
- Date: 2026-07-17

## Context

The Android sender and desktop receiver need a low-rate control boundary that is
independent of PCM, Opus, the UI, and realtime scheduling. The architecture calls
for length-prefixed Protobuf envelopes with versioning, request IDs, bounded size,
structured errors, and explicit capabilities. A schema must also survive additive
changes without turning unrecognized input into a guessed command.

Pairing, TLS/session binding, media key derivation, and identity storage remain
separate security decisions. A schema alone does not authenticate a peer.

## Decision

Use `proto3` source in `proto/control.proto` and generate Rust code into
`sonimesh-protocol` during builds.

- each envelope has an exact protocol version and 64-bit request ID;
- a `oneof` selects a typed command or result; an envelope without a known payload
  is rejected by SoniMesh validation;
- frame bytes are prefixed with an unsigned big-endian 32-bit length;
- wire payloads are limited to 64 KiB before Protobuf decoding;
- validation limits display names, capability count, and error detail length;
- `StopStream` is intentionally represented as a typed idempotent command;
- capabilities are enums, not unbounded feature-name strings;
- gain remains local desktop state in the single-source MVP and is absent from the
  wire schema.

The initial schema covers hello/capability exchange, start/stop stream commands,
stream state, and structured errors for controlled-lab work. It contains no keys,
pairing code, IP address, audio data, or device fingerprint.

## Security implications

The codec does bounded length validation before allocating/decode work. It is not a
transport security layer: every production control frame still requires the
authenticated TLS/pairing context defined by the future security ADR. Unknown
Protobuf fields may be skipped for additive compatibility, but a frame with no
known envelope payload is rejected.

## Validation evidence

Unit tests must cover frame byte order, truncated/oversized frames, malformed
Protobuf, missing payload, version mismatch, capability limits, and idempotent
command representation. Cross-language vectors remain required once the Kotlin
sender is implemented.

## Rollback plan

All generated code is private to `sonimesh-protocol`; apps depend on its stable
validation API. A future schema version is added through a new `oneof` member or
new message field, while unsupported envelope versions produce a structured
incompatibility error rather than fallback behavior.

