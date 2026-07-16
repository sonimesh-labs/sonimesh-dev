# ADR 0006: Media key epoch control lifecycle

- Status: Accepted for controlled-lab protocol work
- Date: 2026-07-17

## Context

The media AEAD context already binds a 16-bit key epoch into its HKDF input,
nonce, header, and ingress identity. The initial control schema could start or
stop a stream by session and stream ID only, so it could not distinguish a
current stream from a delayed control command for a previous epoch. That would
allow a stale `StopStream` to affect a freshly rotated stream, or leave epoch
installation ambiguous.

First pairing, TLS transcript binding, and root-secret provisioning remain
outside this decision. This record defines only how an authenticated control
session identifies a media key context.

## Decision

`StartStream` and `StopStream` carry `media_key_epoch` as a Protobuf `uint32`.
SoniMesh accepts only values from `0` through `65535` and converts the value to
the protocol's existing `u16` epoch. Epoch zero is valid for the first key of a
new media session.

The exact control identity is the tuple:

~~~text
(media_session_id, stream_id, media_key_epoch)
~~~

- A duplicate `StartStream` with the active tuple is idempotent.
- A `StartStream` for the same session/stream and a different epoch is an
  explicit key rotation. The control domain must construct a fresh media opener,
  replay window, jitter state, decoder state, and PCM epoch before admitting
  packets for the new tuple.
- `StopStream` applies only to the matching active tuple. A stop for an older or
  unknown epoch is idempotently ignored or reported as `STREAM_NOT_FOUND`; it
  must never stop a newer epoch.
- The controlled-lab implementation has no overlapping old/new epoch grace
  window. On a rotation it drops old media packets and installs the new context
  before media admission resumes. A future grace window requires a separate
  bounded multi-epoch design and tests.

Rotation is mandatory before a sender would reuse a media sequence number. The
existing `MediaSealer` refusal after `u32::MAX` remains the final local guard;
control code must request and install the next epoch before that point.

## Security implications

The control frame carries an epoch identifier, never a media key or root secret.
The control session must already be authenticated before this command can affect
media state. A stale or replayed command with a mismatched epoch cannot tear down
the active stream. New epoch state begins with an empty replay window, preventing
replay history from one key context being reused under another key.

## Validation evidence

Protocol tests must reject out-of-range epochs and verify that Start/Stop frames
preserve the exact identity. The future control executor requires transition
tests for duplicate start, stale stop, rotation, and packet rejection before new
context installation. Rust/Kotlin control vectors must include epoch-bearing
commands before Android integration.

## Rollback plan

The field is additive Protobuf schema data but required by semantic validation
for protocol version one after this decision. A compatibility bridge, if ever
needed, must use an explicit new control protocol version rather than silently
defaulting an absent epoch.
