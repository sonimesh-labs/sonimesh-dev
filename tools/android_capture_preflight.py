#!/usr/bin/env python3
"""Read-only Android capture-matrix preflight metadata collection."""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import unittest
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path


PACKAGE_NAME = "com.sonimesh.sender"
PROPERTIES = (
    ("Manufacturer", "ro.product.manufacturer"),
    ("Model", "ro.product.model"),
    ("Android release", "ro.build.version.release"),
    ("Android API", "ro.build.version.sdk"),
    ("Build fingerprint", "ro.build.fingerprint"),
    ("ABI", "ro.product.cpu.abi"),
)


@dataclass(frozen=True)
class DeviceRecord:
    manufacturer: str
    model: str
    android_release: str
    android_api: str
    build_fingerprint: str
    abi: str
    capture_package_installed: bool


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


def run_adb(
    adb: str,
    serial: str,
    arguments: list[str],
    runner: Callable[..., subprocess.CompletedProcess[str]] = subprocess.run,
) -> str:
    """Runs one read-only ADB shell command without exposing the serial in output."""
    completed = runner(
        [adb, "-s", serial, *arguments],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if completed.returncode != 0:
        detail = completed.stderr.strip() or "ADB command failed"
        raise RuntimeError(detail.replace(serial, "<redacted-device>"))
    return completed.stdout.strip()


def collect_record(
    adb: str,
    serial: str,
    runner: Callable[..., subprocess.CompletedProcess[str]] = subprocess.run,
) -> DeviceRecord:
    """Collects only the Android matrix preparation fields for one explicit device."""
    values = {
        label: run_adb(adb, serial, ["shell", "getprop", property_name], runner)
        for label, property_name in PROPERTIES
    }
    missing = [label for label, value in values.items() if not value]
    if missing:
        raise RuntimeError(f"connected device did not report required fields: {', '.join(missing)}")
    package_path = run_adb(adb, serial, ["shell", "cmd", "package", "path", PACKAGE_NAME], runner)
    return DeviceRecord(
        manufacturer=values["Manufacturer"],
        model=values["Model"],
        android_release=values["Android release"],
        android_api=values["Android API"],
        build_fingerprint=values["Build fingerprint"],
        abi=values["ABI"],
        capture_package_installed=package_path.startswith("package:"),
    )


def format_record(record: DeviceRecord) -> str:
    """Formats stable matrix preparation data without an ADB serial or source details."""
    package_state = "installed" if record.capture_package_installed else "not installed"
    return "\n".join(
        (
            "Android capture preflight (read-only):",
            f"Device/OEM/model: {record.manufacturer} / {record.model}",
            f"Android release/API: {record.android_release} / {record.android_api}",
            f"Build fingerprint: {record.build_fingerprint}",
            f"ABI: {record.abi}",
            f"Capture package ({PACKAGE_NAME}): {package_state}",
        )
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", required=True, help="local ADB serial; never printed")
    parser.add_argument("--adb", type=Path, help="explicit ADB executable path")
    arguments = parser.parse_args()
    try:
        record = collect_record(adb_path(arguments.adb), arguments.serial)
    except (OSError, RuntimeError) as error:
        print(f"Android capture preflight: failed: {error}", file=sys.stderr)
        return 1
    print(format_record(record))
    return 0


class AndroidCapturePreflightTests(unittest.TestCase):
    def test_collects_required_metadata_without_exposing_serial(self) -> None:
        values = {
            "ro.product.manufacturer": "Example OEM",
            "ro.product.model": "Model X",
            "ro.build.version.release": "15",
            "ro.build.version.sdk": "35",
            "ro.build.fingerprint": "example/model/build:15/ABC/123:userdebug/test-keys",
            "ro.product.cpu.abi": "arm64-v8a",
        }

        def runner(command: list[str], **_: object) -> subprocess.CompletedProcess[str]:
            property_name = command[-1]
            output = values.get(property_name, "package:/data/app/example/base.apk")
            return subprocess.CompletedProcess(command, 0, output, "")

        record = collect_record("adb", "private-serial", runner)
        rendered = format_record(record)
        self.assertTrue(record.capture_package_installed)
        self.assertIn("Example OEM / Model X", rendered)
        self.assertNotIn("private-serial", rendered)

    def test_rejects_missing_required_property_and_adb_failure(self) -> None:
        def missing_runner(command: list[str], **_: object) -> subprocess.CompletedProcess[str]:
            return subprocess.CompletedProcess(command, 0, "", "")

        with self.assertRaisesRegex(RuntimeError, "Manufacturer"):
            collect_record("adb", "private-serial", missing_runner)

        def failed_runner(command: list[str], **_: object) -> subprocess.CompletedProcess[str]:
            return subprocess.CompletedProcess(command, 1, "", "private-serial device offline")

        with self.assertRaisesRegex(RuntimeError, "device offline") as failure:
            collect_record("adb", "private-serial", failed_runner)
        self.assertNotIn("private-serial", str(failure.exception))

    def test_uses_sdk_adb_only_when_present(self) -> None:
        explicit = Path("C:/tools/adb.exe")
        self.assertEqual(adb_path(explicit), str(explicit))


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
