#!/usr/bin/env python3
"""Keep the desktop preview state and metrics aligned with the Rust control plane."""

from __future__ import annotations

import re
import sys
import unittest
from pathlib import Path

TS_UNION_PATTERN = re.compile(
    r"export type ([A-Za-z_][A-Za-z0-9_]*) = ([^;]*?)(?:;|(?=\n\s*export (?:type|interface|const|function)|\Z))"
)
TS_LITERAL_PATTERN = re.compile(r"'([A-Za-z0-9_]+)'")
IDENTIFIER_PATTERN = re.compile(r"^[A-Z][A-Za-z0-9_]*$")

ALIGNMENTS = (
    ("SessionState", "SessionPhase", "crates/sonimesh-app/src/session.rs"),
    ("PairingState", "PairingSnapshotState", "crates/sonimesh-app/src/control_snapshot.rs"),
)

# StreamMetrics fields the desktop preview synthesizes for derived UI values;
# they are not part of the Rust stream snapshot until the control plane
# publishes them.
PREVIEW_ONLY_STREAM_FIELDS = frozenset({"currentJitterMs", "playoutDelayMs"})

STRUCT_ALIGNMENTS = (
    ("StreamMetrics", "StreamMetricsSnapshot", "crates/sonimesh-diagnostics/src/metrics.rs"),
)


def rust_enum_variants(source: str, enum_name: str) -> list[str]:
    """Returns unit-variant names of one Rust enum, in declaration order."""
    match = re.search(rf"pub enum {re.escape(enum_name)}\s*\{{", source)
    if match is None:
        raise ValueError(f"Rust enum {enum_name} is not declared")

    depth = 1
    body_start = match.end()
    body_end = len(source)
    for index in range(body_start, len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                body_end = index
                break

    variants: list[str] = []
    for line in source[body_start:body_end].splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("//") or stripped.startswith("#["):
            continue
        candidate = stripped.rstrip(",").strip()
        if IDENTIFIER_PATTERN.fullmatch(candidate) is not None:
            variants.append(candidate)
    return variants


def ts_union_values(source: str, type_name: str) -> list[str]:
    """Returns literal values of one TypeScript union type, in declaration order."""
    matches = list(TS_UNION_PATTERN.finditer(source))
    if not matches:
        raise ValueError("no TypeScript export type declarations found")
    match = next((item for item in matches if item.group(1) == type_name), None)
    if match is None:
        raise ValueError(f"TypeScript union type {type_name} is not declared")
    return TS_LITERAL_PATTERN.findall(match.group(2))


def camel(variant: str) -> str:
    """Converts a Rust PascalCase variant to its preview camelCase literal."""
    return variant[0].lower() + variant[1:]


def rust_struct_fields(source: str, struct_name: str) -> list[str]:
    """Returns field names of one Rust struct, in declaration order."""
    match = re.search(rf"pub struct {re.escape(struct_name)}\s*\{{", source)
    if match is None:
        raise ValueError(f"Rust struct {struct_name} is not declared")

    depth = 1
    body_start = match.end()
    body_end = len(source)
    for index in range(body_start, len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                body_end = index
                break

    fields: list[str] = []
    for line in source[body_start:body_end].splitlines():
        field_match = re.match(r"^\s*pub\s+([a-z_][a-z0-9_]*)\s*:", line)
        if field_match is not None:
            fields.append(field_match.group(1))
    return fields


def ts_interface_fields(source: str, interface_name: str) -> list[str]:
    """Returns field names of one TypeScript interface, in declaration order."""
    match = re.search(rf"export interface {re.escape(interface_name)}\s*\{{", source)
    if match is None:
        raise ValueError(f"TypeScript interface {interface_name} is not declared")

    depth = 1
    body_start = match.end()
    body_end = len(source)
    for index in range(body_start, len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                body_end = index
                break

    fields: list[str] = []
    for line in source[body_start:body_end].splitlines():
        field_match = re.match(r"^\s*([a-z][A-Za-z0-9_]*)\s*:", line)
        if field_match is not None:
            fields.append(field_match.group(1))
    return fields


def snake_to_camel(field: str) -> str:
    """Converts a Rust snake_case field name to its preview camelCase form."""
    parts = field.split("_")
    return parts[0] + "".join(part.capitalize() for part in parts[1:])


def validate_sources(rust_source: str, ts_source: str) -> list[str]:
    """Returns deterministic alignment errors across every declared mapping."""
    errors: list[str] = []
    for ts_type, rust_enum, _ in ALIGNMENTS:
        try:
            variants = rust_enum_variants(rust_source, rust_enum)
            values = ts_union_values(ts_source, ts_type)
        except ValueError as error:
            errors.append(f"preview alignment is invalid: {error}")
            continue

        expected = [camel(variant) for variant in variants]
        expected_set = set(expected)
        observed_set = set(values)

        missing = sorted(expected_set - observed_set)
        unexpected = sorted(observed_set - expected_set)
        if missing:
            errors.append(f"{ts_type} is missing values: {', '.join(missing)}")
        if unexpected:
            errors.append(f"{ts_type} has undeclared values: {', '.join(unexpected)}")

    for ts_interface, rust_struct, _ in STRUCT_ALIGNMENTS:
        try:
            fields = rust_struct_fields(rust_source, rust_struct)
            observed = set(ts_interface_fields(ts_source, ts_interface))
        except ValueError as error:
            errors.append(f"preview alignment is invalid: {error}")
            continue

        observed -= PREVIEW_ONLY_STREAM_FIELDS
        expected = {snake_to_camel(field) for field in fields}
        missing = sorted(expected - observed)
        unexpected = sorted(observed - expected)
        if missing:
            errors.append(f"{ts_interface} is missing fields: {', '.join(missing)}")
        if unexpected:
            errors.append(f"{ts_interface} has undeclared fields: {', '.join(unexpected)}")
    return errors


def validate_workspace(root: Path) -> list[str]:
    """Returns errors for the checked-in preview and Rust control-plane sources."""
    try:
        rust_paths = [path for _, _, path in ALIGNMENTS + STRUCT_ALIGNMENTS]
        rust_source = "".join(
            (root / path).read_text(encoding="utf-8") for path in rust_paths
        )
        ts_source = (root / "apps" / "desktop-ui" / "src" / "types.ts").read_text(encoding="utf-8")
    except OSError as error:
        return [f"unable to read preview alignment sources: {error}"]
    return validate_sources(rust_source, ts_source)


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    errors = validate_workspace(root)
    for error in errors:
        print(error, file=sys.stderr)
    if errors:
        print(f"preview alignment: {len(errors)} error(s)", file=sys.stderr)
        return 1
    print("preview alignment: ok")
    return 0


class PreviewAlignmentTests(unittest.TestCase):
    RUST = """
pub enum SessionPhase {
    #[default]
    Idle,
    /// A short-lived first-pairing record is active.
    Pairing,
    Ready,
    Streaming,
    Reconnecting,
    OutputUnavailable,
}

pub enum PairingSnapshotState {
    #[default]
    Inactive,
    Active,
    Consumed,
    Expired,
    Revoked,
}
"""
    TS = """
export type SessionState = 'idle' | 'pairing' | 'ready' | 'streaming' | 'reconnecting' | 'outputUnavailable'

export type PairingState = 'inactive' | 'active' | 'consumed' | 'expired' | 'revoked'
"""
    RUST_SNAPSHOT = """
pub struct StreamMetricsSnapshot {
    /// Successfully authenticated packets.
    pub authenticated_packets: u64,
    /// Current PCM blocks belonging to the active stream epoch.
    pub queued_pcm_blocks: u16,
    /// Adaptive-target minimum water level of the active epoch, in frames.
    pub target_min_frames: u16,
    /// Smoothed packet-arrival-jitter estimate of the active epoch, in ms.
    pub target_smoothed_jitter_ms: u32,
    /// Stream-local resets.
    pub stream_resets: u64,
}
"""
    TS_INTERFACE = """
export interface StreamMetrics {
  authenticatedPackets: MetricCounter
  queuedPcmBlocks: number
  targetMinFrames: number
  targetSmoothedJitterMs: number
  streamResets: MetricCounter
  currentJitterMs: number
  playoutDelayMs: number
}
"""

    def test_matching_enums_and_unions_are_valid(self) -> None:
        rust = self.RUST + self.RUST_SNAPSHOT
        ts = self.TS + self.TS_INTERFACE
        self.assertEqual(validate_sources(rust, ts), [])

    def test_reports_missing_and_undeclared_ts_values(self) -> None:
        ts = (self.TS + self.TS_INTERFACE).replace(
            "'outputUnavailable'", "'outputUnavailable' | 'drifted'"
        ).replace("'revoked'", "'expired'")
        self.assertEqual(
            validate_sources(self.RUST + self.RUST_SNAPSHOT, ts),
            [
                "SessionState has undeclared values: drifted",
                "PairingState is missing values: revoked",
            ],
        )

    def test_reports_rust_variant_removal_and_missing_ts_union(self) -> None:
        rust = (self.RUST + self.RUST_SNAPSHOT).replace("    OutputUnavailable,", "")
        self.assertEqual(
            validate_sources(rust, self.TS + self.TS_INTERFACE),
            ["SessionState has undeclared values: outputUnavailable"],
        )

        ts = (self.TS + self.TS_INTERFACE).replace(
            "export type PairingState =", "export type PairingStatus ="
        )
        self.assertEqual(
            validate_sources(self.RUST + self.RUST_SNAPSHOT, ts),
            ["preview alignment is invalid: TypeScript union type PairingState is not declared"],
        )

    def test_rejects_missing_rust_declarations(self) -> None:
        self.assertEqual(
            validate_sources("pub fn unrelated() {}", self.TS + self.TS_INTERFACE),
            [
                "preview alignment is invalid: Rust enum SessionPhase is not declared",
                "preview alignment is invalid: Rust enum PairingSnapshotState is not declared",
                "preview alignment is invalid: Rust struct StreamMetricsSnapshot is not declared",
            ],
        )

    def test_multi_line_ts_union_is_supported(self) -> None:
        ts = (
            "export type SessionState = 'idle' | 'pairing' | 'ready'\n"
            "  | 'streaming' | 'reconnecting'\n"
            "  | 'outputUnavailable'\n"
            "export type PairingState = 'inactive' | 'active' | 'consumed' | 'expired' | 'revoked'\n"
        )
        self.assertEqual(
            validate_sources(self.RUST + self.RUST_SNAPSHOT, ts + self.TS_INTERFACE), []
        )

    def test_matching_struct_fields_are_valid(self) -> None:
        rust = self.RUST + self.RUST_SNAPSHOT
        ts = self.TS + self.TS_INTERFACE
        self.assertEqual(validate_sources(rust, ts), [])

    def test_reports_missing_and_undeclared_ts_fields(self) -> None:
        rust = self.RUST + self.RUST_SNAPSHOT
        ts = self.TS + self.TS_INTERFACE
        ts = ts.replace("  streamResets: MetricCounter\n", "")
        ts = ts.replace("  currentJitterMs: number\n", "  inventedMetric: number\n")
        self.assertEqual(
            validate_sources(rust, ts),
            [
                "StreamMetrics is missing fields: streamResets",
                "StreamMetrics has undeclared fields: inventedMetric",
            ],
        )

    def test_reports_rust_field_removal_and_missing_ts_interface(self) -> None:
        rust = self.RUST + self.RUST_SNAPSHOT.replace(
            "    pub target_smoothed_jitter_ms: u32,\n", ""
        )
        self.assertEqual(
            validate_sources(rust, self.TS + self.TS_INTERFACE),
            ["StreamMetrics has undeclared fields: targetSmoothedJitterMs"],
        )

        ts = self.TS + self.TS_INTERFACE.replace(
            "export interface StreamMetrics {", "export interface StreamMetricsX {"
        )
        self.assertEqual(
            validate_sources(self.RUST + self.RUST_SNAPSHOT, ts),
            [
                "preview alignment is invalid: "
                "TypeScript interface StreamMetrics is not declared"
            ],
        )


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
