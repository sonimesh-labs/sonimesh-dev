#!/usr/bin/env python3
"""Keep the desktop preview state, metrics, and control replies aligned with the Rust control plane and wire schema."""

from __future__ import annotations

import re
import sys
import unittest
from pathlib import Path

TS_TYPE_LINE_PATTERN = re.compile(r"export type ([A-Za-z_][A-Za-z0-9_]*) = ?(.*)$")
TS_LITERAL_PATTERN = re.compile(r"'([A-Za-z0-9_]+)'")
IDENTIFIER_PATTERN = re.compile(r"^[A-Z][A-Za-z0-9_]*$")

ALIGNMENTS = (
    ("SessionState", "SessionPhase", "crates/sonimesh-app/src/session.rs"),
    ("PairingState", "PairingSnapshotState", "crates/sonimesh-app/src/control_snapshot.rs"),
    ("OutputHealth", "OutputHealth", "crates/sonimesh-platform/src/output.rs"),
)

# StreamMetrics fields the desktop preview synthesizes for derived UI values;
# they are not part of the Rust stream snapshot until the control plane
# publishes them.
PREVIEW_ONLY_STREAM_FIELDS = frozenset({"currentJitterMs", "playoutDelayMs"})

STRUCT_ALIGNMENTS = (
    ("StreamMetrics", "StreamMetricsSnapshot", "crates/sonimesh-diagnostics/src/metrics.rs"),
    ("OutputSnapshot", "OutputSnapshot", "crates/sonimesh-app/src/control_snapshot.rs"),
)

# Receiver-generated control-plane replies surface two bounded contracts: the
# reply kind mirrors the Rust ControlReply variants and the structured error
# codes mirror the wire ErrorCode enum minus its UNSPECIFIED sentinel. The wire
# StreamState enum is deliberately not aligned with the preview SessionState
# union: STOPPED and FAILED have no session-phase analog.
REPLY_KIND_ALIGNMENT = ("ControlReply", "kind", "ControlReply")
ERROR_CODE_ALIGNMENT = ("ControlErrorCode", "ErrorCode", "ERROR_CODE_")
REPLY_RUST_PATH = "crates/sonimesh-app/src/control_stream/reply.rs"
PROTO_PATH = "proto/control.proto"


def rust_enum_variants(source: str, enum_name: str) -> list[str]:
    """Returns variant names of one Rust enum, in declaration order.

    Handles unit variants and variants with struct-like or tuple bodies; brace
    depth is balanced so field lines are never mistaken for variant names.
    """
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
            continue
        header = re.match(r"([A-Z][A-Za-z0-9_]*)\s*[{,(]", candidate)
        if header is not None:
            variants.append(header.group(1))
    return variants


def ts_union_values(source: str, type_name: str) -> list[str]:
    """Returns literal values of one TypeScript union type, in declaration order.

    Continuation lines that start with `|` extend the union, matching the
    preview formatting conventions for multi-line unions.
    """
    declarations: list[tuple[str, str]] = []
    lines = source.splitlines()
    for index, line in enumerate(lines):
        match = TS_TYPE_LINE_PATTERN.match(line)
        if match is None:
            continue
        body = match.group(2)
        for continuation in lines[index + 1 :]:
            stripped = continuation.strip()
            if not stripped.startswith("|"):
                break
            body += " " + stripped
        declarations.append((match.group(1), body))
    if not declarations:
        raise ValueError("no TypeScript export type declarations found")
    match = next((item for item in declarations if item[0] == type_name), None)
    if match is None:
        raise ValueError(f"TypeScript union type {type_name} is not declared")
    return TS_LITERAL_PATTERN.findall(match[1])


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
        field_match = re.match(r"^\s*(?:pub\s+)?([a-z_][a-z0-9_]*)\s*:", line)
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


def ts_interface_field_literals(
    source: str, interface_name: str, field_name: str
) -> list[str]:
    """Returns string literals of one inline-union interface field.

    The field may be optional (`kind?`) and the union may continue on
    `|`-prefixed lines, matching the preview formatting conventions.
    """
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

    field_pattern = re.compile(rf"^\s*{re.escape(field_name)}\??\s*:")
    lines = source[body_start:body_end].splitlines()
    for index, line in enumerate(lines):
        if field_pattern.match(line) is None:
            continue
        value = line.split(":", 1)[1]
        for continuation in lines[index + 1 :]:
            stripped = continuation.strip()
            if not stripped or stripped.startswith("//"):
                break
            if stripped.startswith("|"):
                value += " " + stripped
                continue
            break
        literals = TS_LITERAL_PATTERN.findall(value)
        if literals:
            return literals
    raise ValueError(
        f"TypeScript interface {interface_name} has no literal field {field_name}"
    )


def proto_enum_values(source: str, enum_name: str, prefix: str) -> list[str]:
    """Returns values of one Protobuf enum with a common prefix removed.

    The `_UNSPECIFIED` zero sentinel is excluded because the control plane
    never publishes it in a preview-visible reply.
    """
    match = re.search(rf"enum {re.escape(enum_name)}\s*\{{", source)
    if match is None:
        raise ValueError(f"proto enum {enum_name} is not declared")

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

    values: list[str] = []
    for line in source[body_start:body_end].splitlines():
        stripped = line.strip().rstrip(";")
        if not stripped or stripped.startswith("//"):
            continue
        entry = re.match(r"([A-Z][A-Z0-9_]*)\s*=\s*\d+", stripped)
        if entry is None:
            continue
        name = entry.group(1)
        if name.startswith(prefix) and not name.endswith("_UNSPECIFIED"):
            values.append(name[len(prefix) :])
    return values


def proto_value_to_camel(value: str) -> str:
    """Converts a SCREAMING_SNAKE proto value to its preview camelCase form."""
    parts = value.split("_")
    return parts[0].lower() + "".join(part.capitalize() for part in parts[1:])


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


def validate_reply_sources(
    rust_source: str, ts_source: str, proto_source: str
) -> list[str]:
    """Returns deterministic alignment errors for the preview control-reply contract."""
    errors: list[str] = []

    ts_interface, ts_field, rust_enum = REPLY_KIND_ALIGNMENT
    try:
        variants = rust_enum_variants(rust_source, rust_enum)
        observed = set(ts_interface_field_literals(ts_source, ts_interface, ts_field))
    except ValueError as error:
        errors.append(f"preview alignment is invalid: {error}")
    else:
        expected = {camel(variant) for variant in variants}
        label = f"{ts_interface}.{ts_field}"
        missing = sorted(expected - observed)
        unexpected = sorted(observed - expected)
        if missing:
            errors.append(f"{label} is missing values: {', '.join(missing)}")
        if unexpected:
            errors.append(f"{label} has undeclared values: {', '.join(unexpected)}")

    ts_type, proto_enum, prefix = ERROR_CODE_ALIGNMENT
    try:
        codes = proto_enum_values(proto_source, proto_enum, prefix)
        observed = set(ts_union_values(ts_source, ts_type))
    except ValueError as error:
        errors.append(f"preview alignment is invalid: {error}")
    else:
        expected = {proto_value_to_camel(code) for code in codes}
        missing = sorted(expected - observed)
        unexpected = sorted(observed - expected)
        if missing:
            errors.append(f"{ts_type} is missing values: {', '.join(missing)}")
        if unexpected:
            errors.append(f"{ts_type} has undeclared values: {', '.join(unexpected)}")
    return errors


def validate_workspace(root: Path) -> list[str]:
    """Returns errors for the checked-in preview, Rust, and wire sources."""
    try:
        rust_paths = [path for _, _, path in ALIGNMENTS + STRUCT_ALIGNMENTS]
        rust_paths.append(REPLY_RUST_PATH)
        rust_source = "".join(
            (root / path).read_text(encoding="utf-8") for path in rust_paths
        )
        ts_source = (root / "apps" / "desktop-ui" / "src" / "types.ts").read_text(
            encoding="utf-8"
        )
        proto_source = (root / PROTO_PATH).read_text(encoding="utf-8")
    except OSError as error:
        return [f"unable to read preview alignment sources: {error}"]
    return validate_sources(rust_source, ts_source) + validate_reply_sources(
        rust_source, ts_source, proto_source
    )


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

pub enum OutputHealth {
    /// No callback or backend fault has been observed.
    Healthy,
    /// Callback exceeded the preallocated safe output bound.
    CallbackCapacityExceeded,
    /// Backend reported an asynchronous stream error.
    StreamError,
}
"""
    TS = """
export type SessionState = 'idle' | 'pairing' | 'ready' | 'streaming' | 'reconnecting' | 'outputUnavailable'

export type PairingState = 'inactive' | 'active' | 'consumed' | 'expired' | 'revoked'

export type OutputHealth = 'healthy' | 'callbackCapacityExceeded' | 'streamError'
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

pub struct OutputSnapshot {
    /// Whether a recovery selection is retained.
    selection_retained: bool,
    /// Whether the controller currently owns an active stream.
    stream_active: bool,
    /// Whether a non-healthy output fault has already been reported.
    fault_reported: bool,
    /// Latest low-rate output health state.
    health: OutputHealth,
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

export interface OutputSnapshot {
  selectionRetained: boolean
  streamActive: boolean
  faultReported: boolean
  health: OutputHealth
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
                "preview alignment is invalid: Rust enum OutputHealth is not declared",
                "preview alignment is invalid: Rust struct StreamMetricsSnapshot is not declared",
                "preview alignment is invalid: Rust struct OutputSnapshot is not declared",
            ],
        )

    def test_multi_line_ts_union_is_supported(self) -> None:
        ts = (
            "export type SessionState = 'idle' | 'pairing' | 'ready'\n"
            "  | 'streaming' | 'reconnecting'\n"
            "  | 'outputUnavailable'\n"
            "export type PairingState = 'inactive' | 'active' | 'consumed' | 'expired' | 'revoked'\n"
            "export type OutputHealth = 'healthy' | 'callbackCapacityExceeded' | 'streamError'\n"
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


    def test_reports_output_health_value_drift(self) -> None:
        rust = self.RUST + self.RUST_SNAPSHOT
        ts = (self.TS + self.TS_INTERFACE).replace(
            "'streamError'", "'streamError' | 'deviceGone'"
        )
        self.assertEqual(
            validate_sources(rust, ts),
            ["OutputHealth has undeclared values: deviceGone"],
        )

        ts = (self.TS + self.TS_INTERFACE).replace(
            "export type OutputHealth =", "export type OutputHealthX ="
        )
        self.assertEqual(
            validate_sources(rust, ts),
            ["preview alignment is invalid: TypeScript union type OutputHealth is not declared"],
        )

    def test_reports_output_snapshot_field_drift(self) -> None:
        rust = self.RUST + self.RUST_SNAPSHOT
        ts = (self.TS + self.TS_INTERFACE).replace("  health: OutputHealth\n", "")
        self.assertEqual(
            validate_sources(rust, ts),
            ["OutputSnapshot is missing fields: health"],
        )

        ts = (self.TS + self.TS_INTERFACE).replace(
            "  selectionRetained: boolean\n", "  inventedField: boolean\n"
        )
        self.assertEqual(
            validate_sources(rust, ts),
            [
                "OutputSnapshot is missing fields: selectionRetained",
                "OutputSnapshot has undeclared fields: inventedField",
            ],
        )


    RUST_REPLY = """
pub enum ControlReply {
    StreamStatus { identity: u64, state: u32, detail: String },
    Error { code: u32, retryable: bool, detail: String },
}
"""

    TS_REPLY = """
export type ControlErrorCode =
  | 'invalidRequest'
  | 'retryLater'
  | 'unsupportedCapability'
  | 'incompatibleVersion'
  | 'streamNotFound'

export interface ControlReply {
  kind: 'streamStatus' | 'error'
  requestId: number
  detail: string
  state?: SessionState
  code?: ControlErrorCode
  retryable?: boolean
}
"""

    PROTO = """
enum ErrorCode {
  ERROR_CODE_UNSPECIFIED = 0;
  ERROR_CODE_INCOMPATIBLE_VERSION = 1;
  ERROR_CODE_UNSUPPORTED_CAPABILITY = 2;
  ERROR_CODE_INVALID_REQUEST = 3;
  ERROR_CODE_STREAM_NOT_FOUND = 4;
  ERROR_CODE_RETRY_LATER = 5;
}
"""

    def test_matching_reply_contract_is_valid(self) -> None:
        self.assertEqual(
            validate_reply_sources(self.RUST_REPLY, self.TS_REPLY, self.PROTO), []
        )

    def test_reports_reply_kind_drift(self) -> None:
        rust = self.RUST_REPLY.replace(
            "    Error { code: u32, retryable: bool, detail: String },\n", ""
        )
        self.assertEqual(
            validate_reply_sources(rust, self.TS_REPLY, self.PROTO),
            ["ControlReply.kind has undeclared values: error"],
        )

        ts = self.TS_REPLY.replace(
            "kind: 'streamStatus' | 'error'", "kind: 'streamStatus'"
        )
        self.assertEqual(
            validate_reply_sources(self.RUST_REPLY, ts, self.PROTO),
            ["ControlReply.kind is missing values: error"],
        )

    def test_reports_error_code_drift(self) -> None:
        ts = self.TS_REPLY.replace("'streamNotFound'", "'streamGone'")
        self.assertEqual(
            validate_reply_sources(self.RUST_REPLY, ts, self.PROTO),
            [
                "ControlErrorCode is missing values: streamNotFound",
                "ControlErrorCode has undeclared values: streamGone",
            ],
        )

        proto = self.PROTO.replace("  ERROR_CODE_RETRY_LATER = 5;\n", "")
        self.assertEqual(
            validate_reply_sources(self.RUST_REPLY, self.TS_REPLY, proto),
            ["ControlErrorCode has undeclared values: retryLater"],
        )

    def test_reports_missing_reply_declarations(self) -> None:
        self.assertEqual(
            validate_reply_sources("pub fn unrelated() {}", self.TS_REPLY, self.PROTO),
            ["preview alignment is invalid: Rust enum ControlReply is not declared"],
        )

        ts = self.TS_REPLY.replace(
            "kind: 'streamStatus' | 'error'", "kind: ControlReplyKind"
        )
        self.assertEqual(
            validate_reply_sources(self.RUST_REPLY, ts, self.PROTO),
            [
                "preview alignment is invalid: "
                "TypeScript interface ControlReply has no literal field kind"
            ],
        )

        self.assertEqual(
            validate_reply_sources(
                self.RUST_REPLY, self.TS_REPLY, 'syntax = "proto3";'
            ),
            ["preview alignment is invalid: proto enum ErrorCode is not declared"],
        )

        ts = self.TS_REPLY.replace(
            "export type ControlErrorCode =", "export type ControlErrorCodeX ="
        )
        self.assertEqual(
            validate_reply_sources(self.RUST_REPLY, ts, self.PROTO),
            [
                "preview alignment is invalid: "
                "TypeScript union type ControlErrorCode is not declared"
            ],
        )


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
