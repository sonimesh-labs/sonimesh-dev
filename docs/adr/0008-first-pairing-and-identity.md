# ADR 0008: First pairing and device identity boundary

- Status: Accepted design; implementation remains gated by platform spikes
- Date: 2026-07-17

## Context

Discovery is unauthenticated and a local network is hostile. The media AEAD API
deliberately accepts a caller-supplied root secret, but cannot establish whether
that secret belongs to the intended Android device and desktop. A one-sided
six-digit code forwarded over an untrusted connection is relayable and does not
meet the security model.

The Windows WLAN MVP needs an explicit first-pair trust boundary before it can
leave a controlled lab. Android and Windows also require different OS-protected
storage integrations that cannot be substituted by ordinary app files.

## Decision

The initial pairing method is QR-first. The desktop creates a single-use,
short-lived pairing record and QR payload containing:

~~~text
protocol_version
pairing_id: 128-bit random value
control_endpoint: host + port
desktop_tls_spki: DER SubjectPublicKeyInfo bytes
pairing_secret: 256-bit random value
expires_at: monotonic expiry metadata
~~~

Android establishes TLS 1.3 to the QR endpoint and pins the desktop SPKI from
the QR payload for this first connection. Discovery records are never used as a
certificate authority or trust anchor. The peers prove knowledge of the pairing
secret using a transcript-bound exporter or an equivalently reviewed TLS channel
binding, with distinct client/server role labels. The exact proof bytes must be
published as Rust/Kotlin vectors before implementation.

After successful proof, each peer sends its long-term public identity key inside
the pinned TLS channel and stores the peer key plus a user-visible trust record.
The media root secret is derived only from the authenticated pairing/control
transcript through a reviewed HKDF label; it is neither encoded in a control
frame nor persisted as a long-term identity credential.

Manual pairing, when added, uses a PAKE or a bidirectionally displayed
transcript-bound SAS. It must not reuse the QR secret format or treat a code
entered on Android alone as authentication.

Desktop long-term private keys and pairing records use DPAPI or Credential
Manager. Android private keys use Android Keystore. Revocation removes the peer
identity, session resumption material, pairing record, and any cached media
context. Pairing records expire, allow one success, and enforce attempt and
source-rate limits outside any realtime path.

## Security implications

The QR carries a high-entropy secret and public TLS pin, so screen visibility is
security-sensitive. Logs, diagnostics, clipboard history, notifications, and
crash reports must never retain QR payloads, pairing secrets, private keys, TLS
exporters, or media root secrets. A network relay cannot substitute a different
desktop certificate because Android pins the QR SPKI.

The pairing record is not a permanent permission grant. The trust record grants
only the initial audio-sender capability; future administrative capabilities
require distinct explicit consent and protocol fields.

## Validation evidence

Before implementation leaves the lab, provide Rust/Kotlin vectors for QR binary
encoding, TLS channel-binding inputs, role-separated proof bytes, transcript HKDF
derivation, and media root-secret handoff. Test expiry, single-use behavior,
wrong pin, wrong secret, replayed proof, concurrent attempts, revocation,
Android Keystore failure, and Windows credential-store failure. No test may log
secret material.

## Rollback plan

Pairing protocol versions and QR payload versions are explicit. A future
reviewed method adds a new version and migration path; it must not silently
downgrade to discovery trust, an unbound code, or file-based key storage.
