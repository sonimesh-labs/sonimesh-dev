#!/usr/bin/env python3
"""Reject direct Rust dependencies that cross SoniMesh foundation boundaries.

Also enforces the documented standard-library-only contracts: packages in
``ZERO_DEPENDENCY_PACKAGES`` must not declare any direct, dev, or build
dependency, so a documented "no dependencies" boundary cannot silently
gain an external crate.
"""

from __future__ import annotations

import json
import subprocess
import sys
import unittest
from collections.abc import Iterable, Mapping
from typing import Any


FOUNDATION_DEPENDENCIES: dict[str, frozenset[str]] = {
    "sonimesh-protocol": frozenset(),
    "sonimesh-network": frozenset({"sonimesh-protocol"}),
    "sonimesh-audio": frozenset({"sonimesh-diagnostics"}),
    "sonimesh-platform": frozenset(),
    "sonimesh-diagnostics": frozenset(),
    "sonimesh-app": frozenset(
        {
            "sonimesh-audio",
            "sonimesh-diagnostics",
            "sonimesh-network",
            "sonimesh-platform",
            "sonimesh-protocol",
        }
    ),
}

# Packages documented as standard-library-only. Any direct, dev, or build
# dependency violates the published contract: sonimesh-diagnostics is
# "standard library only" in docs/development.md, and tools/latency-measurement
# is "intentionally dependency-free (standard library only)" in its README.
ZERO_DEPENDENCY_PACKAGES = frozenset(
    {
        "sonimesh-diagnostics",
        "sonimesh-latency-measurement",
    }
)


FORBIDDEN_DEPENDENCIES = frozenset(
    {
        "android-activity",
        "jni",
        "jni-sys",
        "ndk",
        "ndk-context",
        "tao",
        "tauri",
        "tauri-build",
        "vue",
        "webview2-com",
        "wry",
    }
)


def validate_metadata(metadata: Mapping[str, Any]) -> list[str]:
    """Returns architecture-policy violations for Cargo metadata output."""
    packages = metadata.get("packages")
    if not isinstance(packages, list):
        return ["cargo metadata did not contain a packages list"]

    package_dependencies = {
        package.get("name"): dependency_names(package.get("dependencies", []))
        for package in packages
        if isinstance(package, Mapping) and isinstance(package.get("name"), str)
    }
    package_all_dependencies = {
        package.get("name"): all_dependency_names(package)
        for package in packages
        if isinstance(package, Mapping) and isinstance(package.get("name"), str)
    }
    violations: list[str] = []

    for package, allowed_internal in FOUNDATION_DEPENDENCIES.items():
        dependencies = package_dependencies.get(package)
        if dependencies is None:
            violations.append(f"missing foundation package: {package}")
            continue

        actual_internal = frozenset(
            dependency
            for dependency in dependencies
            if dependency.startswith("sonimesh-")
        )
        if actual_internal != allowed_internal:
            unexpected = sorted(actual_internal - allowed_internal)
            missing = sorted(allowed_internal - actual_internal)
            if unexpected:
                violations.append(
                    f"{package} has forbidden internal dependencies: {', '.join(unexpected)}"
                )
            if missing:
                violations.append(
                    f"{package} is missing required internal dependencies: {', '.join(missing)}"
                )

        forbidden = sorted(dependencies & FORBIDDEN_DEPENDENCIES)
        if forbidden:
            violations.append(
                f"{package} has forbidden platform or UI dependencies: {', '.join(forbidden)}"
            )

    for package in sorted(ZERO_DEPENDENCY_PACKAGES):
        dependencies = package_dependencies.get(package)
        if dependencies is None:
            violations.append(f"missing zero-dependency package: {package}")
            continue
        unexpected = sorted(package_all_dependencies[package])
        if unexpected:
            violations.append(
                f"{package} must stay standard-library-only but depends on: {', '.join(unexpected)}"
            )

    return violations


def dependency_names(dependencies: object) -> set[str]:
    """Extracts direct dependency names from Cargo's package metadata."""
    if not isinstance(dependencies, Iterable):
        return set()
    return {
        name
        for dependency in dependencies
        if isinstance(dependency, Mapping)
        for name in [dependency.get("name")]
        if isinstance(name, str)
    }


def all_dependency_names(package: Mapping[str, Any]) -> set[str]:
    """Returns direct dependency names across normal, dev, and build tables."""
    names: set[str] = set()
    for table in ("dependencies", "dev_dependencies", "build_dependencies"):
        names.update(dependency_names(package.get(table, [])))
    return names


def cargo_metadata() -> Mapping[str, Any]:
    """Loads direct workspace dependency metadata without resolving transitive crates."""
    completed = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    )
    metadata = json.loads(completed.stdout)
    if not isinstance(metadata, Mapping):
        raise ValueError("cargo metadata root must be an object")
    return metadata


def main() -> int:
    try:
        violations = validate_metadata(cargo_metadata())
    except (OSError, subprocess.CalledProcessError, ValueError, json.JSONDecodeError) as error:
        print(f"workspace architecture check failed to read Cargo metadata: {error}", file=sys.stderr)
        return 2

    if violations:
        print("workspace architecture boundary violations:", file=sys.stderr)
        for violation in violations:
            print(f"- {violation}", file=sys.stderr)
        return 1

    print("workspace architecture boundaries: ok")
    return 0


class WorkspaceBoundaryTests(unittest.TestCase):
    def metadata_with(self, overrides: Mapping[str, Iterable[str]]) -> dict[str, Any]:
        packages = []
        for package, allowed in FOUNDATION_DEPENDENCIES.items():
            dependencies = overrides.get(package, allowed)
            packages.append(
                {
                    "name": package,
                    "dependencies": [{"name": dependency} for dependency in dependencies],
                }
            )
        for package in sorted(ZERO_DEPENDENCY_PACKAGES):
            dependencies = overrides.get(package, [])
            packages.append(
                {
                    "name": package,
                    "dependencies": [{"name": dependency} for dependency in dependencies],
                }
            )
        return {"packages": packages}

    def test_accepts_documented_foundation_edges(self) -> None:
        self.assertEqual(validate_metadata(self.metadata_with({})), [])

    def test_rejects_reversed_internal_dependency(self) -> None:
        violations = validate_metadata(
            self.metadata_with({"sonimesh-protocol": {"sonimesh-network"}})
        )
        self.assertEqual(
            violations,
            ["sonimesh-protocol has forbidden internal dependencies: sonimesh-network"],
        )

    def test_rejects_direct_ui_or_platform_dependency(self) -> None:
        metadata = self.metadata_with(
            {"sonimesh-network": {"sonimesh-protocol", "tauri", "jni"}}
        )
        self.assertEqual(
            validate_metadata(metadata),
            [
                "sonimesh-network has forbidden platform or UI dependencies: jni, tauri",
            ],
        )

    def test_reports_zero_dependency_violation(self) -> None:
        violations = validate_metadata(
            self.metadata_with({"sonimesh-latency-measurement": {"rand"}})
        )
        self.assertEqual(
            violations,
            [
                "sonimesh-latency-measurement must stay standard-library-only but depends on: rand",
            ],
        )

    def test_reports_missing_zero_dependency_package(self) -> None:
        # sonimesh-latency-measurement is not part of the foundation graph, so
        # this exercises only the zero-dependency missing-package path.
        metadata = self.metadata_with({})
        metadata["packages"] = [
            package
            for package in metadata["packages"]
            if package["name"] != "sonimesh-latency-measurement"
        ]
        violations = validate_metadata(metadata)
        self.assertEqual(
            violations,
            ["missing zero-dependency package: sonimesh-latency-measurement"],
        )


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
