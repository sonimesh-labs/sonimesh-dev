# ADR 0011: Diagnostic export boundary

- Status: Accepted design; implementation deferred to Phase 2
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
format, selected buffer policy, session phase, and stream counter snapshots.

The bundle must exclude PCM, Opus, packet payloads, session/root keys, TLS
exporters, pairing secrets, certificate material, full IP addresses, stable
device identifiers, and untrusted display names by default. Export size and
retention require fixed limits; failed redaction aborts the write. No shared
diagnostics crate may depend on a UI, filesystem, Tauri, or platform credential
API.

## Validation evidence

Implementation must prove deterministic field allowlisting, endpoint redaction,
size limits, write-failure handling, and absence of forbidden byte sequences in
test bundles. Hardware reports remain separate from user diagnostic exports.

## Rollback plan

Bundle schemas are versioned. A new export field requires an explicit allowlist
change and tests; unknown future fields are not emitted by older builds.
