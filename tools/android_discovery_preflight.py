#!/usr/bin/env python3
"""Read-only Android 17 local-network discovery preflight metadata."""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
import unittest
from collections.abc import Callable
from contextlib import redirect_stderr
from dataclasses import dataclass
from io import StringIO
from pathlib import Path
from unittest.mock import patch


PACKAGE_NAME = "com.sonimesh.sender"
MAX_PROPERTY_VALUE_LENGTH = 512
MAX_ADB_OUTPUT_LENGTH = 256 * 1024
MAX_PERMISSION_NAME_LENGTH = 256
REQUIRED_PLATFORM_PERMISSIONS = (
    "android.permission.ACCESS_LOCAL_NETWORK",
    "android.permission.NEARBY_WIFI_DEVICES",
)
PROPERTIES = (
    ("Manufacturer", "ro.product.manufacturer"),
    ("Model", "ro.product.model"),
    ("Android release", "ro.build.version.release"),
    ("Android API", "ro.build.version.sdk"),
    ("Build fingerprint", "ro.build.fingerprint"),
    ("ABI", "ro.product.cpu.abi"),
)


@dataclass(frozen=True)
class DiscoveryPreflightRecord:
    manufacturer: str
    model: str
    android_release: str
    android_api: str
    build_fingerprint: str
    abi: str
    capture_package_installed: bool
    capture_target_sdk: int | None
    platform_permissions: tuple[str, ...]
    requested_permissions: tuple[str, ...]


def adb_path(explicit: Path | None) -> str:
    """Returns an explicit, SDK-derived, or PATH-resolved ADB command path."""
    if explicit is not None:
        return str(explicit)
    sdk_root = os.environ.get("ANDROID_SDK_ROOT") or os.environ.get("ANDROID_HOME")
    if sdk_root:
        executable = "adb.exe" if os.name == "nt" else "adb"
        candidate = Path(sdk_root) / "platform-tools" / executable
        if candidate.is_file():
            return str(candidate)
    return "adb"


def validate_serial(serial: str) -> None:
    """Rejects empty or non-printable ADB selectors before any command is issued."""
    if not serial.strip() or not serial.isprintable():
        raise ValueError("ADB serial must be nonempty, printable, and single-line")


def redact_serial(value: str, serial: str) -> str:
    """Prevents a locally selected serial from entering preflight output."""
    return value.replace(serial, "<redacted-device>")


def validate_property_value(label: str, value: str) -> None:
    """Rejects untrusted property values that cannot be rendered safely in one line."""
    if len(value) > MAX_PROPERTY_VALUE_LENGTH or not value.isprintable():
        raise RuntimeError(f"connected device reported invalid {label} metadata")


def run_adb(
    adb: str,
    serial: str,
    arguments: list[str],
    operation: str,
    runner: Callable[..., subprocess.CompletedProcess[str]] = subprocess.run,
) -> str:
    """Runs one read-only ADB shell command without exposing the serial in output."""
    validate_serial(serial)
    completed = runner(
        [adb, "-s", serial, *arguments],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"ADB command failed while collecting {operation}")
    if len(completed.stdout) > MAX_ADB_OUTPUT_LENGTH:
        raise RuntimeError(f"ADB output exceeded the bound while collecting {operation}")
    return redact_serial(completed.stdout.strip(), serial)


def parse_target_sdk(package_dump: str) -> int | None:
    """Extracts the target SDK from one bounded package-manager dump."""
    match = re.search(r"\btargetSdk=(\d+)\b", package_dump)
    return int(match.group(1)) if match else None


def parse_permission_lines(package_dump: str) -> tuple[str, ...]:
    """Extracts requested permission names from the package dump presentation."""
    marker = "requested permissions:"
    _, separator, remainder = package_dump.partition(marker)
    if not separator:
        return ()
    names: list[str] = []
    for line in remainder.splitlines():
        stripped = line.strip()
        if not stripped or stripped.endswith(":"):
            if names:
                break
            continue
        if (
            len(stripped) <= MAX_PERMISSION_NAME_LENGTH
            and stripped.isprintable()
            and not any(character.isspace() for character in stripped)
            and (stripped.startswith("android.permission.") or stripped.startswith("com."))
        ):
            names.append(stripped)
    return tuple(dict.fromkeys(names))


def parse_platform_permissions(permission_dump: str) -> tuple[str, ...]:
    """Extracts the required Android 17 permission names from pm output."""
    lines = {line.strip() for line in permission_dump.splitlines()}
    return tuple(
        permission
        for permission in REQUIRED_PLATFORM_PERMISSIONS
        if f"permission:{permission}" in lines
    )


def collect_record(
    adb: str,
    serial: str,
    runner: Callable[..., subprocess.CompletedProcess[str]] = subprocess.run,
) -> DiscoveryPreflightRecord:
    """Collects bounded Android 17 discovery preparation fields."""
    values = {
        label: run_adb(adb, serial, ["shell", "getprop", property_name], f"{label} metadata", runner)
        for label, property_name in PROPERTIES
    }
    missing = [label for label, value in values.items() if not value]
    if missing:
        raise RuntimeError(f"connected device did not report required fields: {', '.join(missing)}")
    for label, value in values.items():
        validate_property_value(label, value)

    installed_packages = run_adb(
        adb,
        serial,
        ["shell", "pm", "list", "packages", "--user", "0", PACKAGE_NAME],
        "Capture package state",
        runner,
    )
    package_dump = run_adb(
        adb,
        serial,
        ["shell", "dumpsys", "package", PACKAGE_NAME],
        "Capture package manifest state",
        runner,
    )
    permission_dump = run_adb(
        adb,
        serial,
        ["shell", "pm", "list", "permissions", "-g", "-d"],
        "platform permission catalog",
        runner,
    )
    return DiscoveryPreflightRecord(
        manufacturer=values["Manufacturer"],
        model=values["Model"],
        android_release=values["Android release"],
        android_api=values["Android API"],
        build_fingerprint=values["Build fingerprint"],
        abi=values["ABI"],
        capture_package_installed=f"package:{PACKAGE_NAME}" in installed_packages.splitlines(),
        capture_target_sdk=parse_target_sdk(package_dump),
        platform_permissions=parse_platform_permissions(permission_dump),
        requested_permissions=parse_permission_lines(package_dump),
    )


def format_record(record: DiscoveryPreflightRecord) -> str:
    """Formats stable discovery preflight data without device selectors."""
    for label, value in (
        ("Manufacturer", record.manufacturer),
        ("Model", record.model),
        ("Android release", record.android_release),
        ("Android API", record.android_api),
        ("Build fingerprint", record.build_fingerprint),
        ("ABI", record.abi),
    ):
        validate_property_value(label, value)
    platform = ", ".join(record.platform_permissions) or "none"
    requested = ", ".join(record.requested_permissions) or "none"
    for permission in record.platform_permissions + record.requested_permissions:
        if (
            len(permission) > MAX_PERMISSION_NAME_LENGTH
            or not permission.isprintable()
            or any(character.isspace() for character in permission)
        ):
            raise RuntimeError("device reported invalid permission metadata")
    target = str(record.capture_target_sdk) if record.capture_target_sdk is not None else "unknown"
    package_state = "installed" if record.capture_package_installed else "not installed"
    return "\n".join(
        (
            "Android discovery preflight (read-only):",
            f"Device/OEM/model: {record.manufacturer} / {record.model}",
            f"Android release/API: {record.android_release} / {record.android_api}",
            f"Build fingerprint: {record.build_fingerprint}",
            f"ABI: {record.abi}",
            f"Capture package ({PACKAGE_NAME}): {package_state}",
            f"Capture target SDK: {target}",
            f"Required platform permissions defined: {platform}",
            f"Capture package requested permissions: {requested}",
        )
    )


class AndroidDiscoveryPreflightTests(unittest.TestCase):
    def test_collects_and_formats_bounded_discovery_metadata(self) -> None:
        values = {
            "ro.product.manufacturer": "Example OEM",
            "ro.product.model": "Model X",
            "ro.build.version.release": "17",
            "ro.build.version.sdk": "37",
            "ro.build.fingerprint": "example/model/build:17/ABC/123:userdebug/test-keys",
            "ro.product.cpu.abi": "x86_64",
        }
        package_dump = """versionCode=1 minSdk=31 targetSdk=36\nrequested permissions:\n  android.permission.RECORD_AUDIO\n  android.permission.FOREGROUND_SERVICE\n  com.example.INTERNAL\n\nUser 0:"""
        permission_dump = "permission:android.permission.ACCESS_LOCAL_NETWORK\npermission:android.permission.NEARBY_WIFI_DEVICES\n"

        def runner(command: list[str], **_: object) -> subprocess.CompletedProcess[str]:
            if command[-6:] == ["pm", "list", "packages", "--user", "0", PACKAGE_NAME]:
                return subprocess.CompletedProcess(command, 0, f"package:{PACKAGE_NAME}\n", "")
            if command[-3:] == ["dumpsys", "package", PACKAGE_NAME]:
                return subprocess.CompletedProcess(command, 0, package_dump, "")
            if command[-5:] == ["pm", "list", "permissions", "-g", "-d"]:
                return subprocess.CompletedProcess(command, 0, permission_dump, "")
            return subprocess.CompletedProcess(command, 0, values[command[-1]], "")

        record = collect_record("adb", "private-serial", runner)
        rendered = format_record(record)
        self.assertEqual(record.capture_target_sdk, 36)
        self.assertEqual(record.platform_permissions, REQUIRED_PLATFORM_PERMISSIONS)
        self.assertIn("target SDK: 36", rendered)
        self.assertIn("ACCESS_LOCAL_NETWORK", rendered)
        self.assertNotIn("private-serial", rendered)

    def test_rejects_empty_or_multiline_serial_before_adb(self) -> None:
        calls: list[list[str]] = []

        def runner(command: list[str], **_: object) -> subprocess.CompletedProcess[str]:
            calls.append(command)
            return subprocess.CompletedProcess(command, 0, "", "")

        for serial in ("", "  ", "private-serial\nnext-line", "private-serial\x1b[31m"):
            with self.subTest(serial=repr(serial)):
                with self.assertRaisesRegex(ValueError, "serial must be nonempty"):
                    collect_record("adb", serial, runner)
        self.assertEqual(calls, [])

    def test_parse_helpers_fail_closed_on_missing_or_hostile_data(self) -> None:
        self.assertIsNone(parse_target_sdk("package has no sdk"))
        self.assertEqual(parse_permission_lines("requested permissions:\n\nUser 0:"), ())
        self.assertEqual(parse_platform_permissions("permission:android.permission.INTERNET"), ())
        with self.assertRaisesRegex(RuntimeError, "invalid Model metadata"):
            validate_property_value("Model", "Model\nforged")

    def test_rejects_unbounded_adb_output_before_parsing(self) -> None:
        def runner(command: list[str], **_: object) -> subprocess.CompletedProcess[str]:
            return subprocess.CompletedProcess(command, 0, "x" * (MAX_ADB_OUTPUT_LENGTH + 1), "")

        with self.assertRaisesRegex(RuntimeError, "ADB output exceeded the bound"):
            run_adb("adb", "private-serial", ["shell", "getprop"], "metadata", runner)

    def test_main_reports_invalid_serial_without_traceback(self) -> None:
        stderr = StringIO()
        with patch.object(sys, "argv", ["android_discovery_preflight.py", "--serial", ""]):
            with redirect_stderr(stderr):
                self.assertEqual(main(), 1)
        self.assertEqual(
            stderr.getvalue(),
            "Android discovery preflight: failed: ADB serial must be nonempty, printable, and single-line\n",
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", required=True, help="local ADB serial; never printed")
    parser.add_argument("--adb", type=Path, help="explicit ADB executable path")
    arguments = parser.parse_args()
    try:
        record = collect_record(adb_path(arguments.adb), arguments.serial)
    except (OSError, RuntimeError, ValueError) as error:
        print(f"Android discovery preflight: failed: {error}", file=sys.stderr)
        return 1
    print(format_record(record))
    return 0


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
