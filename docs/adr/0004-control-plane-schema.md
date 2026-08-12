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
- validation limits display names, capability count, and status/error detail length;
- stream status requires nonzero media/stream identities and a known, non-unspecified
  state; structured errors require a known, non-unspecified code;
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
A framing, semantic, or non-transient socket error returns its structured error
once and then permanently closes both directions of that adapter so partial
read/write state cannot be reused. This is a transport-framing adapter only; it
must remain behind the authenticated TLS/pairing boundary before envelopes can
affect a trusted session.

The version-one protocol implementation selects only its exact protocol version
and constructs a fixed, canonical intersection of known capabilities. Unknown
numeric capability values remain unselected for additive compatibility; they are
never treated as a guessed feature. Unknown numeric stream states and error codes
are rejected under the exact version-one contract because the receiver cannot
safely interpret them. A later schema version must add explicit selection rules
rather than reinterpreting version-one semantics.

Version-one `StartStream` and `StopStream` commands are accepted by the receiver
control coordinator only after a successful hello fixes the control version and
capability set. A start additionally requires its 10 ms or 20 ms duration
capability in that selected intersection. A semantically valid but unnegotiated
stream command does not prepare output or media work and is not reported as a
successful no-op.

The receiver retains exactly one successful authenticated hello per trusted,
ready control session. A hello received before pairing establishes that state is
rejected. A malformed or incompatible ready-session hello leaves that session
unnegotiated so the transport may deliver a corrected authenticated hello; a
second successful hello is rejected rather than silently changing active
capability policy.

When the authenticated control session expires, the receiver retires its
negotiation, active stream identities, pending operation, and local transport token
together. A later paired session must install a fresh authenticated transport and
complete a fresh hello; it cannot inherit a retired peer's capabilities.

Every installed authenticated transport receives an opaque, non-reusable local
session token from a process-wide monotonic sequence. The transport owner
supplies that token with each decoded envelope. A token from another coordinator,
a retired connection, or an unknown session is rejected before envelope
validation, so a delayed task from an old TLS connection cannot submit work into
a replacement session even when its envelope is otherwise valid. Sequence
exhaustion fails closed rather than reusing a token.

Connection loss retires the transport token and any pending platform/media work
immediately. A successful reconnect uses fresh control-session material, returns
to `Ready`, retires the prior negotiation and active media identity, then requires
a newly installed token and hello before another stream command is accepted.

Each prepared start, key rotation, and stop carries a receiver-issued opaque
operation token from a separate process-wide monotonic sequence. The platform
owner must return that operation token with its completion or failure
acknowledgement. Neither local token kind is reused during the process lifetime,
so delayed transport input or a cross-coordinator completion cannot commit
identical work in another or later paired session. Sequence exhaustion fails
closed. Lifecycle state is owned by the coordinator: start and stop transitions
occur only with matching completion tokens, and session expiry uses the dedicated
retirement operation.

A repeated stream command is idempotent only when its command kind and complete
validated payload match pending or completed work. Reusing a media identity with
different sample rate, channel count, or frame duration is rejected rather than
reported as a successful no-op. A start command also cannot alias an in-flight
stop merely because both name the same media identity.

Version-one `StartStream` commands may establish multiple concurrently active
media streams up to a fixed control-plane bound (`MAX_ACTIVE_STREAMS`), each
distinguished by its own media-session and stream identity. The first admitted
stream advances the session from Ready to Streaming; each additional stream keeps
Streaming; the session returns to Ready after the last active stream stops. Key
rotation stays per-stream: a `StartStream` for an already-active identity with a
strictly newer media key epoch rotates only that stream, and other active streams
are unaffected. A stop of one active stream never stops the others.

## Security implications

The local transport and operation tokens are neither secrets nor wire-level
authentication credentials. They only isolate in-process transport and
asynchronous-operation ownership; the TLS/pairing owner must keep each fresh
token scoped to its matching connection or platform task.

The codec does bounded length validation before allocating/decode work. It is not a
transport security layer: every production control frame still requires the
authenticated TLS/pairing context defined by the future security ADR. Unknown
Protobuf fields may be skipped for additive compatibility, but a frame with no
known envelope payload, an unknown status state, or an unknown error code is
rejected.

## Validation evidence

Unit tests must cover frame byte order, truncated/oversized frames, malformed
Protobuf, partial peer closure, permanent socket-error retirement, missing
payload, version mismatch, capability limits, status/error
semantics, process-unique transport and operation tokens, cross-coordinator and
stale-owner rejection, pre-hello start/stop rejection, and exact-payload duplicate
command handling. The published
[control-frame vectors](../protocol/control-frame-vectors.md) fix the initial
Rust/Kotlin wire encoding. A bounded test-only Kotlin/JVM codec independently
encodes and decodes all five Hello, StartStream, StopStream, StreamStatus, and
ControlError vectors and checks length-prefix and semantic bounds. The
control-frame LibFuzzer target also mutates every published payload shape in
addition to arbitrary input. These host
checks are not evidence for an Android control transport or an authenticated TLS
session.

## Rollback plan

All generated code is private to `sonimesh-protocol`; apps depend on its stable
validation API. A future schema version is added through a new `oneof` member or
new message field, while unsupported envelope versions produce a structured
incompatibility error rather than fallback behavior.
