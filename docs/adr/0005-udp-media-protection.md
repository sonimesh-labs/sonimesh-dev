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

The isolated Android audio lab may exercise this packet format over the Android
emulator's host-loopback route or a controlled local network. For that experiment only, Android generates a
fresh random secret and passes it over an ADB-reversed loopback TCP connection;
the connection remains open for the session lifetime. This bootstrap is not
authenticated pairing or a product control plane. The desktop UDP receiver binds
to loopback by default; a LAN bind must be explicitly selected for the lab.
Each encoded frame gets its sequence before bounded-queue admission
so a dropped frame remains a visible gap. The desktop uses the existing protected
UDP receiver and per-stream decoder, not a bypass around authentication.

The UDP receiver composition starts without a trusted source endpoint. The first
packet that completes authentication, replay admission, and a bounded jitter
decision pins its observed source address and port. An unauthenticated or
otherwise rejected packet cannot pin the endpoint. Once pinned, a changed source
is rejected before parsing or authentication and cannot mutate replay state.
Epoch rotation preserves the pin. Only successful installation of a fresh media
session clears it; the next successfully admitted packet then establishes the
new endpoint. There is no automatic same-session address migration.

## Security implications

Nonce uniqueness depends on a fresh authenticated control-session secret when a
session is recreated and on key epoch rotation before sender sequence reuse. This
API cannot make a caller-provided root secret authentic; callers must only obtain it
from the future reviewed pairing/TLS transcript.

Packets with wrong context, truncated tags, invalid tags, insufficient output
storage, or an unexpected source endpoint are rejected before decoder delivery.
The first-packet pin occurs only after successful admission, preventing an
unauthenticated datagram from locking out the real sender. A legitimate network
address or port change intentionally stops media until the authenticated control
plane installs a fresh session; this fail-closed interruption is preferred over
silent key-state reuse. Keys use zeroizing storage and are not `Debug` or
serializable. The implementation does not log plaintext, packet payloads, or
secrets.

## Validation evidence

Tests cover HKDF determinism/domain separation, nonce separation by every field,
header tampering, ciphertext tampering, context mismatch, output bounds, sequence
exhaustion, authenticated replay ordering through the network adapter, and the
published [cross-language media crypto vector](../protocol/media-crypto-vectors.md).
The source-pin state machine has deterministic unit coverage, and a virtual-
endpoint integration test applies it around the protected receiver pipeline to
show that rejected packets cannot pin, a changed source cannot advance replay
state, epoch rotation preserves the pin, failed fresh-session installation
preserves it, and successful fresh-session installation permits rebinding.
Loopback UDP tests exercise the same behavior through a retained host socket.
A Kotlin/JVM unit test independently reproduces the public crypto vector,
decrypts it, and rejects header or ciphertext tampering using the host JDK
provider. A LibFuzzer protected-ingress target exercises arbitrary datagrams,
valid authenticated bounded payloads, duplicate replay, undersized output,
and one-byte header/ciphertext/tag mutations while asserting that rejection
clears plaintext. Linux CI is configured for a finite AddressSanitizer smoke
run.

The Android-to-desktop vector also passes on an API 37 x86_64 emulator using the
Android runtime provider. Physical-device provider coverage, authenticated
control-plane session replacement, key rotation control messages, sustained
fuzzing, and future native Opus-decoder fuzzing remain open. Emulator, loopback,
finite fuzz, and host JVM results are not physical WLAN or production
key-handling evidence.

## Rollback plan

Media protection is isolated behind the protocol key context and opener/sealer
types. A future reviewed algorithm version uses a new protocol/key-derivation
label and epoch, not a silent replacement under the same wire version.
