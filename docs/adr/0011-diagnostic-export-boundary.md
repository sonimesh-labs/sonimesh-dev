# ADR 0011: Diagnostic export boundary

- Status: Accepted design; bundle writer implemented (Phase 2)
- Date: 2026-07-17

## Context

Low-rate metrics are useful for support and measurement, but diagnostic export
crosses a privacy boundary. The realtime path may only update atomic counters;
it must never serialize, allocate for, or write an export. A future UI action
must not accidentally include audio, keys, raw addresses, pairing material, or
stable identifiers.

## Decision

Exports are explicit user actions owned by the desktop control domain. Before
writing a bounded, versioned bundle, that owner snapshots only approved
low-rate values: software version, redacted endpoint family/port, platform
format, selected buffer policy, session phase, and stream counter snapshots,
including distinct underflow and callback refill-budget exhaustion counts.

Each cumulative `u64` counter uses relaxed lock-free updates and saturates at
`u64::MAX` instead of wrapping to a smaller value. When stream snapshots cross
a JSON or TypeScript control/UI boundary, those counters are represented as
unsigned decimal strings. This avoids JavaScript number precision loss during
long-running sessions; bounded
format, queue, and timing fields remain numeric where their documented ranges
fit exactly.

The bundle must exclude PCM, Opus, packet payloads, session/root keys, TLS
exporters, pairing secrets, certificate material, full IP addresses, stable
device identifiers, and untrusted display names by default. Export size and
retention require fixed limits; failed redaction aborts the write. No shared
diagnostics crate may depend on a UI, filesystem, Tauri, or platform credential
API.

The bundle writer lives in `sonimesh-diagnostics` (crate-private writer,
public snapshot schema) and serializes exactly the allowlisted fields above
to a bounded, deterministic, versioned JSON document. The desktop control
domain owns the export action: it fills the snapshot from its low-rate state
and is responsible for retention policy. The app-side export command and
retention limits remain open.

The app-side collector now exists in `sonimesh-app`:
`ControlToMixerBridge::export_snapshot` fills an allowlisted
`ExportSessionSnapshot` from live control and mixer state — the coordinator's
lifecycle phase, the platform output format once the output is active, and one
redacted `StreamMetricsSnapshot` per active mixer stream in slot order — and
`MixerStreamSession::stream_metrics` reads those counters from the session's
retained per-stream endpoints. The bundle writer remains the single serializer
in `sonimesh-diagnostics`; the desktop control domain still owns the export
action, and the UI/Tauri command that triggers an export and the file
retention policy remain open.

## Validation evidence

Implementation must prove deterministic field allowlisting, endpoint redaction,
size limits, write-failure handling, and absence of forbidden byte sequences in
test bundles. Hardware reports remain separate from user diagnostic exports.

Implemented evidence (Phase 2): the `export` module's unit tests assert the
exact emitted bundle for a fixed snapshot (deterministic allowlisting), redacted
endpoint output for IPv4 and IPv6, the fixed byte and stream bounds, fail-closed
rejection of malformed or oversized input, and the absence of forbidden byte
sequences in test bundles.

The app-side collector tests assert that the bridge fills the export snapshot
from live state: session phase maps onto the closed export phase set, platform
format appears only while the output is active, per-stream metrics follow slot
order with zeroed entries for streams without a counters endpoint, and a built
bundle redacts the endpoint address while retaining the allowlisted fields.

## Rollback plan

Bundle schemas are versioned. A new export field requires an explicit allowlist
change and tests; unknown future fields are not emitted by older builds.
