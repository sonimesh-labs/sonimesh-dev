#!/usr/bin/env python3
"""Reject direct realtime-boundary regressions in reviewed realtime render paths.

The guarded functions are the Windows output adapter callback shells, the
app-crate `RenderSource::render` implementations and their render-path helpers,
and the fixed-slot mixer's callback entry points in `sonimesh-audio` (including
the membership command drain that runs at the start of each render). All of
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
        Path("crates/sonimesh-audio/src/mixer_membership.rs"),
        "render",
    ),
    FunctionTarget(
        Path("crates/sonimesh-audio/src/mixer_membership.rs"),
        "drain_pending",
    ),
    FunctionTarget(
        Path("crates/sonimesh-app/src/mixer_render_source.rs"),
        "render",
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


def test_module_ranges(source: str) -> list[tuple[int, int]]:
    """Returns (start, end) ranges of `#[cfg(test)]`-gated modules.

    The guarded realtime functions are production code, so a test-only trait
    implementation must not count as a duplicate or shadow the production
    function. Only attributes immediately followed by a `mod <name> {` block
    are treated as test modules; anything else is scanned normally.
    """
    ranges: list[tuple[int, int]] = []
    search_from = 0
    while True:
        attribute = source.find("#[cfg(test)]", search_from)
        if attribute < 0:
            return ranges
        search_from = attribute + len("#[cfg(test)]")
        rest = source[search_from:]
        stripped = rest.lstrip()
        offset = search_from + len(rest) - len(stripped)
        if not stripped.startswith("mod "):
            continue
        brace = stripped.find("{")
        if brace < 0:
            continue
        start = offset + brace
        depth = 0
        for index in range(start, len(source)):
            if source[index] == "{":
                depth += 1
            elif source[index] == "}":
                depth -= 1
                if depth == 0:
                    ranges.append((attribute, index + 1))
                    search_from = index + 1
                    break
    return ranges


def function_body(source: str, function: str) -> str:
    """Returns one named function body, rejecting ambiguous or malformed source."""
    needle = f"fn {function}("
    test_ranges = test_module_ranges(source)

    def outside_tests(position: int) -> bool:
        return not any(start <= position < end for start, end in test_ranges)

    start = -1
    search_from = 0
    while True:
        candidate = source.find(needle, search_from)
        if candidate < 0:
            break
        if outside_tests(candidate):
            start = candidate
            search_from = candidate + len(needle)
            break
        search_from = candidate + len(needle)
    if start < 0:
        raise ValueError(f"missing function {function}")
    while True:
        duplicate = source.find(needle, search_from)
        if duplicate < 0:
            break
        if outside_tests(duplicate):
            raise ValueError(f"multiple functions named {function}")
        search_from = duplicate + len(needle)
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

    def test_skips_functions_inside_cfg_test_modules(self) -> None:
        source = (
            "fn render() { do_work(); }\n"
            "#[cfg(test)]\n"
            "mod tests {\n"
            "    impl RenderSource for FakeSource {\n"
            "        fn render(&mut self, output: &mut [f32]) { output.fill(0.0); }\n"
            "    }\n"
            "}\n"
        )
        self.assertEqual(function_body(source, "render").strip(), "do_work();")

    def test_test_module_does_not_mask_a_missing_function(self) -> None:
        source = "#[cfg(test)]\nmod tests { fn render() {} }\n"
        with self.assertRaisesRegex(ValueError, "missing function"):
            function_body(source, "render")

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
                Path("crates/sonimesh-app/src/mixer_render_source.rs"),
                "render",
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

    def test_membership_boundary_render_path_is_guarded(self) -> None:
        membership_targets = {
            (target.path, target.function)
            for target in TARGETS
            if target.path
            == Path("crates/sonimesh-audio/src/mixer_membership.rs")
        }

        assert membership_targets == {
            (
                Path("crates/sonimesh-audio/src/mixer_membership.rs"),
                "render",
            ),
            (
                Path("crates/sonimesh-audio/src/mixer_membership.rs"),
                "drain_pending",
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
