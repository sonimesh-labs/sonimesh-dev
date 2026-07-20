#!/usr/bin/env python3
"""Reject production Android dependencies outside the Phase 0A capture boundary."""

from __future__ import annotations

import re
import sys
import unittest
from pathlib import Path


ANDROID_APP = Path("apps/android/app")
MAIN_MANIFEST = ANDROID_APP / "src/main/AndroidManifest.xml"
MAIN_SOURCE = ANDROID_APP / "src/main/java"
BUILD_SCRIPT = ANDROID_APP / "build.gradle.kts"

FORBIDDEN_MANIFEST_MARKERS = (
    "android.permission.INTERNET",
)

PRODUCTION_DEPENDENCY_SUFFIXES = (
    "implementation",
    "api",
    "compileOnly",
    "runtimeOnly",
    "kapt",
    "ksp",
)

TEST_DEPENDENCY_PREFIXES = (
    "test",
    "androidTest",
)

FORBIDDEN_PRODUCTION_DEPENDENCY_PREFIXES = (
    "io.ktor:",
    "com.squareup.okhttp3:",
    "org.conscrypt:",
    "org.bouncycastle:",
    "org.concentus:",
    "org.jitsi:",
    "com.github.luben:",
    "net.java.dev.jna:",
)

FORBIDDEN_SOURCE_MARKERS = (
    "import java.net.",
    "import javax.net.",
    "import javax.crypto.",
    "import java.security.KeyStore",
    "import android.security.keystore.",
    "import org.conscrypt.",
    "import org.bouncycastle.",
    "import org.concentus.",
    "import org.jitsi.",
    "import org.webrtc.",
    "import opus.",
    "System.loadLibrary(",
    "external fun ",
)


def source_files(root: Path) -> list[Path]:
    """Returns only production Kotlin sources under the configured source root."""
    return sorted(root.rglob("*.kt")) if root.is_dir() else []


def forbidden_markers(text: str, markers: tuple[str, ...]) -> list[str]:
    """Returns direct boundary markers that occur in one text artifact."""
    return [marker for marker in markers if marker in text]


def forbidden_production_dependencies(build_script: str) -> list[str]:
    """Finds forbidden artifacts declared by a production Gradle configuration."""
    pattern = re.compile(
        r"^\s*(?P<configuration>[A-Za-z][A-Za-z0-9_]*)\s*\(\s*(?:platform\s*\(\s*)?[\"'](?P<artifact>[^\"']+)[\"']",
        re.MULTILINE,
    )
    violations = []
    for match in pattern.finditer(build_script):
        configuration = match.group("configuration")
        if not is_production_dependency_configuration(configuration):
            continue
        artifact = match.group("artifact")
        if artifact.startswith(FORBIDDEN_PRODUCTION_DEPENDENCY_PREFIXES):
            violations.append(f"{configuration}({artifact})")
    return violations


def is_production_dependency_configuration(configuration: str) -> bool:
    """Returns whether a Gradle dependency configuration reaches an app variant."""
    if configuration.startswith(TEST_DEPENDENCY_PREFIXES):
        return False
    normalized = configuration.lower()
    for code_generation_prefix in ("kapt", "ksp"):
        if normalized.startswith(code_generation_prefix):
            return not normalized[len(code_generation_prefix) :].startswith(("test", "androidtest"))
    return normalized.endswith(tuple(suffix.lower() for suffix in PRODUCTION_DEPENDENCY_SUFFIXES))


def validate_android_capture_boundary(
    manifest: str,
    build_script: str,
    sources: dict[str, str],
) -> list[str]:
    """Returns deterministic Phase 0A production-boundary violations."""
    violations = [
        f"main manifest contains forbidden capture-only marker: {marker}"
        for marker in forbidden_markers(manifest, FORBIDDEN_MANIFEST_MARKERS)
    ]
    violations.extend(
        f"production build script contains forbidden capture-only dependency: {dependency}"
        for dependency in forbidden_production_dependencies(build_script)
    )
    for path, source in sorted(sources.items()):
        violations.extend(
            f"production source {path} contains forbidden capture-only marker: {marker}"
            for marker in forbidden_markers(source, FORBIDDEN_SOURCE_MARKERS)
        )
    return violations


def read_text(path: Path) -> str:
    """Reads one checked-in UTF-8 project artifact."""
    return path.read_text(encoding="utf-8")


def production_sources(root: Path) -> dict[str, str]:
    """Loads source contents keyed by stable path text for diagnostic output."""
    return {path.as_posix(): read_text(path) for path in source_files(root)}


def main() -> int:
    try:
        violations = validate_android_capture_boundary(
            read_text(MAIN_MANIFEST),
            read_text(BUILD_SCRIPT),
            production_sources(MAIN_SOURCE),
        )
    except OSError as error:
        print(f"Android capture boundary check failed to read project files: {error}", file=sys.stderr)
        return 2

    if violations:
        print("Android capture boundary violations:", file=sys.stderr)
        for violation in violations:
            print(f"- {violation}", file=sys.stderr)
        return 1

    print("Android capture boundary: ok")
    return 0


class AndroidCaptureBoundaryTests(unittest.TestCase):
    def test_accepts_capture_only_production_artifacts(self) -> None:
        self.assertEqual(
            validate_android_capture_boundary(
                '<manifest><uses-permission android:name="android.permission.RECORD_AUDIO" /></manifest>',
                'dependencies { implementation("androidx.core:core-ktx:1.13.1") }',
                {"CaptureService.kt": "import android.media.AudioRecord"},
            ),
            [],
        )

    def test_rejects_network_keystore_and_native_markers(self) -> None:
        violations = validate_android_capture_boundary(
            '<uses-permission android:name="android.permission.INTERNET" />',
            (
                'api("org.conscrypt:conscrypt-android:2.6.1")\n'
                'runtimeOnly("org.concentus:concentus:1.0.0")\n'
                'debugImplementation("io.ktor:ktor-client-core:3.0.0")\n'
                'freeReleaseApi("org.bouncycastle:bcprov-jdk18on:1.0.0")\n'
                'kspDebug("org.jitsi:jitsi-opus:1.0.0")\n'
                'kaptTest("org.conscrypt:conscrypt-android:2.6.1")\n'
                'androidTestImplementation("org.conscrypt:conscrypt-android:2.6.1")'
            ),
            {
                "CaptureService.kt": (
                    "import java.net.Socket\n"
                    "import android.security.keystore.KeyGenParameterSpec\n"
                    "external fun decode()"
                )
            },
        )
        self.assertEqual(
            violations,
            [
                "main manifest contains forbidden capture-only marker: android.permission.INTERNET",
                "production build script contains forbidden capture-only dependency: api(org.conscrypt:conscrypt-android:2.6.1)",
                "production build script contains forbidden capture-only dependency: runtimeOnly(org.concentus:concentus:1.0.0)",
                "production build script contains forbidden capture-only dependency: debugImplementation(io.ktor:ktor-client-core:3.0.0)",
                "production build script contains forbidden capture-only dependency: freeReleaseApi(org.bouncycastle:bcprov-jdk18on:1.0.0)",
                "production build script contains forbidden capture-only dependency: kspDebug(org.jitsi:jitsi-opus:1.0.0)",
                "production source CaptureService.kt contains forbidden capture-only marker: import java.net.",
                "production source CaptureService.kt contains forbidden capture-only marker: import android.security.keystore.",
                "production source CaptureService.kt contains forbidden capture-only marker: external fun ",
            ],
        )


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
