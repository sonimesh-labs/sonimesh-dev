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
certificate authority or trust anchor. A provider used by either peer must export
exactly 32 bytes after a successful TLS 1.3 handshake with:

~~~text
label = "EXPORTER-SoniMesh Pairing v1" (ASCII)
context = pairing_id (the exact 16 QR bytes)
use_context = true
length = 32
~~~

The provider output is an opaque `tls_exporter` value. The peers prove knowledge
of the pairing secret with the fixed HKDF-SHA256 construction published in
`docs/protocol/pairing-tls-binding-vectors.md`; client and server proofs have
distinct role bytes. Pairing helpers must accept the endpoint's local role and
internally derive the opposite expected proof role, rather than allowing a
pairing owner to select an arbitrary expected role when it verifies a peer
proof. The media root uses a separate label under the same exporter/secret
extract. No implementation may replace this mechanism with a
custom transcript hash, certificate fingerprint, discovery record, or one-sided
code. A provider that cannot pin the QR SPKI, complete TLS 1.3, or export these
exact bytes fails pairing closed. The exact proof bytes must be published as
Rust/Kotlin vectors before implementation.

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
source-rate limits outside any realtime path. The record must apply lifecycle,
monotonic-expiry, source-capacity, and per-source attempt checks before invoking
a caller-supplied proof verifier. An admitted attempt is charged before that
verifier runs, and a rejected admission must not invoke it. The verifier reports
verified, rejected, or failed; failure remains distinguishable from an invalid
proof, consumes the admitted attempt, and cannot establish trust. The verifier
may borrow opaque TLS/proof inputs for the duration of the call, but the record
never stores those inputs or their result independently of the lifecycle
transition. A consumed record remains redacted audit state while its verified
session later streams, reconnects, or recovers output; only expiration or
revocation retires that trusted session.

Before selecting a production Windows store, a Windows-only credential-store
spike may create a random, session-persisted Generic Credential containing only
fixed non-secret test bytes. Its target must come from the Windows
system-preferred CNG RNG with no predictable fallback. The spike must write,
read, validate, delete, and verify absence using that fresh target name, while
redacting both the target and credential blob. It is an API and failure-path
experiment only: it must not persist a pairing secret, private key, QR payload,
trust record, session ticket, or media context; it does not select Credential
Manager over DPAPI for the production identity format.

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
encoding, the fixed TLS exporter inputs, role-separated proof bytes, transcript
HKDF derivation, and media root-secret handoff. Test expiry, single-use behavior,
wrong pin, wrong secret, wrong exporter, wrong role, replayed proof, concurrent
attempts, revocation, Android Keystore failure, and Windows credential-store
failure. No test may log secret material.

The controlled-lab Rust core validates the bounded pairing-record lifecycle and
its control-domain session transitions: expiry, single use, monotonic-time
failure closure, user expiry, per-source attempt limits, and fixed source-table
capacity. Tests also prove that rejected admission never invokes the supplied
verifier and that an admitted failure consumes one attempt. Its public lifecycle
state cannot establish trust directly; only the pairing owner can advance a
consumed proof to `Ready`. The core schedules but does not implement the proof
algorithm, and it stores no proof bytes, secret, identity, or QR payload.

The same controlled-lab core models a local revocation as a terminal redacted
record state. Revocation must reject every later proof attempt and retire the
in-memory control session, negotiated capabilities, active stream identity, and
pending completion before returning to `Idle`. This verifies fail-closed
lifecycle ownership only; it neither stores nor deletes a production identity,
pairing record, session ticket, or media context. The future platform pairing
owner remains responsible for atomically deleting those durable values before
it reports a successful user-visible revocation.

The Windows credential-store spike must separately record successful cleanup and
the observed error category after deletion. A successful round trip does not
replace tests for access denial, profile/store unavailability, revocation, or
long-term identity migration.

Before selecting the Android identity format, an Android instrumentation spike
may generate a fresh AndroidKeyStore EC signing key under a randomized alias,
sign and verify fixed non-secret test bytes, confirm that private-key encoding is
unavailable, delete the alias, and confirm removal. The spike is isolated from
MediaProjection capture, pairing, QR data, TLS, and network paths. It must not
log the alias, public key, signature, or test bytes, and it must not create a
long-lived key or trust record. A device/emulator result is required because a
host Gradle build cannot execute AndroidKeyStore behavior.

Before a TLS library is selected for pairing, separate Windows and Android
provider spikes must demonstrate a fresh pinned TLS 1.3 connection and the exact
exporter call defined above. They must use fresh lab certificates and fixed
non-secret test inputs, compare only a success/failure category and the public
vector result, and clean up all listeners and temporary key material. A host-only
HKDF vector test does not prove provider exporter support, pin enforcement, or
TLS interoperability.

The Windows provider spike may use an isolated loopback Rust tool with Rustls
0.23.27 and its `ring` provider, both selected only for feasibility evidence.
The tool must create one in-memory self-signed lab certificate per run, force
TLS 1.3, and use a custom verifier whose only trust anchor is the expected DER
SPKI. It must reject a presented certificate whose SPKI differs from that pin
and verify the TLS 1.3 `CertificateVerify` signature using that same pinned
SPKI and the selected provider's supported algorithms. It must not substitute a
temporary root store, a hostname-only verifier, or an assertion-only custom
verifier. The positive path must show that client and server export equal,
exactly 32-byte values for the specified label and a fixed public 16-byte
context; an altered expected SPKI must make the handshake fail. The tool may
reuse only the public exporter constants from `sonimesh-protocol`; it must not
implement QR encoding, pairing records, proof exchange, trust storage,
revocation, discovery, or media networking.

The spike must use a bounded loopback listener, join its server thread, and
retain no certificate, SPKI, private-key DER, exporter, pairing input, or
identity material after process exit. It may print only stage success/failure
categories, negotiated TLS version, and requested exporter length. Temporary
private-key byte copies must use zeroizing storage where the dependency API
permits; an in-memory key object that cannot provide explicit zeroization must
remain local to the run and be dropped before the tool returns. A dated result
is valid only after a local run and must name the Windows build, Rust, Rustls,
rcgen, and provider versions. It does not select Rustls for production, prove
Android interoperability, establish a production certificate lifecycle, or
enable pairing.

The Android provider spike may use `conscrypt-android` 2.6.1, an Apache-2.0
Android AAR with `arm64-v8a` and `x86_64` native libraries, solely in an API
31-or-newer instrumentation test. It must generate a fresh AndroidKeyStore EC
key under a randomized alias and request its short-lived self-signed lab
certificate through `KeyGenParameterSpec` certificate fields, then delete the
alias in `finally`. An Android-side TLS 1.3 client must pin the certificate's
exact DER SPKI through a narrow test-only trust manager; a loopback server
using the same temporary AndroidKeyStore key entry must complete its TLS 1.3
handshake. Client and server must each call Conscrypt's public
`exportKeyingMaterial` with the ADR label, a supplied fixed public 16-byte
context, and length 32, then compare the results only in memory. A connection
using an altered expected SPKI must fail. The test must not install a provider
globally, rely on the device's ambient TLS provider, use a temporary root store,
use a hostname-only verifier, or accept a trust-manager assertion without an
SPKI comparison.

This Android spike is isolated from MediaProjection, capture, QR parsing,
pairing records, proof exchange, identity/trust storage, discovery, media
networking, and the audio path. It may use loopback TCP and a bounded server
thread only. Its `INTERNET` permission belongs solely to the isolated spike
target-app manifest, not the capture-only application manifest: Android
instrumentation executes the probe in that target app's process. It must neither
print nor write certificate bytes, SPKI, private keys, exporters, aliases, or
pairing inputs. A host Gradle build validates only test compilation; a dated API
31+ device/emulator record is required before claiming Android provider behavior.
The spike does not choose Conscrypt for production, establish interoperability
with Windows Rustls, or enable pairing.

## Rollback plan

Pairing protocol versions and QR payload versions are explicit. A future
reviewed method adds a new version and migration path; it must not silently
downgrade to discovery trust, an unbound code, or file-based key storage.
