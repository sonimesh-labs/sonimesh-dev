#!/usr/bin/env python3
"""Validate redacted Android discovery measurement records."""

from __future__ import annotations

import argparse
import json
import re
import sys
import unittest
from datetime import datetime
from pathlib import Path
from typing import Any


CURRENT_SCHEMA_VERSION = 1
MAX_ERROR_CODE = 255
ALLOWED_PERMISSION_STATES = frozenset({"denied", "granted"})
FORBIDDEN_IDENTIFIERS = (
    re.compile(r"emulator-\d+", re.IGNORECASE),
    re.compile(r"SoniMeshProbe-\d+", re.IGNORECASE),
)


def require_object(value: Any, label: str, keys: set[str]) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != keys:
        raise ValueError(f"{label} does not match the closed discovery record schema")
    return value


def require_string(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{label} must be a nonempty string")
    return value


def require_enum(value: Any, allowed: frozenset[str], label: str) -> str:
    value = require_string(value, label)
    if value not in allowed:
        raise ValueError(f"{label} is not recognized")
    return value


def require_int(value: Any, label: str, *, minimum: int = 0, maximum: int | None = None) -> int:
    if not isinstance(value, int) or isinstance(value, bool):
        raise ValueError(f"{label} must be an integer")
    if value < minimum or (maximum is not None and value > maximum):
        raise ValueError(f"{label} is outside its bound")
    return value


def validate_timestamp(value: Any) -> None:
    timestamp = require_string(value, "recorded_at")
    normalized = f"{timestamp[:-1]}+00:00" if timestamp.endswith("Z") else timestamp
    try:
        parsed = datetime.fromisoformat(normalized)
    except ValueError as error:
        raise ValueError("recorded_at must be an ISO 8601 timestamp") from error
    if parsed.tzinfo is None or parsed.utcoffset() is None:
        raise ValueError("recorded_at must include an explicit UTC offset")


def validate_record(record: Any) -> None:
    root = require_object(
        record,
        "record",
        {
            "schema_version",
            "recorded_at",
            "scenario",
            "device",
            "spike",
            "permission",
            "observations",
            "instrumentation",
            "limitations",
        },
    )
    if require_int(root["schema_version"], "schema_version") != CURRENT_SCHEMA_VERSION:
        raise ValueError("unsupported discovery record schema_version")
    validate_timestamp(root["recorded_at"])
    if root["scenario"] != "android_17_discovery_lifecycle":
        raise ValueError("record scenario is not the Android 17 discovery lifecycle")

    device = require_object(
        root["device"],
        "device",
        {"oem", "model", "android_release", "api_level"},
    )
    for field in ("oem", "model", "android_release"):
        require_string(device[field], f"device.{field}")
    if require_int(device["api_level"], "device.api_level") != 37:
        raise ValueError("device.api_level must be Android 17 API 37")

    spike = require_object(
        root["spike"],
        "spike",
        {"package", "target_sdk", "service_type", "callback_timeout_seconds"},
    )
    if spike["package"] != "com.sonimesh.discoveryspike":
        raise ValueError("spike.package must identify the isolated discovery target")
    if require_int(spike["target_sdk"], "spike.target_sdk") != 37:
        raise ValueError("spike.target_sdk must be 37")
    if spike["service_type"] != "_sonimesh._tcp":
        raise ValueError("spike.service_type is not the SoniMesh service type")
    require_int(spike["callback_timeout_seconds"], "spike.callback_timeout_seconds", minimum=1, maximum=60)

    permission = require_object(
        root["permission"],
        "permission",
        {"name", "picker_run_state", "local_service_run_state"},
    )
    if permission["name"] != "ACCESS_LOCAL_NETWORK":
        raise ValueError("permission.name must identify ACCESS_LOCAL_NETWORK")
    require_enum(permission["picker_run_state"], ALLOWED_PERMISSION_STATES, "permission.picker_run_state")
    require_enum(permission["local_service_run_state"], ALLOWED_PERMISSION_STATES, "permission.local_service_run_state")

    observations = require_object(
        root["observations"],
        "observations",
        {
            "request_shape",
            "picker_without_broad_permission",
            "denied_permission_probe",
            "local_service_with_broad_permission",
            "candidate_ledger_model",
        },
    )
    request_shape = require_object(
        observations["request_shape"], "observations.request_shape", {"picker_flag", "result"}
    )
    if request_shape != {"picker_flag": "present", "result": "passed"}:
        raise ValueError("request_shape must record the picker flag as present and passed")

    picker = require_object(
        observations["picker_without_broad_permission"],
        "observations.picker_without_broad_permission",
        {"callback_outcome", "terminal_callback", "result"},
    )
    if picker != {"callback_outcome": "stopped", "terminal_callback": True, "result": "passed"}:
        raise ValueError("picker observation does not match the bounded stopped result")

    denied = require_object(
        observations["denied_permission_probe"],
        "observations.denied_permission_probe",
        {"permission_state", "outcome", "error_code", "service_found_callback", "result"},
    )
    if denied["permission_state"] != "denied" or denied["outcome"] != "callback_failed":
        raise ValueError("denied permission observation must be a callback failure")
    require_int(denied["error_code"], "denied_permission_probe.error_code", maximum=MAX_ERROR_CODE)
    if denied["service_found_callback"] != "not_observed" or denied["result"] != "passed":
        raise ValueError("denied permission observation must reject service delivery")

    local = require_object(
        observations["local_service_with_broad_permission"],
        "observations.local_service_with_broad_permission",
        {
            "registration_callback",
            "service_found_callback",
            "service_lost_callback",
            "candidate_trusted",
            "result",
        },
    )
    for field in ("registration_callback", "service_found_callback", "service_lost_callback"):
        if local[field] != "observed":
            raise ValueError(f"{field} must be observed")
    if local["candidate_trusted"] is not False or local["result"] != "passed":
        raise ValueError("local service candidate must remain untrusted")

    ledger = require_object(
        observations["candidate_ledger_model"],
        "observations.candidate_ledger_model",
        {"lost_removes_candidate", "unknown_loss_is_noop", "fixed_bound_eviction", "result"},
    )
    if ledger != {
        "lost_removes_candidate": True,
        "unknown_loss_is_noop": True,
        "fixed_bound_eviction": True,
        "result": "passed",
    }:
        raise ValueError("candidate ledger model must pass every bounded stale-candidate rule")

    instrumentation = require_object(
        root["instrumentation"], "instrumentation", {"runner", "tests_in_class", "result"}
    )
    if instrumentation["runner"] != "AndroidJUnitRunner":
        raise ValueError("instrumentation.runner must be AndroidJUnitRunner")
    if require_int(instrumentation["tests_in_class"], "instrumentation.tests_in_class") < 5:
        raise ValueError("instrumentation.tests_in_class is below the recorded probe set")
    if instrumentation["result"] != "ok":
        raise ValueError("instrumentation.result must be ok")

    limitations = root["limitations"]
    if not isinstance(limitations, list) or not limitations or any(
        not isinstance(item, str) or not item.strip() for item in limitations
    ):
        raise ValueError("limitations must be a nonempty list of strings")
    for value in iter_strings(root):
        if any(pattern.search(value) for pattern in FORBIDDEN_IDENTIFIERS):
            raise ValueError("record contains a stable device or random service identifier")


def iter_strings(value: Any):
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for key, child in value.items():
            yield from iter_strings(key)
            yield from iter_strings(child)
    elif isinstance(value, list):
        for child in value:
            yield from iter_strings(child)


class DiscoveryRecordTests(unittest.TestCase):
    def setUp(self) -> None:
        self.record = {
            "schema_version": 1,
            "recorded_at": "2026-11-22T07:42:32+08:00",
            "scenario": "android_17_discovery_lifecycle",
            "device": {"oem": "Google", "model": "sdk_gphone64_x86_64", "android_release": "17", "api_level": 37},
            "spike": {"package": "com.sonimesh.discoveryspike", "target_sdk": 37, "service_type": "_sonimesh._tcp", "callback_timeout_seconds": 10},
            "permission": {"name": "ACCESS_LOCAL_NETWORK", "picker_run_state": "denied", "local_service_run_state": "granted"},
            "observations": {
                "request_shape": {"picker_flag": "present", "result": "passed"},
                "picker_without_broad_permission": {"callback_outcome": "stopped", "terminal_callback": True, "result": "passed"},
                "denied_permission_probe": {"permission_state": "denied", "outcome": "callback_failed", "error_code": 7, "service_found_callback": "not_observed", "result": "passed"},
                "local_service_with_broad_permission": {"registration_callback": "observed", "service_found_callback": "observed", "service_lost_callback": "observed", "candidate_trusted": False, "result": "passed"},
                "candidate_ledger_model": {"lost_removes_candidate": True, "unknown_loss_is_noop": True, "fixed_bound_eviction": True, "result": "passed"},
            },
            "instrumentation": {"runner": "AndroidJUnitRunner", "tests_in_class": 7, "result": "ok"},
            "limitations": ["emulator only"],
        }

    def test_valid_record(self) -> None:
        validate_record(self.record)

    def test_rejects_extra_root_field(self) -> None:
        self.record["adb_serial"] = "emulator-5554"
        with self.assertRaises(ValueError):
            validate_record(self.record)

    def test_rejects_trusted_candidate(self) -> None:
        self.record["observations"]["local_service_with_broad_permission"]["candidate_trusted"] = True
        with self.assertRaises(ValueError):
            validate_record(self.record)

    def test_rejects_stable_or_random_identifiers(self) -> None:
        self.record["limitations"] = ["captured on emulator-5554"]
        with self.assertRaises(ValueError):
            validate_record(self.record)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("record", nargs="?", type=Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
    if args.self_test:
        result = unittest.TextTestRunner(verbosity=1).run(
            unittest.defaultTestLoader.loadTestsFromTestCase(DiscoveryRecordTests)
        )
        return 0 if result.wasSuccessful() else 1
    if args.record is None:
        parser.error("record is required unless --self-test is used")
    try:
        validate_record(json.loads(args.record.read_text(encoding="utf-8")))
    except (OSError, json.JSONDecodeError, ValueError) as error:
        print(f"discovery record invalid: {error}", file=sys.stderr)
        return 1
    print("discovery record: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
