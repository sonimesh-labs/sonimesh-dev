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

The repository also provides an untrusted, non-blocking TCP reader/writer that
incrementally collects the four-byte length prefix and bounded payload before
calling the same frame validator, and queues at most one bounded outgoing frame.
A framing or semantic error terminates that connection. This is a transport-
framing adapter only; it must remain behind the authenticated TLS/pairing
boundary before envelopes can affect a trusted session.

The version-one protocol implementation selects only its exact protocol version
and constructs a fixed, canonical intersection of known capabilities. Unknown
numeric capability values remain unselected for additive compatibility; they are
never treated as a guessed feature. A later schema version must add explicit
selection rules rather than reinterpreting a version-one hello.

A version-one `StartStream` is accepted by the receiver control coordinator only
when its 10 ms or 20 ms duration capability appears in that selected
intersection. A semantically valid but unnegotiated command does not prepare
output or media work.

The receiver retains exactly one successful authenticated hello per trusted,
ready control session. A hello received before pairing establishes that state is
rejected. A malformed or incompatible ready-session hello leaves that session
unnegotiated so the transport may deliver a corrected authenticated hello; a
second successful hello is rejected rather than silently changing active
capability policy.

When the authenticated control session expires, the receiver retires its
negotiation, active stream identity, and pending operation together. A later
paired session must complete a fresh hello; it cannot inherit a retired peer's
capabilities.

A successful reconnect also uses fresh control-session material. The receiver
returns to `Ready` while retiring the prior negotiation, active media identity,
and pending completion; the reconnected peer must send a new hello before it
can issue another stream command.

Each prepared start, key rotation, and stop carries a receiver-issued opaque
operation token. The platform owner must return that token with its completion
or failure acknowledgement. Tokens are never reused during the coordinator's
lifetime, including after control-session expiry, so a delayed completion cannot
commit an identical command in a later paired session. Lifecycle state is owned
by the coordinator: start and stop transitions occur only with matching
completion tokens, and session expiry uses the dedicated retirement operation.

## Security implications

The codec does bounded length validation before allocating/decode work. It is not a
transport security layer: every production control frame still requires the
authenticated TLS/pairing context defined by the future security ADR. Unknown
Protobuf fields may be skipped for additive compatibility, but a frame with no
known envelope payload is rejected.

## Validation evidence

Unit tests must cover frame byte order, truncated/oversized frames, malformed
Protobuf, missing payload, version mismatch, capability limits, and idempotent
command representation. The published
[control-frame vector](../protocol/control-frame-vectors.md) fixes the initial
Rust/Kotlin wire encoding. A bounded test-only Kotlin/JVM codec independently
encodes and decodes the Hello and epoch-bearing StartStream vectors and checks
length-prefix bounds. It is not evidence for an Android control transport or an
authenticated TLS session.

## Rollback plan

All generated code is private to `sonimesh-protocol`; apps depend on its stable
validation API. A future schema version is added through a new `oneof` member or
new message field, while unsupported envelope versions produce a structured
incompatibility error rather than fallback behavior.
