# ADR 0005: UDP media protection and nonce lifecycle

- Status: Accepted for controlled-lab media transport
- Date: 2026-07-17

## Context

UDP media packets are observable, mutable, replayable, and reorderable on a local
network. The media header already carries a session ID, stream ID, sequence, key
epoch, and payload bounds, but it needs a reviewed key derivation and nonce
construction before any encrypted transport can be trusted.

This decision does not define first pairing, TLS, certificate pinning, identity
storage, or control-session transcript creation. Those mechanisms supply the
authenticated root secret required below.

## Decision

Use ChaCha20-Poly1305 with 256-bit per-session/per-stream/per-direction/per-epoch
keys. The key is derived using HKDF-SHA256:

~~~text
root = HKDF-Extract("sonimesh media root v1", authenticated control-session secret)
info = "sonimesh media stream key v1"
       || protocol_version_u8
       || session_id_be_u64
       || stream_id_be_u32
       || key_epoch_be_u16
       || direction_u8
key = HKDF-Expand(root, info, 32)
~~~

Direction is `0` for Android-to-desktop and `1` for desktop-to-Android. The
control-session secret is never logged or serialized by this API.

Each packet nonce is exactly 96 bits:

~~~text
key_epoch_be_u16 || direction_u8 || 0x00 || stream_id_be_u32 || sequence_be_u32
~~~

The session ID is bound into the derived key and authenticated header; stream ID,
epoch, direction, and sequence are additionally represented in the nonce. A sender
owns a monotonic sequence counter, may send `u32::MAX` exactly once, then refuses
further packets until a new key epoch/context is installed.

The full encoded header, including any validated future extension bytes, is AEAD
associated data. Decryptors verify session, stream, and epoch context before
decryption. They write plaintext only to caller-owned storage and return no bytes
on authentication failure. Replay admission remains in `sonimesh-network` and
occurs only after this successful authentication.

## Security implications

Nonce uniqueness depends on a fresh authenticated control-session secret when a
session is recreated and on key epoch rotation before sender sequence reuse. This
API cannot make a caller-provided root secret authentic; callers must only obtain it
from the future reviewed pairing/TLS transcript.

Packets with wrong context, truncated tags, invalid tags, or insufficient output
storage are rejected before decoder delivery. Keys use zeroizing storage and are
not `Debug` or serializable. The implementation does not log plaintext, packet
payloads, or secrets.

## Validation evidence

Tests cover HKDF determinism/domain separation, nonce separation by every field,
header tampering, ciphertext tampering, context mismatch, output bounds, sequence
exhaustion, and authenticated replay ordering through the network adapter.

Cross-language Rust/Kotlin crypto vectors, key rotation control messages, address
migration, and fuzzing remain required before a build leaves the lab network.

## Rollback plan

Media protection is isolated behind the protocol key context and opener/sealer
types. A future reviewed algorithm version uses a new protocol/key-derivation
label and epoch, not a silent replacement under the same wire version.

