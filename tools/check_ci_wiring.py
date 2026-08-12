#!/usr/bin/env python3
"""Verifies every tools/ checker is wired into the GitHub Actions CI job.

A checker that exists locally but is not invoked (or is invoked without its
self-test or normal mode) cannot enforce its contract in CI, and a CI reference
to a deleted or renamed checker is a stale gate. This script compares the
`tools/` inventory against `.github/workflows/ci.yml` so neither side can drift
without failing CI. Hardware-log and device-bound tools legitimately run only
`--self-test` in CI because their normal mode requires a measurement log or a
connected Android device.
"""

from __future__ import annotations

import re
import sys
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CI_PATH = REPO_ROOT / ".github" / "workflows" / "ci.yml"

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


def main() -> int:
    try:
        ci_text = CI_PATH.read_text(encoding="utf-8")
    except OSError as exc:
        print(f"cannot read CI workflow: {exc}", file=sys.stderr)
        return 2
    errors = ci_wiring_errors(ci_text)
    for error in errors:
        print(error, file=sys.stderr)
    if errors:
        print(f"CI wiring: {len(errors)} error(s)", file=sys.stderr)
        return 1
    print("CI wiring: ok")
    return 0


def wired_ci_text() -> str:
    """Renders a fully wired CI Python step from the current tools inventory."""
    lines = ["run: |"]
    for name in discover_tools():
        lines.append(f"  python tools/{name} --self-test")
        if name not in SELF_TEST_ONLY:
            lines.append(f"  python tools/{name}")
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


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
