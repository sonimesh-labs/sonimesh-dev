# ADR 0001: Core workspace boundaries

- Status: Accepted for feasibility work
- Date: 2026-07-14

## Context

SoniMesh needs testable protocol and audio logic before Android, Windows, UI, and
security implementation choices settle. The realtime callback and hostile
network boundary also require constraints that UI or platform frameworks must
not weaken.

## Decision

Use a Rust 2024 workspace with six narrow crates matching the architecture:

- `sonimesh-protocol` owns bounded wire primitives, not sockets or crypto;
- `sonimesh-network` owns ordered authenticated ingress, not decoding;
- `sonimesh-audio` owns fixed-capacity audio containers and DSP primitives;
- `sonimesh-platform` owns adapter contracts, not a selected backend;
- `sonimesh-diagnostics` owns atomic counters and redaction;
- `sonimesh-app` owns control-domain lifecycle and later Tauri commands.

The portable foundation uses only the standard library. New dependencies require
a concrete implementation need and must not cross these dependency boundaries.
The Android and desktop UI projects remain gated until their platform ADRs have
enough prototype evidence to select build/runtime dependencies.

## Security implications

Unauthenticated packet parsing is allocation-free and bounded. The network API
orders authentication before replay-window mutation and does not expose payloads
to a decoder on failure. No placeholder cipher or pairing code is accepted as a
security implementation.

## Validation evidence

Unit tests cover byte order, header extensions and limits, replay decisions,
authentication ordering, bounded jitter/PCM behavior, redaction, and lifecycle
transitions. CI repeats format, Clippy, and tests on Windows and Linux.

Platform audio timing, Android lifecycle, cryptographic vectors, and physical
latency remain unvalidated and cannot be inferred from these tests.

## Rollback plan

Adapters may replace internal algorithms without moving platform/UI dependencies
into shared crates. If another language is required for a platform boundary, keep
the wire vectors and ownership rules and replace only the affected adapter.

