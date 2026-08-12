#!/usr/bin/env python3
"""Reject direct realtime-boundary regressions in reviewed realtime render paths.

The guarded functions are the Windows output adapter callback shells, the
app-crate `RenderSource::render` implementations and their render-path helpers,
and the fixed-slot mixer's callback entry points in `sonimesh-audio`. All of
them run inside the output callback, so they must avoid allocation, locking,
logging, blocking I/O, and device enumeration.
"""

from __future__ import annotations

import sys
import unittest
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class FunctionTarget:
    path: Path
    function: str


TARGETS = (
    FunctionTarget(
        Path("crates/sonimesh-platform/src/cpal_callback.rs"),
        "render_float32",
    ),
    FunctionTarget(
        Path("crates/sonimesh-platform/src/cpal_callback.rs"),
        "render_signed16",
    ),
    FunctionTarget(
        Path("tools/windows-wasapi-prototype/src/wasapi.rs"),
        "render_available",
    ),
    FunctionTarget(
        Path("crates/sonimesh-app/src/pcm_render_source.rs"),
        "render",
    ),
    FunctionTarget(
        Path("crates/sonimesh-app/src/pcm_render_source.rs"),
        "apply_gain",
    ),
    FunctionTarget(
        Path("crates/sonimesh-audio/src/fixed_slot_mixer.rs"),
        "render",
    ),
    FunctionTarget(
        Path("crates/sonimesh-audio/src/fixed_slot_mixer.rs"),
        "render_into",
    ),
    FunctionTarget(
        Path("crates/sonimesh-audio/src/fixed_slot_mixer.rs"),
        "apply_slot_gain",
    ),
    FunctionTarget(
        Path("crates/sonimesh-app/src/channel_mapped_render_source.rs"),
        "render",
    ),
    FunctionTarget(
        Path("crates/sonimesh-app/src/sample_rate_mapped_render_source.rs"),
        "render",
    ),
    FunctionTarget(
        Path("crates/sonimesh-app/src/sample_rate_mapped_render_source.rs"),
        "ensure_input",
    ),
    FunctionTarget(
        Path("crates/sonimesh-app/src/sample_rate_mapped_render_source.rs"),
        "consume_input",
    ),
)

BANNED_MARKERS = (
    "Arc::",
    "Box::",
    "Mutex",
    "RwLock",
    "Vec<",
    "Vec::",
    "eprintln!",
    "format!",
    "mpsc",
    "println!",
    "std::thread",
    "thread::",
    "WaitForSingleObject",
    "device_by_id",
    "output_devices",
    "vec!",
)


def function_body(source: str, function: str) -> str:
    """Returns one named function body, rejecting ambiguous or malformed source."""
    needle = f"fn {function}("
    start = source.find(needle)
    if start < 0:
        raise ValueError(f"missing function {function}")
    if source.find(needle, start + len(needle)) >= 0:
        raise ValueError(f"multiple functions named {function}")
    brace = source.find("{", start)
    if brace < 0:
        raise ValueError(f"function {function} has no opening brace")

    depth = 0
    for index in range(brace, len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return source[brace + 1 : index]
    raise ValueError(f"function {function} has no closing brace")


def validate_target(source: str, function: str) -> list[str]:
    """Returns direct forbidden markers in one reviewed realtime function."""
    body = function_body(source, function)
    return [marker for marker in BANNED_MARKERS if marker in body]


def validate_workspace(root: Path) -> list[str]:
    """Returns deterministic realtime-boundary violations for the checked-in shells."""
    violations: list[str] = []
    for target in TARGETS:
        try:
            source = (root / target.path).read_text(encoding="utf-8")
            markers = validate_target(source, target.function)
        except (OSError, ValueError) as error:
            violations.append(f"{target.path}: {error}")
            continue
        violations.extend(
            f"{target.path}:{target.function} contains forbidden realtime marker: {marker}"
            for marker in markers
        )
    return violations


def main() -> int:
    violations = validate_workspace(Path.cwd())
    if violations:
        print("output realtime boundary violations:", file=sys.stderr)
        for violation in violations:
            print(f"- {violation}", file=sys.stderr)
        return 1
    print("output realtime boundary: ok")
    return 0


class OutputRealtimeBoundaryTests(unittest.TestCase):
    def test_extracts_one_function_body(self) -> None:
        source = "fn render() { source.render(output); }\nfn other() {}"
        self.assertEqual(function_body(source, "render").strip(), "source.render(output);")

    def test_rejects_direct_allocation_and_logging(self) -> None:
        source = "fn render() { let samples = vec![0.0; 8]; println!(\"{samples:?}\"); }"
        self.assertEqual(validate_target(source, "render"), ["println!", "vec!"])

    def test_app_crate_render_path_is_guarded(self) -> None:
        app_crate_targets = {
            (target.path, target.function)
            for target in TARGETS
            if target.path.parts[:2] == ("crates", "sonimesh-app")
        }

        assert app_crate_targets == {
            (
                Path("crates/sonimesh-app/src/pcm_render_source.rs"),
                "render",
            ),
            (
                Path("crates/sonimesh-app/src/pcm_render_source.rs"),
                "apply_gain",
            ),
            (
                Path("crates/sonimesh-app/src/channel_mapped_render_source.rs"),
                "render",
            ),
            (
                Path("crates/sonimesh-app/src/sample_rate_mapped_render_source.rs"),
                "render",
            ),
            (
                Path("crates/sonimesh-app/src/sample_rate_mapped_render_source.rs"),
                "ensure_input",
            ),
            (
                Path("crates/sonimesh-app/src/sample_rate_mapped_render_source.rs"),
                "consume_input",
            ),
        }

    def test_audio_mixer_render_path_is_guarded(self) -> None:
        audio_mixer_targets = {
            (target.path, target.function)
            for target in TARGETS
            if target.path
            == Path("crates/sonimesh-audio/src/fixed_slot_mixer.rs")
        }

        assert audio_mixer_targets == {
            (
                Path("crates/sonimesh-audio/src/fixed_slot_mixer.rs"),
                "render",
            ),
            (
                Path("crates/sonimesh-audio/src/fixed_slot_mixer.rs"),
                "render_into",
            ),
            (
                Path("crates/sonimesh-audio/src/fixed_slot_mixer.rs"),
                "apply_slot_gain",
            ),
        }

    def test_rejects_missing_or_ambiguous_function(self) -> None:
        with self.assertRaisesRegex(ValueError, "missing function"):
            function_body("fn other() {}", "render")
        with self.assertRaisesRegex(ValueError, "multiple functions"):
            function_body("fn render() {} fn render() {}", "render")


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
