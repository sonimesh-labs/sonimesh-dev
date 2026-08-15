#!/usr/bin/env python3
"""Keep declared `> Updated:` dates in sync with document content changes.

The design and measurement documents publish a `> Updated: YYYY-MM-DD` line as
the snapshot date of the content. When prose, evidence, or constraints change,
that date must advance; a stale date makes readers trust outdated boundaries.

A pure date-line edit (refreshing the `Updated:` value itself) must not count as
a content change, otherwise updating the date would immediately look stale on
the next run. This script walks the git history of each document and compares
the declared date against the newest commit that changed any line other than a
date declaration.
"""

from __future__ import annotations

import datetime
import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# Generated, vendored, or tool-owned directories that are not project docs.
SKIP_DIRECTORIES = frozenset(
    {
        ".agents",
        ".git",
        ".gradle-sandbox",
        ".playwright-cli",
        ".playwright-mcp",
        "artifacts",
        "corpus",
        "node_modules",
        "output",
        "target",
    }
)

DECLARATION_PREFIX = re.compile(r"^\s*>?\s*Updated\s*[:：]\s*")
VALID_DATE = re.compile(r"^20\d{2}-\d{2}-\d{2}$")


def _valid_calendar_date(value: str) -> bool:
    """Returns whether `value` is a real calendar date."""
    try:
        datetime.date.fromisoformat(value)
    except ValueError:
        return False
    return True


DIFF_HEADER_PATTERN = re.compile(r"^(---|\+\+\+|@@|diff --git|index |new file|deleted file|rename )")


def document_files(root: Path) -> list[Path]:
    """Returns every owned Markdown file under root, sorted by path."""
    return sorted(
        path
        for path in root.rglob("*.md")
        if not any(part in SKIP_DIRECTORIES for part in path.relative_to(root).parts)
    )


def declared_dates(source: str) -> list[tuple[int, str | None]]:
    """Returns (line number, date-or-None) for every `Updated:` declaration.

    A declaration whose value is not an ISO date contributes a `None` entry so
    the caller can report the malformed line.
    """
    results: list[tuple[int, str | None]] = []
    for index, line in enumerate(source.splitlines(), 1):
        match = DECLARATION_PREFIX.match(line)
        if match is None:
            continue
        date = line[match.end() :].strip()
        results.append(
            (
                index,
                date
                if VALID_DATE.fullmatch(date) and _valid_calendar_date(date)
                else None,
            )
        )
    return results


def git(root: Path, *arguments: str) -> subprocess.CompletedProcess[str]:
    """Runs git inside root and returns the completed process."""
    return subprocess.run(
        ["git", *arguments],
        cwd=root,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )


def commit_changes_content(root: Path, relative: str, commit: str) -> bool:
    """Returns whether `commit` changes any non-date line of `relative`.

    The patch is inspected line by line; removed or added blank lines count as
    content edits, while lines that only refresh a `> Updated:` declaration do
    not. When the patch cannot be inspected the commit is treated as content so
    the gate fails closed.
    """
    process = git(root, "show", "--format=", commit, "--", relative)
    if process.returncode != 0:
        return True
    for raw in process.stdout.splitlines():
        if not raw.startswith(("+", "-")):
            continue
        if DIFF_HEADER_PATTERN.match(raw):
            continue
        content = raw[1:]
        if not content.strip():
            return True
        if DECLARATION_PREFIX.match(content):
            continue
        return True
    return False


def content_commit_dates(root: Path, relative: Path) -> list[str]:
    """Returns dates of commits that changed content, newest first.

    Returns an empty list when git history is unavailable for the file (for
    example an untracked file or a shallow checkout); callers skip such files.
    """
    process = git(
        root,
        "log",
        "--format=%H %ad",
        "--date=short",
        "--",
        relative.as_posix(),
    )
    if process.returncode != 0:
        return []
    dates: list[str] = []
    for entry in process.stdout.splitlines():
        parts = entry.split(" ", 1)
        if len(parts) != 2:
            continue
        commit, date = parts
        if commit_changes_content(root, relative.as_posix(), commit):
            dates.append(date)
    return dates


def validate_workspace(root: Path) -> list[str]:
    """Returns stale or malformed date declarations across owned documents."""
    errors: list[str] = []
    for document in document_files(root):
        relative = document.relative_to(root)
        try:
            source = document.read_text(encoding="utf-8")
        except OSError as error:
            errors.append(f"unable to read {relative.as_posix()}: {error}")
            continue
        declarations = declared_dates(source)
        if not declarations:
            continue
        content_dates = content_commit_dates(root, relative)
        if not content_dates:
            continue
        latest_content_date = content_dates[0]
        for line, date in declarations:
            if date is None:
                errors.append(f"{relative.as_posix()}:{line} has a malformed Updated date")
            elif date < latest_content_date:
                errors.append(
                    f"{relative.as_posix()}:{line} declares Updated {date}, "
                    f"but content was last modified on {latest_content_date}"
                )
    return errors


def main() -> int:
    errors = validate_workspace(REPO_ROOT)
    for error in errors:
        print(error, file=sys.stderr)
    if errors:
        print(f"doc dates: {len(errors)} error(s)", file=sys.stderr)
        return 1
    print("doc dates: ok")
    return 0


class DocDateTests(unittest.TestCase):
    """Runs against a throwaway git repository so history is fully controlled."""

    def setUp(self) -> None:
        self._directory = tempfile.TemporaryDirectory()
        self.repo = Path(self._directory.name)
        git(self.repo, "init", "-q")
        git(self.repo, "config", "user.name", "test")
        git(self.repo, "config", "user.email", "test@example.com")

    def tearDown(self) -> None:
        self._directory.cleanup()

    def commit(self, relative: str, source: str, date: str) -> None:
        path = self.repo / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source, encoding="utf-8")
        environment = os.environ.copy()
        environment["GIT_AUTHOR_DATE"] = f"{date}T00:00:00+00:00"
        environment["GIT_COMMITTER_DATE"] = f"{date}T00:00:00+00:00"
        git(self.repo, "add", "--", relative)
        subprocess.run(
            ["git", "commit", "-q", "-m", "update"],
            cwd=self.repo,
            env=environment,
            check=True,
            capture_output=True,
            text=True,
        )

    def test_current_declared_date_is_valid(self) -> None:
        self.commit("docs/a.md", "# A\n\n> Updated: 2026-01-01\n\nprose\n", "2026-01-01")
        self.commit(
            "docs/a.md", "# A\n\n> Updated: 2026-02-01\n\nrevised prose\n", "2026-02-01"
        )
        self.assertEqual(validate_workspace(self.repo), [])

    def test_stale_declared_date_is_reported(self) -> None:
        self.commit("docs/a.md", "# A\n\n> Updated: 2026-01-01\n\nprose\n", "2026-01-01")
        self.commit("docs/a.md", "# A\n\n> Updated: 2026-01-01\n\nrevised prose\n", "2026-02-01")
        self.assertEqual(
            validate_workspace(self.repo),
            [
                "docs/a.md:3 declares Updated 2026-01-01, "
                "but content was last modified on 2026-02-01"
            ],
        )

    def test_date_only_edit_does_not_count_as_content(self) -> None:
        self.commit("docs/a.md", "# A\n\n> Updated: 2026-01-01\n\nprose\n", "2026-01-01")
        self.commit("docs/a.md", "# A\n\n> Updated: 2026-01-01\n\nrevised prose\n", "2026-02-01")
        self.commit("docs/a.md", "# A\n\n> Updated: 2026-02-01\n\nrevised prose\n", "2026-03-01")
        self.assertEqual(validate_workspace(self.repo), [])

    def test_malformed_date_is_reported(self) -> None:
        self.commit("docs/a.md", "# A\n\n> Updated: not-a-date\n\nprose\n", "2026-01-01")
        self.assertEqual(
            validate_workspace(self.repo),
            ["docs/a.md:3 has a malformed Updated date"],
        )

    def test_document_without_declared_date_is_ignored(self) -> None:
        self.commit("docs/b.md", "# B\n\nprose without a date\n", "2026-01-01")
        self.assertEqual(validate_workspace(self.repo), [])

    def test_untracked_document_is_skipped(self) -> None:
        path = self.repo / "docs" / "new.md"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("# New\n\n> Updated: 2026-01-01\n\nprose\n", encoding="utf-8")
        self.assertEqual(validate_workspace(self.repo), [])

    def test_each_declaration_is_checked_independently(self) -> None:
        source = (
            "# A\n\n"
            "> Updated: 2026-01-01\n\n"
            "> Updated: 2026-02-01\n\n"
            "prose\n"
        )
        self.commit("docs/a.md", source, "2026-01-01")
        self.commit("docs/a.md", source.replace("prose", "revised prose"), "2026-02-01")
        errors = validate_workspace(self.repo)
        self.assertEqual(len(errors), 1)
        self.assertIn("declares Updated 2026-01-01", errors[0])


    def test_impossible_calendar_date_is_reported(self) -> None:
        self.commit("docs/a.md", "# A\n\n> Updated: 2026-02-30\n\nprose\n", "2026-01-01")
        self.assertEqual(
            validate_workspace(self.repo),
            ["docs/a.md:3 has a malformed Updated date"],
        )

    def test_leap_day_is_accepted(self) -> None:
        self.commit("docs/a.md", "# A\n\n> Updated: 2024-02-29\n\nprose\n", "2024-02-29")
        self.assertEqual(validate_workspace(self.repo), [])


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
