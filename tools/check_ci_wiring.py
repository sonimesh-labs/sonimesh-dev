#!/usr/bin/env python3
"""Verifies every tools/ checker is wired into CI and documented for contributors.

A checker that exists locally but is not invoked (or is invoked without its
self-test or normal mode) cannot enforce its contract in CI, and a CI reference
to a deleted or renamed checker is a stale gate. This script compares the
`tools/` inventory against `.github/workflows/ci.yml` so neither side can drift
without failing CI. Hardware-log and device-bound tools legitimately run only
`--self-test` in CI because their normal mode requires a measurement log or a
connected Android device.

The contributor-facing `## Core checks` block in `docs/development.md` is the
same contract from the developer side: a contributor following the documented
gates must run the same tools with the same modes CI enforces, so the block is
checked against the same inventory and mode rules.
"""

from __future__ import annotations

import re
import sys
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CI_PATH = REPO_ROOT / ".github" / "workflows" / "ci.yml"
DEVELOPMENT_PATH = REPO_ROOT / "docs" / "development.md"
CORE_CHECKS_HEADING = "## Core checks"

# Tools whose normal mode needs a measurement log or a connected device; CI
# runs only their `--self-test` mode.
SELF_TEST_ONLY = frozenset(
    {
        "android_capture_preflight.py",
        "check_android_capture_record.py",
        "check_output_lifecycle_log.py",
        "check_output_recovery_log.py",
        "check_output_soak_log.py",
    }
)


def discover_tools() -> list[str]:
    """Returns every standalone Python checker in tools/, sorted by name."""
    return sorted(path.name for path in (REPO_ROOT / "tools").glob("*.py"))


def ci_wiring_errors(ci_text: str) -> list[str]:
    """Returns wiring violations between the tools inventory and CI text."""
    ci_text = ci_text.replace("\r", "")
    tools = frozenset(discover_tools())
    referenced = set(re.findall(r"python\s+tools/([\w.]+)", ci_text))

    errors: list[str] = []
    stale = sorted(referenced - tools)
    if stale:
        errors.append(
            "CI references tools that do not exist: " + ", ".join(stale)
        )
    for name in sorted(tools):
        if not re.search(
            rf"python\s+tools/{re.escape(name)}\s+--self-test", ci_text
        ):
            errors.append(f"CI does not run {name} --self-test")
        if name not in SELF_TEST_ONLY and not re.search(
            rf"python\s+tools/{re.escape(name)}\s*$", ci_text, re.MULTILINE
        ):
            errors.append(f"CI does not run {name} in normal mode")
    return errors


def core_checks_block(development_text: str) -> tuple[str, str | None]:
    """Returns the literal Core checks section and any structural error.

    The section is the text between the `## Core checks` heading and the next
    level-two heading. The heading regex mirrors the one used by
    `check_fuzz_targets.py`; the section body contains no nested level-two
    headings, so fence tracking is unnecessary here.
    """
    text = development_text.replace("\r", "")
    lines = text.splitlines()
    headings = [
        index
        for index, line in enumerate(lines)
        if re.fullmatch(r"## [^\r\n]+[ \t]*", line) is not None
    ]
    starts = [
        index for index in headings if lines[index].strip() == CORE_CHECKS_HEADING
    ]
    if not starts:
        return "", "development docs are missing the Core checks section"
    if len(starts) > 1:
        return "", "development docs repeat the Core checks section"

    start = starts[0] + 1
    end = next((index for index in headings if index >= start), len(lines))
    return "\n".join(lines[start:end]), None


def core_checks_errors(development_text: str) -> list[str]:
    """Returns drift between the tools inventory and the Core checks block."""
    block, error = core_checks_block(development_text)
    if error is not None:
        return [error]

    tools = frozenset(discover_tools())
    referenced = set(re.findall(r"python\s+tools/([\w.]+)", block))
    errors: list[str] = []
    stale = sorted(referenced - tools)
    if stale:
        errors.append(
            "Core checks references tools that do not exist: " + ", ".join(stale)
        )
    for name in sorted(tools):
        if name in SELF_TEST_ONLY:
            if not re.search(
                rf"python\s+tools/{re.escape(name)}\s+--self-test", block
            ):
                errors.append(f"Core checks does not document {name} --self-test")
        elif not re.search(
            rf"python\s+tools/{re.escape(name)}\s*$", block, re.MULTILINE
        ):
            errors.append(f"Core checks does not document {name} in normal mode")
    return errors


def main() -> int:
    try:
        ci_text = CI_PATH.read_text(encoding="utf-8")
    except OSError as exc:
        print(f"cannot read CI workflow: {exc}", file=sys.stderr)
        return 2
    try:
        development_text = DEVELOPMENT_PATH.read_text(encoding="utf-8")
    except OSError as exc:
        print(f"cannot read development docs: {exc}", file=sys.stderr)
        return 2

    ci_errors = ci_wiring_errors(ci_text)
    core_errors = core_checks_errors(development_text)
    for error in ci_errors:
        print(error, file=sys.stderr)
    for error in core_errors:
        print(error, file=sys.stderr)
    if ci_errors:
        print(f"CI wiring: {len(ci_errors)} error(s)", file=sys.stderr)
    else:
        print("CI wiring: ok")
    if core_errors:
        print(f"Core checks: {len(core_errors)} error(s)", file=sys.stderr)
    else:
        print("Core checks: ok")
    return 1 if ci_errors or core_errors else 0


def wired_ci_text() -> str:
    """Renders a fully wired CI Python step from the current tools inventory."""
    lines = ["run: |"]
    for name in discover_tools():
        lines.append(f"  python tools/{name} --self-test")
        if name not in SELF_TEST_ONLY:
            lines.append(f"  python tools/{name}")
    return "\n".join(lines) + "\n"


def wired_core_checks_text() -> str:
    """Renders a fully documented Core checks block from the tools inventory."""
    lines = [CORE_CHECKS_HEADING, "", "~~~powershell"]
    for name in discover_tools():
        suffix = " --self-test" if name in SELF_TEST_ONLY else ""
        lines.append(f"python tools/{name}{suffix}")
    lines.extend(["~~~", ""])
    return "\n".join(lines) + "\n"


class CiWiringTests(unittest.TestCase):
    def test_accepts_a_fully_wired_ci_step(self) -> None:
        self.assertEqual(ci_wiring_errors(wired_ci_text()), [])

    def test_reports_a_missing_tool(self) -> None:
        text = wired_ci_text()
        missing = discover_tools()[0]
        text = text.replace(f"python tools/{missing} --self-test", "")
        errors = ci_wiring_errors(text)
        self.assertTrue(
            any(missing in error and "self-test" in error for error in errors)
        )

    def test_reports_a_missing_normal_mode(self) -> None:
        text = wired_ci_text()
        normal = next(
            name for name in discover_tools() if name not in SELF_TEST_ONLY
        )
        text = text.replace(f"  python tools/{normal}\n", "")
        errors = ci_wiring_errors(text)
        self.assertTrue(
            any(normal in error and "normal mode" in error for error in errors)
        )

    def test_reports_a_stale_ci_reference(self) -> None:
        text = wired_ci_text() + "  python tools/check_removed.py --self-test\n"
        errors = ci_wiring_errors(text)
        self.assertTrue(
            any(
                "do not exist" in error and "check_removed.py" in error
                for error in errors
            )
        )

    def test_hardware_bound_tools_do_not_need_normal_mode(self) -> None:
        for name in SELF_TEST_ONLY:
            text = wired_ci_text()
            text = text.replace(f"  python tools/{name} --self-test\n", "")
            errors = ci_wiring_errors(text)
            self.assertTrue(
                any(name in error and "self-test" in error for error in errors)
            )

    def test_accepts_a_fully_documented_core_checks_block(self) -> None:
        self.assertEqual(core_checks_errors(wired_core_checks_text()), [])

    def test_reports_a_missing_core_checks_tool(self) -> None:
        text = wired_core_checks_text()
        missing = discover_tools()[0]
        text = text.replace(f"python tools/{missing}", "")
        errors = core_checks_errors(text)
        self.assertTrue(
            any(
                missing in error
                and ("self-test" in error or "normal mode" in error)
                for error in errors
            )
        )

    def test_reports_a_missing_core_checks_self_test(self) -> None:
        text = wired_core_checks_text()
        hardware = next(iter(sorted(SELF_TEST_ONLY)))
        text = text.replace(f"python tools/{hardware} --self-test", f"python tools/{hardware}")
        errors = core_checks_errors(text)
        self.assertTrue(
            any(hardware in error and "self-test" in error for error in errors)
        )

    def test_reports_a_missing_core_checks_normal_mode(self) -> None:
        text = wired_core_checks_text()
        normal = next(
            name for name in discover_tools() if name not in SELF_TEST_ONLY
        )
        text = text.replace(f"python tools/{normal}\n", "")
        errors = core_checks_errors(text)
        self.assertTrue(
            any(normal in error and "normal mode" in error for error in errors)
        )

    def test_reports_a_stale_core_checks_reference(self) -> None:
        text = wired_core_checks_text() + "python tools/check_removed.py\n"
        errors = core_checks_errors(text)
        self.assertTrue(
            any(
                "do not exist" in error and "check_removed.py" in error
                for error in errors
            )
        )

    def test_reports_a_missing_core_checks_section(self) -> None:
        text = wired_core_checks_text().replace(CORE_CHECKS_HEADING + "\n", "")
        errors = core_checks_errors(text)
        self.assertEqual(
            errors,
            ["development docs are missing the Core checks section"],
        )


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
