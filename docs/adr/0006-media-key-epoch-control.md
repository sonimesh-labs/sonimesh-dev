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
- A `StartStream` for the same session/stream and a higher epoch is an explicit
  key rotation. The control domain must construct a fresh media opener, replay
  window, jitter state, decoder state, and PCM epoch before admitting packets
  for the new tuple. Lower epochs are stale and rejected.
- `StopStream` applies only to the matching active tuple. A stop for an older or
  unknown epoch is idempotently ignored or reported as `STREAM_NOT_FOUND`; it
  must never stop a newer epoch.
- A fresh media-session installation must use a new `media_session_id`. It
  replaces the ingress and replay window only after the control domain has
  authenticated fresh session material; it must not be used to clear replay
  history for an unchanged session/epoch.
- The controlled-lab implementation has no overlapping old/new epoch grace
  window. On a rotation it drops old media packets and installs the new context
  before media admission resumes. A future grace window requires a separate
  bounded multi-epoch design and tests.

Rotation is mandatory before a sender would reuse a media sequence number. The
existing `MediaSealer` refusal after `u32::MAX` remains the final local guard;
control code must request and install the next epoch before that point.
Epoch values do not wrap within one media session: after `65535`, the sender
must establish a fresh media session and root secret.

## Security implications

The control frame carries an epoch identifier, never a media key or root secret.
The control session must already be authenticated before this command can affect
media state. A stale or replayed command with a mismatched epoch cannot tear down
the active stream. New epoch state begins with an empty replay window, preventing
replay history from one key context being reused under another key.

## Validation evidence

Protocol tests must reject out-of-range epochs and verify that Start/Stop frames
preserve the exact identity. Application transition tests cover duplicate start,
stale stop, and monotonic rotation. Network tests verify a rotation installs a
fresh AEAD opener/replay window and rejects packets from the retired epoch.
Audio tests verify that a matching decode epoch drops retired jitter and PCM
before current audio reaches the output consumer.
Rust and Kotlin/JVM host tests now reproduce the epoch-bearing StartStream
vector and reject an epoch above `65535`. This cross-language wire evidence does
not implement Android control transport or authenticated secret provisioning.

## Rollback plan

The field is additive Protobuf schema data but required by semantic validation
for protocol version one after this decision. A compatibility bridge, if ever
needed, must use an explicit new control protocol version rather than silently
defaulting an absent epoch.
