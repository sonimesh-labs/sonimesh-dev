#!/usr/bin/env python3
"""Validate redacted, operator-authored Android Phase 0A capture records."""

from __future__ import annotations

import argparse
import json
import sys
import unittest
from pathlib import Path
from typing import Any


SCENARIOS = frozenset(
    {
        "permission_denied",
        "consent_cancelled",
        "allowed",
        "idle_silent",
        "protected_unknown",
        "background",
        "lock_screen",
        "notification_stop",
        "projection_revoked",
        "process_recreation",
    }
)
SOURCE_CATEGORIES = frozenset({"allowed", "idle", "silent", "protected_unknown"})
PRESENCE_VALUES = frozenset({"present", "absent", "not_measured"})
TERMINAL_STATES = frozenset(
    {
        "active",
        "user_requested",
        "projection_revoked",
        "service_destroyed",
        "permission_denied",
        "consent_cancelled",
        "failed",
    }
)
NO_CAPTURE_SCENARIOS = frozenset({"permission_denied", "consent_cancelled"})
CAPTURE_SAMPLE_RATE_RANGE = range(8_000, 192_001)
MAX_CAPTURE_CHANNELS = 8
MAX_CAPTURE_BUFFER_BYTES = 64 * 1024
CAPTURE_READ_DURATION_MS = 20
CURRENT_SCHEMA_VERSION = 2
SUPPORTED_SCHEMA_VERSIONS = frozenset({1, CURRENT_SCHEMA_VERSION})
MAX_SIGNED_64 = (1 << 63) - 1


def require_object(value: Any, label: str, keys: set[str]) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != keys:
        raise ValueError(f"{label} does not match the closed record schema")
    return value


def require_nonempty_string(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{label} must be a nonempty string")
    return value


def require_nonnegative_int(value: Any, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise ValueError(f"{label} must be a nonnegative integer")
    return value


def require_nonnegative_long(value: Any, label: str) -> int:
    value = require_nonnegative_int(value, label)
    if value > MAX_SIGNED_64:
        raise ValueError(f"{label} must fit a signed 64-bit integer")
    return value


def validate_snapshot(
    value: Any, label: str, schema_version: int
) -> dict[str, int | None]:
    keys = {
        "capture_duration_ms",
        "last_read_completion_offset_ms",
        "captured_frames",
        "silent_reads",
        "read_errors",
    }
    if schema_version >= 2:
        keys.add("silent_frames")
    snapshot = require_object(value, label, keys)
    capture_duration_ms = require_nonnegative_long(
        snapshot["capture_duration_ms"], f"{label}.capture_duration_ms"
    )
    captured_frames = require_nonnegative_long(
        snapshot["captured_frames"], f"{label}.captured_frames"
    )
    silent_reads = require_nonnegative_long(snapshot["silent_reads"], f"{label}.silent_reads")
    read_errors = require_nonnegative_long(snapshot["read_errors"], f"{label}.read_errors")
    validated: dict[str, int | None] = {
        "capture_duration_ms": capture_duration_ms,
        "captured_frames": captured_frames,
        "silent_frames": None,
        "silent_reads": silent_reads,
        "read_errors": read_errors,
    }
    if schema_version >= 2:
        silent_frames = require_nonnegative_long(
            snapshot["silent_frames"], f"{label}.silent_frames"
        )
        if silent_frames > captured_frames:
            raise ValueError(f"{label}.silent_frames exceeds captured_frames")
        if silent_reads > silent_frames:
            raise ValueError(f"{label}.silent_reads exceeds silent_frames")
        validated["silent_frames"] = silent_frames

    offset = snapshot["last_read_completion_offset_ms"]
    if offset is not None:
        offset = require_nonnegative_long(offset, f"{label}.last_read_completion_offset_ms")
        if offset > capture_duration_ms:
            raise ValueError(f"{label} read completion offset exceeds capture duration")
    elif captured_frames != 0:
        raise ValueError(f"{label} has captured frames without a read completion offset")
    return {**validated, "last_read_completion_offset_ms": offset}


def validate_capture_format(value: Any) -> None:
    capture_format = require_object(
        value,
        "capture_format",
        {
            "sample_rate_hz",
            "channels",
            "audio_source",
            "channel_mask",
            "pcm_format",
            "buffer_size_bytes",
            "minimum_buffer_bytes",
            "read_samples",
        },
    )
    sample_rate = require_nonnegative_int(capture_format["sample_rate_hz"], "sample_rate_hz")
    channels = require_nonnegative_int(capture_format["channels"], "channels")
    if sample_rate not in CAPTURE_SAMPLE_RATE_RANGE or not 1 <= channels <= MAX_CAPTURE_CHANNELS:
        raise ValueError("capture format sample rate or channels are outside the Phase 0A bound")
    if capture_format["pcm_format"] != "PCM16":
        raise ValueError("capture format must retain PCM16 representation")
    require_nonnegative_int(capture_format["audio_source"], "audio_source")
    require_nonnegative_int(capture_format["channel_mask"], "channel_mask")
    buffer_size = require_nonnegative_int(capture_format["buffer_size_bytes"], "buffer_size_bytes")
    minimum_buffer = require_nonnegative_int(
        capture_format["minimum_buffer_bytes"], "minimum_buffer_bytes"
    )
    if not 1 <= minimum_buffer <= buffer_size <= MAX_CAPTURE_BUFFER_BYTES:
        raise ValueError("capture format buffers are outside the Phase 0A bound")
    expected_read_samples = (sample_rate * CAPTURE_READ_DURATION_MS + 999) // 1_000 * channels
    if capture_format["read_samples"] != expected_read_samples:
        raise ValueError("capture format read_samples does not match the fixed 20 ms read")


def validate_record(record: Any) -> None:
    root = require_object(
        record,
        "record",
        {
            "schema_version",
            "recorded_at",
            "scenario",
            "device",
            "app",
            "observed_source_category",
            "capture_format",
            "initial_snapshot",
            "final_snapshot",
            "terminal_state",
            "lifecycle",
            "device_conditions",
        },
    )
    schema_version = require_nonnegative_int(root["schema_version"], "schema_version")
    if schema_version not in SUPPORTED_SCHEMA_VERSIONS:
        raise ValueError("record schema_version must be 1 or 2")
    require_nonempty_string(root["recorded_at"], "recorded_at")
    scenario = root["scenario"]
    if scenario not in SCENARIOS:
        raise ValueError("record scenario is not in the Phase 0A matrix")
    validate_device(root["device"])
    validate_app(root["app"])
    validate_lifecycle(root["lifecycle"])
    validate_device_conditions(root["device_conditions"])
    terminal_state = root["terminal_state"]
    if terminal_state not in TERMINAL_STATES:
        raise ValueError("record terminal_state is not recognized")

    if scenario in NO_CAPTURE_SCENARIOS:
        validate_no_capture_record(root, scenario, terminal_state)
        return

    source_category = root["observed_source_category"]
    if source_category not in SOURCE_CATEGORIES:
        raise ValueError("capture record requires a bounded observed source category")
    if scenario == "allowed" and source_category != "allowed":
        raise ValueError("allowed scenario must retain the allowed source category")
    if scenario == "idle_silent" and source_category not in {"idle", "silent"}:
        raise ValueError("idle_silent scenario must retain idle or silent only")
    if scenario == "protected_unknown" and source_category != "protected_unknown":
        raise ValueError("protected_unknown scenario must retain protected_unknown only")
    if root["capture_format"] is None or root["initial_snapshot"] is None or root["final_snapshot"] is None:
        raise ValueError("capture scenario requires format plus initial and final snapshots")
    validate_capture_format(root["capture_format"])
    initial = validate_snapshot(root["initial_snapshot"], "initial_snapshot", schema_version)
    final = validate_snapshot(root["final_snapshot"], "final_snapshot", schema_version)
    validate_snapshot_progress(initial, final)
    validate_terminal_lifecycle(root["lifecycle"], scenario, terminal_state)


def validate_device(value: Any) -> None:
    device = require_object(
        value,
        "device",
        {"oem", "model", "android_release", "api_level", "build_fingerprint"},
    )
    for field in ("oem", "model", "android_release", "build_fingerprint"):
        require_nonempty_string(device[field], f"device.{field}")
    if require_nonnegative_int(device["api_level"], "device.api_level") == 0:
        raise ValueError("device.api_level must be positive")


def validate_app(value: Any) -> None:
    app = require_object(value, "app", {"version", "target_sdk"})
    require_nonempty_string(app["version"], "app.version")
    if require_nonnegative_int(app["target_sdk"], "app.target_sdk") == 0:
        raise ValueError("app.target_sdk must be positive")


def validate_lifecycle(value: Any) -> None:
    lifecycle = require_object(
        value,
        "lifecycle",
        {"notification", "service", "projection", "event_duration_ms", "suppressed_teardown_wakeup"},
    )
    for field in ("notification", "service", "projection"):
        if lifecycle[field] not in PRESENCE_VALUES:
            raise ValueError(f"lifecycle.{field} must be present, absent, or not_measured")
    require_nonnegative_int(lifecycle["event_duration_ms"], "lifecycle.event_duration_ms")
    if not isinstance(lifecycle["suppressed_teardown_wakeup"], bool):
        raise ValueError("lifecycle.suppressed_teardown_wakeup must be boolean")


def validate_device_conditions(value: Any) -> None:
    conditions = require_object(value, "device_conditions", {"battery", "thermal", "cpu", "memory"})
    for field, observation in conditions.items():
        require_nonempty_string(observation, f"device_conditions.{field}")


def validate_no_capture_record(root: dict[str, Any], scenario: str, terminal_state: str) -> None:
    expected_terminal = "permission_denied" if scenario == "permission_denied" else "consent_cancelled"
    if terminal_state != expected_terminal:
        raise ValueError(f"{scenario} record has an incompatible terminal state")
    if root["observed_source_category"] is not None:
        raise ValueError(f"{scenario} record must not retain a capture source category")
    if any(root[field] is not None for field in ("capture_format", "initial_snapshot", "final_snapshot")):
        raise ValueError(f"{scenario} record must not retain capture evidence")
    validate_absent_lifecycle(root["lifecycle"], scenario)


def validate_snapshot_progress(initial: dict[str, int | None], final: dict[str, int | None]) -> None:
    for field in ("capture_duration_ms", "captured_frames", "silent_reads", "read_errors"):
        if final[field] < initial[field]:
            raise ValueError(f"final_snapshot.{field} regressed")
    initial_silent_frames = initial["silent_frames"]
    final_silent_frames = final["silent_frames"]
    if initial_silent_frames is not None and (
        final_silent_frames is None or final_silent_frames < initial_silent_frames
    ):
        raise ValueError("final_snapshot.silent_frames regressed")
    initial_offset = initial["last_read_completion_offset_ms"]
    final_offset = final["last_read_completion_offset_ms"]
    if initial_offset is not None and (final_offset is None or final_offset < initial_offset):
        raise ValueError("final_snapshot read completion offset regressed")


def validate_terminal_lifecycle(lifecycle: dict[str, Any], scenario: str, terminal_state: str) -> None:
    expected_terminal = {
        "notification_stop": "user_requested",
        "projection_revoked": "projection_revoked",
        "process_recreation": "service_destroyed",
    }.get(scenario)
    if expected_terminal is not None and terminal_state != expected_terminal:
        raise ValueError(f"{scenario} record has an incompatible terminal state")
    if scenario in {"background", "lock_screen"} and lifecycle["event_duration_ms"] == 0:
        raise ValueError(f"{scenario} record requires a positive event duration")
    if terminal_state in {"user_requested", "projection_revoked", "service_destroyed", "failed"}:
        validate_absent_lifecycle(lifecycle, scenario)


def validate_absent_lifecycle(lifecycle: dict[str, Any], scenario: str) -> None:
    if any(lifecycle[field] != "absent" for field in ("notification", "service", "projection")):
        raise ValueError(f"{scenario} record requires absent notification, service, and projection")


def read_record(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("record", type=Path)
    arguments = parser.parse_args()
    try:
        validate_record(read_record(arguments.record))
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"Android capture record: invalid: {error}", file=sys.stderr)
        return 1
    print(f"Android capture record: ok path={arguments.record}")
    return 0


class AndroidCaptureRecordTests(unittest.TestCase):
    def test_accepts_redacted_active_capture_record(self) -> None:
        validate_record(active_record())

    def test_accepts_legacy_v1_records_without_silent_frames(self) -> None:
        validate_record(active_record(schema_version=1))

    def test_rejects_unknown_schema_and_inconsistent_silent_frame_counters(self) -> None:
        record = active_record()
        record["schema_version"] = 3
        with self.assertRaisesRegex(ValueError, "schema_version must be 1 or 2"):
            validate_record(record)

        record = active_record()
        record["initial_snapshot"].pop("silent_frames")
        with self.assertRaisesRegex(ValueError, "closed record schema"):
            validate_record(record)

        record = active_record()
        record["initial_snapshot"]["silent_frames"] = 1_500_000
        with self.assertRaisesRegex(ValueError, "silent_frames exceeds captured_frames"):
            validate_record(record)

        record = active_record()
        record["initial_snapshot"]["silent_frames"] = 1
        record["initial_snapshot"]["silent_reads"] = 2
        with self.assertRaisesRegex(ValueError, "silent_reads exceeds silent_frames"):
            validate_record(record)

        record = active_record()
        record["final_snapshot"]["silent_frames"] = 100
        with self.assertRaisesRegex(ValueError, "silent_frames regressed"):
            validate_record(record)

    def test_rejects_snapshot_values_larger_than_android_long(self) -> None:
        record = active_record()
        record["final_snapshot"]["captured_frames"] = MAX_SIGNED_64 + 1
        with self.assertRaisesRegex(ValueError, "must fit a signed 64-bit integer"):
            validate_record(record)

        record = active_record()
        record["final_snapshot"]["capture_duration_ms"] = MAX_SIGNED_64
        record["final_snapshot"]["last_read_completion_offset_ms"] = MAX_SIGNED_64
        validate_record(record)

    def test_rejects_counter_regression_and_unredacted_schema_extension(self) -> None:
        record = active_record()
        record["final_snapshot"]["captured_frames"] = 1_000_000
        with self.assertRaisesRegex(ValueError, "captured_frames regressed"):
            validate_record(record)

        record = active_record()
        record["source_application"] = "not allowed"
        with self.assertRaisesRegex(ValueError, "closed record schema"):
            validate_record(record)

    def test_requires_no_capture_evidence_after_denied_permission(self) -> None:
        record = no_capture_record("permission_denied")
        validate_record(record)
        record["capture_format"] = active_record()["capture_format"]
        with self.assertRaisesRegex(ValueError, "must not retain capture evidence"):
            validate_record(record)

    def test_validates_fixed_read_shape_and_terminal_teardown(self) -> None:
        record = active_record(scenario="projection_revoked", terminal_state="projection_revoked")
        record["lifecycle"] = absent_lifecycle()
        validate_record(record)

        record = active_record(terminal_state="user_requested")
        with self.assertRaisesRegex(ValueError, "requires absent notification"):
            validate_record(record)

        record = active_record()
        record["capture_format"]["read_samples"] = 1_919
        with self.assertRaisesRegex(ValueError, "fixed 20 ms read"):
            validate_record(record)


def active_record(
    scenario: str = "allowed",
    terminal_state: str = "active",
    schema_version: int = CURRENT_SCHEMA_VERSION,
) -> dict[str, Any]:
    record = {
        "schema_version": schema_version,
        "recorded_at": "2026-07-20T20:30:00+08:00",
        "scenario": scenario,
        "device": {
            "oem": "Example OEM",
            "model": "Example Model",
            "android_release": "15",
            "api_level": 35,
            "build_fingerprint": "example/model/build:15/ABC/123:userdebug/test-keys",
        },
        "app": {"version": "0.1.0-debug", "target_sdk": 36},
        "observed_source_category": "allowed",
        "capture_format": {
            "sample_rate_hz": 48_000,
            "channels": 2,
            "audio_source": 8,
            "channel_mask": 12,
            "pcm_format": "PCM16",
            "buffer_size_bytes": 38_400,
            "minimum_buffer_bytes": 7_680,
            "read_samples": 1_920,
        },
        "initial_snapshot": {
            "capture_duration_ms": 30_200,
            "last_read_completion_offset_ms": 30_180,
            "captured_frames": 1_449_984,
            "silent_frames": 960,
            "silent_reads": 2,
            "read_errors": 0,
        },
        "final_snapshot": {
            "capture_duration_ms": 69_700,
            "last_read_completion_offset_ms": 69_680,
            "captured_frames": 3_342_272,
            "silent_frames": 2_880,
            "silent_reads": 6,
            "read_errors": 0,
        },
        "terminal_state": terminal_state,
        "lifecycle": {
            "notification": "present",
            "service": "present",
            "projection": "present",
            "event_duration_ms": 15_000,
            "suppressed_teardown_wakeup": False,
        },
        "device_conditions": {
            "battery": "not_measured",
            "thermal": "not_measured",
            "cpu": "not_measured",
            "memory": "not_measured",
        },
    }
    if schema_version == 1:
        record["initial_snapshot"].pop("silent_frames")
        record["final_snapshot"].pop("silent_frames")
    return record


def absent_lifecycle() -> dict[str, Any]:
    return {
        "notification": "absent",
        "service": "absent",
        "projection": "absent",
        "event_duration_ms": 0,
        "suppressed_teardown_wakeup": True,
    }


def no_capture_record(scenario: str) -> dict[str, Any]:
    record = active_record(scenario=scenario, terminal_state=scenario)
    record["observed_source_category"] = None
    record["capture_format"] = None
    record["initial_snapshot"] = None
    record["final_snapshot"] = None
    record["lifecycle"] = absent_lifecycle()
    return record


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
