#!/usr/bin/env python3
"""Keep declared, documented, and finite-CI fuzz targets in sync."""

from __future__ import annotations

import re
import sys
import tempfile
import tomllib
import unittest
from collections.abc import Iterable
from pathlib import Path

DOC_TARGET_PATTERN = re.compile(r"^- `([a-z0-9_]+)`", re.MULTILINE)
MARKDOWN_HEADING_PATTERN = re.compile(r"^## [^\r\n]+[ \t]*$")
MARKDOWN_FENCE_PATTERN = re.compile(r"^[ \t]*(`{3,}|~{3,})")
TARGET_NAME_PATTERN = re.compile(r"^[a-z0-9_]+$")
SAFE_FUZZ_OPTION_PATTERN = re.compile(r"^-[A-Za-z0-9_]+=[A-Za-z0-9_./:+-]+$")
SMOKE_STEP_NAME = "      - name: Run finite fuzz smoke"
FUZZ_RUN_PREFIX = ("cargo", "+nightly-2026-07-18", "fuzz", "run")


def duplicate_names(names: Iterable[str]) -> list[str]:
    """Returns each duplicate target name once, in first-duplicate order."""
    seen: set[str] = set()
    duplicates: list[str] = []
    for name in names:
        if name in seen and name not in duplicates:
            duplicates.append(name)
        seen.add(name)
    return duplicates


def manifest_targets(source: str) -> list[tuple[str, str]]:
    """Returns declared fuzz target names and source paths from Cargo TOML."""
    manifest = tomllib.loads(source)
    bins = manifest.get("bin")
    if not isinstance(bins, list):
        raise ValueError("fuzz manifest has no [[bin]] targets")

    targets: list[tuple[str, str]] = []
    for entry in bins:
        if not isinstance(entry, dict):
            raise ValueError("fuzz manifest has a malformed [[bin]] target")
        name = entry.get("name")
        path = entry.get("path")
        if not isinstance(name, str) or not isinstance(path, str):
            raise ValueError("fuzz manifest target needs string name and path")
        targets.append((name, path))
    return targets


def markdown_section(source: str, heading: str) -> str:
    """Returns one exact level-two Markdown section outside fenced code blocks."""
    lines = source.splitlines(keepends=True)
    headings: list[tuple[int, str]] = []
    fence: str | None = None
    for index, line in enumerate(lines):
        content = line.rstrip("\r\n")
        fence_match = MARKDOWN_FENCE_PATTERN.match(content)
        if fence is not None:
            if fence_match is not None and fence_match.group(1)[0] == fence[0] and len(fence_match.group(1)) >= len(fence):
                fence = None
            continue
        if fence_match is not None:
            fence = fence_match.group(1)
            continue
        if MARKDOWN_HEADING_PATTERN.fullmatch(content) is not None:
            headings.append((index, content.rstrip(" \t")))

    section_indices = [index for index, title in headings if title == heading]
    if not section_indices:
        raise ValueError(f"fuzz README is missing section: {heading}")
    if len(section_indices) > 1:
        raise ValueError(f"fuzz README repeats section: {heading}")

    start = section_indices[0] + 1
    end = next((index for index, _ in headings if index >= start), len(lines))
    return "".join(lines[start:end])


def documented_targets(source: str) -> list[str]:
    """Returns fuzz target names from the dedicated README inventory section."""
    return DOC_TARGET_PATTERN.findall(markdown_section(source, "## Targets"))


def documented_run_targets(source: str) -> list[str]:
    """Returns targets from standalone commands in the README runnable-command section."""
    targets: list[str] = []
    for line in markdown_section(source, "## Run a target").splitlines():
        parts = line.strip().split()
        if len(parts) < 6 or tuple(parts[:4]) != FUZZ_RUN_PREFIX:
            continue
        target = parts[4]
        if (
            TARGET_NAME_PATTERN.fullmatch(target) is not None
            and parts[5] == "--"
            and all(SAFE_FUZZ_OPTION_PATTERN.fullmatch(option) is not None for option in parts[6:])
        ):
            targets.append(target)
    return targets


def smoke_script(source: str) -> tuple[str, str | None]:
    """Returns only the literal script body of the named finite-smoke CI step."""
    lines = source.splitlines()
    steps = [index for index, line in enumerate(lines) if line == SMOKE_STEP_NAME]
    if not steps:
        return "", "fuzz CI is missing the Run finite fuzz smoke step"
    if len(steps) > 1:
        return "", "fuzz CI repeats the Run finite fuzz smoke step"

    run_index: int | None = None
    for index in range(steps[0] + 1, len(lines)):
        line = lines[index]
        if line.strip() and not line.startswith("        "):
            break
        if line == "        run: |":
            run_index = index
            break
    if run_index is None:
        return "", "fuzz CI smoke step has no literal run script"

    commands: list[str] = []
    for line in lines[run_index + 1 :]:
        if not line.strip():
            continue
        if not line.startswith("          "):
            break
        commands.append(line[10:])
    if not commands:
        return "", "fuzz CI smoke step has no commands"
    return "\n".join(commands), None


def parse_finite_smoke_command(command: str) -> tuple[str | None, str | None]:
    """Validates one standalone cargo-fuzz invocation with exactly one positive run limit."""
    parts = command.split()
    if len(parts) < 7 or tuple(parts[:4]) != FUZZ_RUN_PREFIX:
        return None, "command must be one cargo-fuzz invocation"
    target = parts[4]
    if TARGET_NAME_PATTERN.fullmatch(target) is None or parts[5] != "--":
        return None, "command has an invalid cargo-fuzz target or separator"

    options = parts[6:]
    if any(SAFE_FUZZ_OPTION_PATTERN.fullmatch(option) is None for option in options):
        return None, "command has unsupported shell composition or argument syntax"
    runs = [option.removeprefix("-runs=") for option in options if option.startswith("-runs=")]
    if len(runs) != 1 or not runs[0].isdigit() or int(runs[0]) <= 0:
        return None, "command must have exactly one positive -runs option"
    return target, None


def finite_fuzz_run_targets(script: str) -> tuple[list[str], list[str]]:
    """Returns finite cargo-fuzz targets and deterministic script validation errors."""
    targets: list[str] = []
    errors: list[str] = []
    for line in script.splitlines():
        command = line.strip()
        if not command:
            continue
        target, error = parse_finite_smoke_command(command)
        if error is not None:
            errors.append(f"{error}: {command}")
        else:
            targets.append(target)
    return targets, errors


def inventory_errors(expected: list[str], observed: list[str], label: str) -> list[str]:
    """Returns duplicate, missing, and undeclared target names for one inventory."""
    errors: list[str] = []
    duplicates = duplicate_names(observed)
    if duplicates:
        errors.append(f"{label} repeats fuzz targets: {', '.join(duplicates)}")

    expected_names = set(expected)
    observed_names = set(observed)
    missing = sorted(expected_names - observed_names)
    unexpected = sorted(observed_names - expected_names)
    if missing:
        errors.append(f"{label} is missing fuzz targets: {', '.join(missing)}")
    if unexpected:
        errors.append(f"{label} has undeclared fuzz targets: {', '.join(unexpected)}")
    return errors


def validate_sources(manifest: str, readme: str, workflow: str, root: Path) -> list[str]:
    """Returns deterministic consistency errors across fuzz target inventories."""
    try:
        targets = manifest_targets(manifest)
        readme_targets = documented_targets(readme)
        readme_runs = documented_run_targets(readme)
    except (tomllib.TOMLDecodeError, ValueError) as error:
        return [f"fuzz target inventory is invalid: {error}"]

    declared = [name for name, _ in targets]
    errors: list[str] = []
    duplicates = duplicate_names(declared)
    if duplicates:
        errors.append(f"fuzz manifest repeats target names: {', '.join(duplicates)}")
    for name, path in targets:
        if TARGET_NAME_PATTERN.fullmatch(name) is None:
            errors.append(f"fuzz manifest has an invalid target name: {name}")
            continue
        expected_path = f"fuzz_targets/{name}.rs"
        if path != expected_path:
            errors.append(f"fuzz manifest target {name} must use source path: {expected_path}")
            continue
        if not (root / "fuzz" / path).is_file():
            errors.append(f"fuzz manifest target path does not exist: fuzz/{path}")

    errors.extend(inventory_errors(declared, readme_targets, "fuzz README"))
    errors.extend(inventory_errors(declared, readme_runs, "fuzz README commands"))
    script, script_error = smoke_script(workflow)
    if script_error is not None:
        errors.append(script_error)
        return errors
    smoke_targets, smoke_errors = finite_fuzz_run_targets(script)
    errors.extend(f"fuzz CI smoke has {error}" for error in smoke_errors)
    errors.extend(inventory_errors(declared, smoke_targets, "fuzz CI smoke"))
    return errors


def validate_workspace(root: Path) -> list[str]:
    """Returns errors for the checked-in fuzz manifests and CI workflow."""
    try:
        manifest = (root / "fuzz" / "Cargo.toml").read_text(encoding="utf-8")
        readme = (root / "fuzz" / "README.md").read_text(encoding="utf-8")
        workflow = (root / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
    except OSError as error:
        return [f"unable to read fuzz target inventory: {error}"]
    return validate_sources(manifest, readme, workflow, root)


def write_target(root: Path, name: str) -> None:
    """Creates one minimal canonical fuzz-target source fixture."""
    path = root / "fuzz" / "fuzz_targets" / f"{name}.rs"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.touch()


def main() -> int:
    errors = validate_workspace(Path.cwd())
    if errors:
        print("fuzz target inventory errors:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print("fuzz target inventory: ok")
    return 0


class FuzzTargetInventoryTests(unittest.TestCase):
    def sources(self, manifest: str, targets: str, commands: str, smoke: str) -> tuple[str, str, str]:
        readme = f"## Targets\n{targets}\n## Run a target\n{commands}"
        workflow = f"""      - name: Reproduction command
        run: cargo +nightly-2026-07-18 fuzz run control_frame -- -runs=1
      - name: Run finite fuzz smoke
        working-directory: fuzz
        run: |
{smoke}  android:
    name: Android capture bootstrap
    steps:
      - name: Later reproduction command
        run: |
          cargo +nightly-2026-07-18 fuzz run control_frame -- -runs=0
"""
        return manifest, readme, workflow

    def test_requires_real_unique_readme_sections_and_standalone_commands(self) -> None:
        with self.assertRaisesRegex(ValueError, "missing section"):
            markdown_section("prose ## Targets\n", "## Targets")
        with self.assertRaisesRegex(ValueError, "missing section"):
            markdown_section("```text\n## Targets\n```\n", "## Targets")
        with self.assertRaisesRegex(ValueError, "repeats section"):
            markdown_section("## Targets\nfirst\n## Targets\nsecond\n", "## Targets")

        manifest = """[[bin]]
name = "control_frame"
path = "fuzz_targets/control_frame.rs"
"""
        manifest, readme, workflow = self.sources(
            manifest,
            "- `control_frame` accepts arbitrary frames.\n",
            "true && cargo +nightly-2026-07-18 fuzz run control_frame -- -max_len=1\n",
            "          cargo +nightly-2026-07-18 fuzz run control_frame -- -runs=1\n",
        )
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_target(root, "control_frame")
            self.assertEqual(
                validate_sources(manifest, readme, workflow, root),
                ["fuzz README commands is missing fuzz targets: control_frame"],
            )

    def test_rejects_attached_shell_metacharacters_in_smoke_options(self) -> None:
        command_prefix = "cargo +nightly-2026-07-18 fuzz run control_frame -- -runs=1 -max_len=1"
        for suffix in (";:", "&&:", "||:", "|:", ">:", "<:", "$(:)", "`:`", "\\"):
            with self.subTest(suffix=suffix):
                self.assertEqual(
                    parse_finite_smoke_command(f"{command_prefix}{suffix}"),
                    (None, "command has unsupported shell composition or argument syntax"),
                )

    def test_rejects_duplicate_smoke_step_and_invalid_manifest_name(self) -> None:
        manifest = """[[bin]]
name = "../outside"
path = "fuzz_targets/../outside.rs"
"""
        manifest, readme, workflow = self.sources(
            manifest,
            "- `outside` is not the invalid manifest target.\n",
            "cargo +nightly-2026-07-18 fuzz run outside -- -max_len=1\n",
            "          cargo +nightly-2026-07-18 fuzz run outside -- -runs=1\n",
        )
        workflow += """      - name: Run finite fuzz smoke
        working-directory: fuzz
        run: |
          cargo +nightly-2026-07-18 fuzz run outside -- -runs=1
"""
        with tempfile.TemporaryDirectory() as directory:
            errors = validate_sources(manifest, readme, workflow, Path(directory))
            self.assertIn("fuzz manifest has an invalid target name: ../outside", errors)
            self.assertIn("fuzz CI repeats the Run finite fuzz smoke step", errors)

    def test_accepts_matching_inventories_and_ignores_other_ci_runs(self) -> None:
        manifest = """[[bin]]
name = "control_frame"
path = "fuzz_targets/control_frame.rs"

[[bin]]
name = "media_packet"
path = "fuzz_targets/media_packet.rs"
"""
        manifest, readme, workflow = self.sources(
            manifest,
            "- `control_frame` accepts arbitrary frames.\n- `media_packet` accepts arbitrary datagrams.\n",
            "cargo +nightly-2026-07-18 fuzz run control_frame -- -max_len=1\ncargo +nightly-2026-07-18 fuzz run media_packet -- -max_len=1\n",
            "          cargo +nightly-2026-07-18 fuzz run control_frame -- -runs=1\n          cargo +nightly-2026-07-18 fuzz run media_packet -- -runs=1\n",
        )
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_target(root, "control_frame")
            write_target(root, "media_packet")
            self.assertEqual(validate_sources(manifest, readme, workflow, root), [])

    def test_reports_missing_documentation_and_smoke_targets(self) -> None:
        manifest = """[[bin]]
name = "control_frame"
path = "fuzz_targets/control_frame.rs"

[[bin]]
name = "pairing_binding"
path = "fuzz_targets/pairing_binding.rs"
"""
        manifest, readme, workflow = self.sources(
            manifest,
            "- `control_frame` accepts arbitrary frames.\n",
            "cargo +nightly-2026-07-18 fuzz run control_frame -- -max_len=1\n",
            "          cargo +nightly-2026-07-18 fuzz run control_frame -- -runs=1\n",
        )
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_target(root, "control_frame")
            write_target(root, "pairing_binding")
            self.assertEqual(
                validate_sources(manifest, readme, workflow, root),
                [
                    "fuzz README is missing fuzz targets: pairing_binding",
                    "fuzz README commands is missing fuzz targets: pairing_binding",
                    "fuzz CI smoke is missing fuzz targets: pairing_binding",
                ],
            )

    def test_rejects_noncanonical_paths_and_composed_or_nonfinite_smoke_commands(self) -> None:
        manifest = """[[bin]]
name = "control_frame"
path = "../outside.rs"
"""
        manifest, readme, workflow = self.sources(
            manifest,
            "- `control_frame` accepts arbitrary frames.\n",
            "cargo +nightly-2026-07-18 fuzz run control_frame -- -max_len=1\n",
            "          cargo +nightly-2026-07-18 fuzz run control_frame -- -runs=1 \\\n          && cargo +nightly-2026-07-18 fuzz run control_frame -- -runs=0\n          true && cargo +nightly-2026-07-18 fuzz run control_frame -- -runs=0\n          cargo +nightly-2026-07-18 fuzz run control_frame -- -runs=1 -runs=0\n",
        )
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.assertEqual(
                validate_sources(manifest, readme, workflow, root),
                [
                    "fuzz manifest target control_frame must use source path: fuzz_targets/control_frame.rs",
                    "fuzz CI smoke has command has unsupported shell composition or argument syntax: cargo +nightly-2026-07-18 fuzz run control_frame -- -runs=1 \\",
                    "fuzz CI smoke has command must be one cargo-fuzz invocation: && cargo +nightly-2026-07-18 fuzz run control_frame -- -runs=0",
                    "fuzz CI smoke has command must be one cargo-fuzz invocation: true && cargo +nightly-2026-07-18 fuzz run control_frame -- -runs=0",
                    "fuzz CI smoke has command must have exactly one positive -runs option: cargo +nightly-2026-07-18 fuzz run control_frame -- -runs=1 -runs=0",
                    "fuzz CI smoke is missing fuzz targets: control_frame",
                ],
            )

    def test_reports_duplicate_and_unknown_targets(self) -> None:
        manifest = """[[bin]]
name = "control_frame"
path = "fuzz_targets/control_frame.rs"
"""
        manifest, readme, workflow = self.sources(
            manifest,
            "- `control_frame` accepts arbitrary frames.\n- `control_frame` repeats the target.\n- `unknown_target` is not declared.\n",
            "cargo +nightly-2026-07-18 fuzz run control_frame -- -max_len=1\ncargo +nightly-2026-07-18 fuzz run unknown_target -- -max_len=1\n",
            "          cargo +nightly-2026-07-18 fuzz run control_frame -- -runs=1\n          cargo +nightly-2026-07-18 fuzz run control_frame -- -runs=1\n",
        )
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            write_target(root, "control_frame")
            self.assertEqual(
                validate_sources(manifest, readme, workflow, root),
                [
                    "fuzz README repeats fuzz targets: control_frame",
                    "fuzz README has undeclared fuzz targets: unknown_target",
                    "fuzz README commands has undeclared fuzz targets: unknown_target",
                    "fuzz CI smoke repeats fuzz targets: control_frame",
                ],
            )


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
