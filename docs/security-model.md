# SoniMesh Security Model

This document defines the security boundary for the local-network control and
audio planes. A local network is treated as hostile: another device may observe,
spoof, replay, flood, or mutate traffic.

## Threat model

An attacker may:

- observe packets on the WLAN;
- impersonate a desktop or Android device before pairing;
- inject, replay, reorder, or corrupt UDP packets;
- brute-force a pairing code;
- send oversized, malformed, or high-rate control messages;
- send authenticated but malicious Opus payloads from a compromised paired device;
- exploit parser, decoder, or logging bugs;
- read local logs or discoverable device metadata.

The system does not protect against a fully compromised Android device, a fully
compromised desktop, or an attacker who controls the operating system audio
stack. It does protect the network boundary and limits the damage of malformed
input.

## Trust and first pairing

Discovery is unauthenticated. A discovered device is only a candidate.

The preferred pairing flow is QR-based:

1. The desktop creates a short-lived pairing record.
2. The QR payload contains the desktop identity public key, a control address,
   and a one-time high-entropy pairing secret.
3. Android connects using TLS and pins the public key from the QR payload.
4. Both sides use the fixed TLS 1.3 exporter label/context and role-separated
   HKDF proofs from the published pairing TLS-binding vector.
5. The devices store each other's long-term public key and a user-visible trust
   record.

The manual-code flow must use a PAKE or a transcript-bound short-authentication
string compared on both screens. A code that is merely entered on Android and
forwarded through an untrusted channel is not sufficient.

The QR payload, TLS pinning, transcript-binding requirement, identity storage,
and revocation boundary are specified by
[ADR 0008](./adr/0008-first-pairing-and-identity.md). The bounded version-one
QR byte layout is now implemented and pinned by
[ADR 0018](./adr/0018-pairing-qr-payload-encoding.md) and the
[pairing QR vector](./protocol/pairing-qr-vectors.md); the platform pairing
owner that renders and scans it remains unimplemented.

The pairing TLS-binding vector fixes a TLS 1.3 exporter call with QR `pairing_id`
as context, followed by HKDF-SHA256 role proofs and media-root derivation. A
provider that cannot satisfy the exact exporter call or QR SPKI pin must fail
closed. It must not fall back to a locally invented transcript hash, a discovery
record, or a one-sided code.

Pairing records need an expiry, attempt limit, per-source rate limit, and
explicit revocation. The desktop should default to allowing only “send audio”;
future administrative permissions must be separate capabilities.

## Long-term identity storage

- Android identity keys belong in Android Keystore-backed storage.
- Windows identity keys and trust records belong in DPAPI/Credential Manager or
  an equivalent OS-protected store.
- Device display names are untrusted presentation data.
- Revocation removes the peer key, session tickets, and any pending pairing data.

## Session keys and UDP packets

After pairing, the control session authenticates the peers. Media keys should be
derived from the authenticated transcript with HKDF and explicit labels. Do not
write media keys into ordinary control logs or crash reports.

Recommended derivation shape:

~~~text
root = HKDF-Extract(pairing/session secret, authenticated transcript)
direction_key = HKDF-Expand(root, "sonimesh media v1" || direction, ...)
stream_key = HKDF-Expand(direction_key, stream_id || epoch, ...)
~~~

The exact byte encoding is specified in the protocol document and tested in
Rust and a Kotlin/JVM unit test. The Kotlin test is host-provider evidence only;
an Android-runtime provider test remains required. A nonce must be unique for
every packet under a key. The implementation uses an explicit 96-bit layout
whose fields include the key epoch, stream identity, direction, and sequence.

The controlled-lab Rust implementation records this exact derivation, nonce
layout, sequence exhaustion behavior, and ChaCha20-Poly1305 choice in
[ADR 0005](./adr/0005-udp-media-protection.md). It accepts only a caller-supplied
root secret and does not claim to implement pairing, TLS transcript binding, or
identity storage.

Before a source endpoint is known, packets are accepted only after:

1. length and minimum-header checks;
2. bounded header parsing;
3. session and stream lookup;
4. AEAD authentication with the complete header as associated data;
5. replay-window validation and update;
6. a bounded jitter admission decision;
7. source-endpoint pinning after the protected pipeline returns an outcome.

Once pinned, a datagram from another endpoint is rejected before parsing or
cryptographic work and cannot advance replay state. Unknown sessions and
unauthenticated payloads must never reach Opus or pin the source endpoint.
Replay window state must not advance for failed authentication attempts.

Key epochs need an explicit rotation and grace-period protocol. The controlled-
lab Start/Stop epoch identity and no-overlap policy are defined by
[ADR 0006](./adr/0006-media-key-epoch-control.md). Sequence wrap, session resume,
and control-channel expiry must all result in new nonce/key material rather than
silently reusing old state. Automatic address migration is disabled: epoch
rotation keeps the pinned endpoint, while a successfully installed fresh media
session clears it so a newly authenticated source can bind.

## Abuse controls

- Bound every control message, packet, queue, stream count, and pairing attempt.
- Rate-limit new TCP/TLS connections and admit pairing proof verification only
  after bounded per-source checks have charged the attempt.
- Drop stale UDP packets before expensive work when possible, without advancing
  authenticated replay state.
- Fuzz the packet parser and the authenticated Opus boundary. Authentication does
  not make a paired peer benign.
- Keep the decoder and parser libraries current and isolate decoder failures to a
  single stream.
- Do not expose stable public-key fingerprints in discovery records unless the
  privacy trade-off is explicitly accepted.

## Privacy

The default behavior is no recording, no disk persistence of audio, and no cloud
upload. Diagnostics redact full IP addresses, long-term identifiers, keys, and
audio content. User-exported diagnostic bundles must be explicit, bounded, and
redacted before writing to disk.
