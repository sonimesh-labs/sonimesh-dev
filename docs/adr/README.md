# Architecture Decision Records

These decisions are required before the corresponding implementation becomes a
release boundary. Each ADR should record the context, alternatives, decision,
security implications, test evidence, and rollback plan.

## Priority ADRs

1. **Android UI and service boundary** — Compose-only versus another UI shell;
   JNI/native ownership; buffer and lifecycle ownership.
2. **Windows output backend** — CPAL/WASAPI baseline, fixed-buffer behavior,
   device notifications, and fallback policy.
3. **First pairing and device identity** — QR, PAKE/SAS, key storage, revocation,
   and trust migration.
4. **UDP media security** — AEAD algorithm, HKDF labels, nonce layout, replay
   window, key epochs, and address migration.
5. **Protocol schema evolution** — Protobuf envelope, required/optional
   capabilities, message-size limits, and cross-language vectors.
6. **Frame duration and Opus policy** — 10 ms versus 20 ms, bitrate, DTX, FEC,
   content classes, and CPU/power evidence.
7. **Jitter and clock control** — playout scheduling, adaptive targets, ASRC
   choice, ratio bounds, and hard resynchronization.
8. **Realtime stream membership** — fixed slots or preallocated snapshot boundary
   with no callback allocation or long-lived lock.
9. **Android 17 local-network discovery** — system-mediated NsdManager picker,
   broad permission path, denial behavior, and manual address UX.
10. **Privacy and diagnostics** — retention, redaction, diagnostic export, and
    user visibility for capture limitations.
11. **Channel layout mapping** — bounded interleaved mapping, speaker-layout
    evidence, and the boundary between decoded PCM and device output.
12. **Nominal sample-rate conversion** — bounded conversion, drift separation,
    filter quality, and rate-mismatch evidence.

Do not mark an ADR complete merely because an API name has been selected. The
decision is complete only when its failure modes and validation evidence are
documented.

## Records

- [ADR 0001: Core workspace boundaries](./0001-core-workspace-boundaries.md) —
  accepted for feasibility work; platform and security choices remain gated.
- [ADR 0002: Realtime PCM queue boundary](./0002-realtime-pcm-queue.md) —
  accepted for the single-stream feasibility path with hardware timing still open.
- [ADR 0003: Windows output feasibility prototype](./0003-windows-output-prototype.md) —
  provisional CPAL shared-mode spike; direct WASAPI comparison remains required.
- [ADR 0004: Control-plane envelope and schema evolution](./0004-control-plane-schema.md) —
  accepted for bounded controlled-lab Protobuf work; security remains separate.
- [ADR 0005: UDP media protection and nonce lifecycle](./0005-udp-media-protection.md) —
  controlled-lab AEAD/key derivation boundary; pairing and TLS remain separate.
- [ADR 0006: Media key epoch control lifecycle](./0006-media-key-epoch-control.md) —
  control commands bind stream lifecycle to the AEAD key epoch.
- [ADR 0007: Bounded PCM occupancy drift control](./0007-drift-control-decision.md) —
  non-realtime per-stream PI control and hard-resynchronization boundary.
- [ADR 0008: First pairing and device identity boundary](./0008-first-pairing-and-identity.md) —
  QR-pinned TLS, transcript binding, OS-protected identity storage, and revocation.
- [ADR 0009: Opus decoder binding and frame policy](./0009-opus-codec-boundary.md) —
  optional native decoder feature gated by reproducible Windows/Android evidence.
- [ADR 0010: Android capture service and native boundary](./0010-android-capture-service-boundary.md) —
  Kotlin/Compose consent UI, foreground-service ownership, and capture-only evidence gate.
- [ADR 0011: Diagnostic export boundary](./0011-diagnostic-export-boundary.md) —
  explicit, bounded, redacted desktop export design deferred to Phase 2.
- [ADR 0012: Bounded channel layout mapping](./0012-channel-layout-mapping.md) —
  provisional index-based mapping between decoded PCM and output layouts.
- [ADR 0013: Bounded nominal sample-rate conversion](./0013-nominal-sample-rate-conversion.md) —
  provisional windowed-sinc conversion separated from drift control and hardware
  quality claims.
- [ADR 0014: Android 17 local-network discovery](./0014-android-17-local-network-discovery.md) —
  provisional picker-first discovery design pending Android 17 device evidence.
- [ADR 0015: Realtime stream membership boundary](./0015-realtime-stream-membership.md) —
  provisional fixed-slot membership boundary pending Phase 2 mixer evidence.
- [ADR 0016: Adaptive jitter target control](./0016-adaptive-jitter-target.md) —
  provisional slow bounded target control pending Phase 2 jitter evidence.
- [ADR 0017: Authenticated control-plane reconnection supplies a fresh media session](./0017-control-reconnect-fresh-media-session.md) —
  provisional `MediaSessionAllocator` no-reuse boundary pending the authenticated
  control-plane transport gate.
- [ADR 0018: Pairing QR payload encoding boundary](./0018-pairing-qr-payload-encoding.md) —
  provisional bounded version-one QR wire format pending the platform pairing owner.
- [ADR 0019: Pairing proof-exchange frame boundary](./0019-pairing-proof-exchange-frame.md) —
  provisional fixed-length transcript-proof wire frame inside the pinned TLS channel.
- [ADR 0020: Authenticated control-plane transport boundary](./0020-authenticated-control-transport.md) —
  provisional type-state wrapper that unlocks bounded control framing only after the pairing proof exchange.
- [ADR 0021: Production TLS provider selection for pairing](./0021-production-tls-provider.md) —
  provisional Rustls(`ring`) on the desktop and explicit Conscrypt on Android,
  pending cross-platform interoperability evidence.
