# ADR 0014: Android 17 local-network discovery

- Status: Provisional pending Android 17 device evidence
- Date: 2026-08-12

## Context

Discovery is unauthenticated: a discovered device is only a candidate, and the
local network is treated as hostile. The desktop may advertise
`_sonimesh._tcp.local`, but discovery records must never establish identity or
carry stable public-key material.

Android 17 makes local-network protection mandatory for applications targeting
SDK 37 or later. The architecture therefore requires the discovery design to
support both a system-mediated NsdManager picker path and an explicit
`ACCESS_LOCAL_NETWORK` path where broad local-network access is truly required.
An API 37 emulator is now available, but the production sender still targets
SDK 36 and has no discovery surface. A 2026-11-22 preflight confirms only the
platform and permission definitions; neither discovery path is validated here.
This record fixes the intended boundary and the evidence that must gate Phase 1
(Windows WLAN MVP) discovery.

## Decision

Discovery remains optional in every build: the QR/manual address path is always
available and does not depend on any discovery service.

- **Primary Android 17+ UX**: the system-mediated NsdManager picker. The user
  selects the desktop from the system UI, so the sender does not need broad
  local-network permission for the common single-desktop flow.
- **Explicit `ACCESS_LOCAL_NETWORK` path**: used only where broad local-network
  access is truly required (for example browsing without the system picker).
  The manifest entry must be justified in code review, and a permission denial
  fails closed: no fallback that weakens trust or silently retries a different
  discovery mode.
- **Manual fallback**: QR or typed IP/bootstrap remains supported and is not
  removed by either discovery path.
- **Desktop advertisement**: `_sonimesh._tcp.local` TXT records contain only
  non-sensitive, non-authoritative values: protocol version, control-service
  capabilities, a temporary instance identifier, a user-editable display name,
  and a control port. Media ports are negotiated only after authentication. A
  stable public-key fingerprint is never a discovery identifier.
- **Discovery is not trust**: a picked or browsed device still completes the
  QR-pinned TLS pairing of [ADR 0008](./0008-first-pairing-and-identity.md)
  before any session key or media stream is established.

## Security implications

A hostile device on the same LAN can advertise a look-alike service. The pairing
handshake, not the discovery record, binds identity, so a wrong selection is
rejected by the proof and QR binding rather than by the discovery label.
Permission denial or a cancelled picker must surface as a clear, non-retrying
state rather than an unauthenticated fallback. Discovery logs and TXT records
must not include pairing IDs, fingerprints, keys, or stable device identifiers.

## Validation evidence

Before Phase 1 can claim discovery:

- Android 17 (SDK 37 target) device tests for the system-mediated picker flow,
  including cancellation and a second launch after denial;
- an `ACCESS_LOCAL_NETWORK` grant/deny matrix on the same devices, with the
  sender reporting "no addressable peer" and listing possible causes instead
  of claiming a source reason;
- NsdManager discovery with the permission denied, including service-removal
  and stale-candidate handling;
- manual QR/IP fallback end-to-end on the same devices;
- report Android model/build, OEM, and network profile for every result.

The [2026-11-22 Android 17 preflight](../measurements/2026-11-22-android-17-local-network-preflight.md)
only establishes emulator availability and records that the capture-only
sender does not yet request discovery permissions. It does not satisfy any of
the discovery validation items above. The separate target-SDK 37 discovery
spike now exercises the request and picker callback shape on that emulator, but
it is still emulator-only and does not validate physical networking, broad
permission denial UX, service removal, or stale-candidate handling. Its
opt-in local-service probe records the permission state and skips unsupported
registration, discovery, or removal callbacks after cleanup; the current
emulator run therefore provides no service-removal evidence.

These are lifecycle and permission tests only. They do not qualify pairing,
TLS exporter support, capture behavior, or physical latency.

## Rollback plan

Discovery is a UI/control-plane convenience behind the manual bootstrap path.
Either the picker or the broad-permission path can be disabled or reordered
without changing the protocol, pairing, media, or realtime contracts.
