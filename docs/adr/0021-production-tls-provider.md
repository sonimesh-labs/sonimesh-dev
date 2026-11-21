# ADR 0021: Production TLS provider selection for pairing

- Status: Provisional
- Date: 2026-08-24

## Context

[ADR 0008](./0008-first-pairing-and-identity.md) requires a pinned TLS 1.3
channel with an exact exporter call before any pairing proof can be derived,
but deliberately left the TLS library choice gated behind per-platform spikes:
the Windows spike "does not select Rustls for production", and the Android
spike "does not choose Conscrypt for production". The spikes have since run:

- Windows ([2026-07-20](../measurements/2026-07-20-windows-tls-provider-spike.md),
  [2026-08-22 rerun](../measurements/2026-08-22-windows-tls-provider-spike.md)):
  Rustls 0.23.27 with its `ring` provider completed a TLS 1.3-only loopback
  handshake whose only trust anchor was the expected DER SPKI, verified the
  `CertificateVerify` signature against that raw key, exported exactly 32 bytes
  for the pairing label, rejected an altered SPKI, drove the pure desktop
  handshake composition with the real exported bytes, and exchanged canonical
  [ADR 0019](./0019-pairing-proof-exchange-frame.md) frames over the live
  channel.
- Android ([2026-07-20](../measurements/2026-07-20-android-tls-provider-spike.md)):
  on a physical vivo V2309A (Android 15 / API 35, `arm64-v8a`), an explicitly
  loaded `conscrypt-android` 2.6.1 context completed the same positive path —
  AndroidKeyStore-held lab key, exact DER-SPKI pin, TLS 1.3, equal 32-byte
  exporters, altered-SPKI rejection — without installing the provider globally.

The [ADR 0020](./0020-authenticated-control-transport.md) type-state wrapper
now consumes such a channel, so the remaining software-side gate is naming the
production providers behind it. Without a selection, every later component
(control connect, session install, reconnection) inherits an undefined
dependency surface.

## Alternatives considered

- **Platform TLS stacks (SChannel on Windows, the ambient Conscrypt/ conscrypt
  system provider on Android).** SChannel cannot express the raw-key
  `CertificateVerify` verification against a QR-carried SPKI without a
  certificate trust object, and the ambient Android provider is versioned by
  each OEM's OS image, so exporter behavior and TLS 1.3 policy would vary by
  device. Rejected for the MVP control channel.
- **Rustls with `aws-lc-rs`.** Equivalent wire behavior but a larger native
  build surface on Windows for no measured benefit over `ring`, which the
  existing spike already exercised end to end. Rejected for now; switching
  providers later is a config-level change, not a protocol change.
- **OpenSSL/BoringSSL FFI on either side.** Largest audit and ABI surface,
  duplicates what Rustls already proves. Rejected.
- **No production library (hand-rolled TLS).** Violates ADR 0008's reviewed-
  mechanism requirement outright. Rejected.

## Decision

Pairing and authenticated control-plane connections use:

- **Desktop (Windows):** Rustls 0.23.x (exact version pinned in the workspace
  lockfile) with its `ring` crypto provider, configured TLS 1.3-only. The
  client installs a custom `ServerCertVerifier` whose sole trust anchor is the
  DER SPKI carried in the scanned QR payload: it must parse the end-entity
  certificate, require the exact SPKI match, verify the TLS 1.3
  `CertificateVerify` signature with `verify_tls13_signature_with_raw_key`,
  reject TLS 1.2 unconditionally, and expose a redacted `Debug`. After the
  handshake, the desktop exports exactly 32 bytes with the ADR 0008 label and
  the 16-byte pairing ID as context through `export_keying_material`.
- **Android:** `conscrypt-android` 2.6.1 loaded explicitly for the pairing
  connection (`SSLContext.getInstance(..., Conscrypt)`), never installed as the
  process-default security provider. The client uses a narrow trust manager
  that compares the presented leaf certificate's DER SPKI byte-for-byte against
  the QR pin and otherwise fails closed; the server-side lab/production key may
  live in AndroidKeyStore. Exporter extraction goes through Conscrypt's public
  `exportKeyingMaterial` with the same label, context, and length 32.
- Both sides fail closed on any of: wrong SPKI, negotiated version below
  TLS 1.3, exporter length mismatch, or unequal exporter derivation surfaces
  detected by the ADR 0019 proof exchange itself.

The selection covers only the pairing/authenticated-control channel. Media
packets remain AEAD-over-UDP under [ADR 0005](./0005-udp-media-protection.md)
and never touch TLS. Discovery stays unauthenticated per the architecture
documents.

## Security implications

The trust anchor is the QR-carried SPKI, so a relay cannot substitute a
different certificate; the altered-pin negative paths in both spikes are the
regression pins for this property. Restricting Android to an explicitly loaded
Conscrypt context prevents OEM ambient-provider drift from silently changing
TLS policy or exporter behavior. Raw-key `CertificateVerify` verification means
no root store exists to poison or mispin. Provider versions join the supply-
chain pinning list: any bump of `rustls`, `ring`, or `conscrypt-android` that
changes exporter or verifier behavior invalidates the recorded spike evidence
and must rerun it before release claims. Exporters, private keys, and pairing
inputs remain zeroized/redacted per ADR 0008 logging rules.

## Validation evidence

- A separate cross-platform lab probe may use a fresh ephemeral desktop
  certificate, a manually transferred public SPKI pin, and a fixed public test
  pairing secret. Both peers derive and verify role-separated ADR 0008 proofs
  from their local exporters; exporter and media-root bytes must not be sent or
  logged. The listener defaults to loopback and accepts one connection. This
  probe is not production pairing or identity storage.
- Windows: spike result dated 2026-07-20 plus the 2026-08-22 rerun that adds
  the in-memory handshake-composition stage and the live-channel proof-frame
  exchange; `tools/windows-tls-provider-prototype` reproduces it locally via
  `cargo test -p windows-tls-provider-prototype`.
- Android: device result dated 2026-07-20 on vivo V2309A / Android 15 with the
  declared Conscrypt and Keystore behavior.
- Android emulator to desktop Rustls: [2026-11-22 result](../measurements/2026-11-22-android-rustls-tls-interoperability.md)
  completed the pinned TLS 1.3 handshake and both role proofs over an ADB
  reverse-loopback route. This is cross-implementation evidence, but not a
  physical WLAN or OEM result.
- The desktop selection is implemented as
  `sonimesh-network::desktop_tls` (pinned client verifier, TLS 1.3-only
  server config, exporter extraction into zeroized storage). Loopback
  integration tests prove a real pinned handshake exports equal bytes on
  both sides, an altered SPKI fails closed, and the trusted channel drives
  the full pairing composition through hello negotiation in the
  `sonimesh-app` control-listen suite. These are same-host Rustls results;
  they do not measure cross-platform behavior.
- Cross-platform interoperability over a physical Android-to-Windows WLAN
  socket, with the real exporter feeding both transcript bindings, has **not**
  been measured yet. The emulator/ADB reverse-loopback result is insufficient
  to promote this ADR beyond **Provisional**.
- The ADR 0020 loopback integration test continues to prove the application
  path behind whichever provider supplies the channel.

## Rollback plan

Providers sit behind the ADR 0020 wrapper and the pure pairing composition;
neither imports TLS types. Reverting to a different stack replaces only the
connection-establishment code and this record, keeping the wire frames, HKDF
labels, vectors, and coordinator boundaries unchanged. Because the exporter
label/context/length are fixed by ADR 0008, a provider swap is invisible to
the peer as long as the exact bytes are reproduced and re-verified against the
published vectors.
