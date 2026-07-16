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
