#!/usr/bin/env python3
"""Keep the architecture decision record index in sync with its files."""

from __future__ import annotations

import re
import sys
import tempfile
import unittest
from pathlib import Path

ADR_FILENAME_PATTERN = re.compile(r"^[0-9]{4}-[a-z0-9-]+\.md$")
RECORD_PATTERN = re.compile(
    r"^- \[ADR (?P<number>[0-9]{4}): [^\]\r\n]+\]\(\.\/(?P<filename>[0-9]{4}-[a-z0-9-]+\.md)\)"
)
MARKDOWN_HEADING_PATTERN = re.compile(r"^## [^\r\n]+[ \t]*$")
MARKDOWN_FENCE_PATTERN = re.compile(r"^[ \t]*(`{3,}|~{3,})")
RECORDS_HEADING = "## Records"


def adr_files(adr_directory: Path) -> list[tuple[int, str]]:
    """Returns canonical ADR numbers and filenames from the records directory."""
    records: list[tuple[int, str]] = []
    for path in sorted(adr_directory.glob("*.md")):
        if path.name == "README.md":
            continue
        match = ADR_FILENAME_PATTERN.fullmatch(path.name)
        if match is None:
            raise ValueError(f"non-canonical ADR filename: {path.name}")
        records.append((int(path.name[:4]), path.name))
    return records


def markdown_section(source: str, heading: str) -> str:
    """Returns one exact level-two Markdown section outside fenced code blocks."""
    lines = source.splitlines(keepends=True)
    headings: list[tuple[int, str]] = []
    fence: str | None = None
    for index, line in enumerate(lines):
        content = line.rstrip("\r\n")
        fence_match = MARKDOWN_FENCE_PATTERN.match(content)
        if fence is not None:
            if (
                fence_match is not None
                and fence_match.group(1)[0] == fence[0]
                and len(fence_match.group(1)) >= len(fence)
            ):
                fence = None
            continue
        if fence_match is not None:
            fence = fence_match.group(1)
            continue
        if MARKDOWN_HEADING_PATTERN.fullmatch(content) is not None:
            headings.append((index, content.rstrip(" \t")))

    section_indices = [index for index, title in headings if title == heading]
    if not section_indices:
        raise ValueError(f"ADR README is missing section: {heading}")
    if len(section_indices) > 1:
        raise ValueError(f"ADR README repeats section: {heading}")

    start = section_indices[0] + 1
    end = next((index for index, _ in headings if index >= start), len(lines))
    return "".join(lines[start:end])


def record_entries(source: str) -> tuple[list[tuple[int, str]], list[str]]:
    """Returns parsed record numbers/filenames and malformed record bullets."""
    entries: list[tuple[int, str]] = []
    malformed: list[str] = []
    for line in markdown_section(source, RECORDS_HEADING).splitlines():
        stripped = line.rstrip("\r\n")
        if not stripped.strip():
            continue
        if not stripped.startswith("- ["):
            continue
        match = RECORD_PATTERN.match(stripped)
        if match is None:
            malformed.append(stripped)
            continue
        entries.append((int(match.group("number")), match.group("filename")))
    return entries, malformed


def missing_numbers(numbers: list[int]) -> list[int]:
    """Returns the canonical numbers absent from a contiguous 1..N range."""
    present = set(numbers)
    highest = max(numbers, default=0)
    return [number for number in range(1, highest + 1) if number not in present]


def duplicate_entries(entries: list[tuple[int, str]]) -> list[str]:
    """Returns each filename that appears more than once, in first-duplicate order."""
    seen: set[str] = set()
    duplicates: list[str] = []
    for _, filename in entries:
        if filename in seen and filename not in duplicates:
            duplicates.append(filename)
        seen.add(filename)
    return duplicates


def validate_sources(readme: str, adr_directory: Path) -> list[str]:
    """Returns deterministic consistency errors across the ADR index."""
    try:
        files = adr_files(adr_directory)
        entries, malformed = record_entries(readme)
    except ValueError as error:
        return [f"ADR index is invalid: {error}"]

    errors: list[str] = []
    for line in malformed:
        errors.append(f"ADR README has a malformed record bullet: {line}")

    file_numbers = [number for number, _ in files]
    for number in missing_numbers(file_numbers):
        errors.append(f"docs/adr is missing ADR {number:04d}")

    file_names = {filename for _, filename in files}
    number_by_filename = {filename: number for number, filename in files}
    entry_by_filename: dict[str, tuple[int, str]] = {}
    seen_numbers: set[int] = set()
    for number, filename in entries:
        if number in seen_numbers:
            errors.append(f"ADR README repeats ADR {number:04d}")
        seen_numbers.add(number)
        entry_by_filename[filename] = (number, filename)
        if filename not in file_names:
            errors.append(f"ADR README links missing file: ./{filename}")
            continue
        expected_number = number_by_filename[filename]
        if number != expected_number:
            errors.append(
                f"ADR README ADR {number:04d} must link ./{expected_number:04d}-"
                f"{filename.split('-', 1)[1]}"
            )

    for filename in duplicate_entries(entries):
        errors.append(f"ADR README repeats record file: {filename}")

    for number, filename in files:
        if filename not in entry_by_filename:
            errors.append(f"ADR README is missing ADR {number:04d} ({filename})")
    return errors


def validate_workspace(root: Path) -> list[str]:
    """Returns errors for the checked-in ADR index and record files."""
    try:
        readme = (root / "docs" / "adr" / "README.md").read_text(encoding="utf-8")
    except OSError as error:
        return [f"unable to read ADR index: {error}"]
    return validate_sources(readme, root / "docs" / "adr")


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    errors = validate_workspace(root)
    for error in errors:
        print(error, file=sys.stderr)
    if errors:
        print(f"ADR index: {len(errors)} error(s)", file=sys.stderr)
        return 1
    print("ADR index: ok")
    return 0


class AdrIndexTests(unittest.TestCase):
    @staticmethod
    def write_records(directory: Path, filenames: list[str]) -> None:
        for filename in filenames:
            (directory / filename).write_text("# record\n", encoding="utf-8")

    def test_matching_index_and_files_are_valid(self) -> None:
        readme = """## Priority ADRs

1. **First decision** - context.

## Records

- [ADR 0001: First decision](./0001-first-decision.md) - accepted.
- [ADR 0002: Second decision](./0002-second-decision.md) - provisional.
- [ADR 0003: Third decision](./0003-third-decision.md) - deferred.
"""
        with tempfile.TemporaryDirectory() as directory:
            self.write_records(
                Path(directory),
                [
                    "0001-first-decision.md",
                    "0002-second-decision.md",
                    "0003-third-decision.md",
                ],
            )
            self.assertEqual(validate_sources(readme, Path(directory)), [])

    def test_reports_number_gaps_across_files(self) -> None:
        readme = """## Records

- [ADR 0001: First decision](./0001-first-decision.md) - accepted.
- [ADR 0003: Third decision](./0003-third-decision.md) - deferred.
"""
        with tempfile.TemporaryDirectory() as directory:
            self.write_records(
                Path(directory),
                ["0001-first-decision.md", "0003-third-decision.md"],
            )
            self.assertEqual(
                validate_sources(readme, Path(directory)),
                ["docs/adr is missing ADR 0002"],
            )

    def test_reports_link_to_missing_file_duplicate_and_missing_index_entry(self) -> None:
        readme = """## Records

- [ADR 0001: First decision](./0001-first-decision.md) - accepted.
- [ADR 0001: Duplicate decision](./0001-first-decision.md) - repeated.
- [ADR 0002: Missing decision](./0002-missing-decision.md) - deferred.
"""
        with tempfile.TemporaryDirectory() as directory:
            self.write_records(Path(directory), ["0001-first-decision.md"])
            self.assertEqual(
                validate_sources(readme, Path(directory)),
                [
                    "ADR README repeats ADR 0001",
                    "ADR README links missing file: ./0002-missing-decision.md",
                    "ADR README repeats record file: 0001-first-decision.md",
                ],
            )

    def test_reports_number_filename_mismatch_and_malformed_bullets(self) -> None:
        readme = """## Records

- [ADR 0002: Misnumbered](./0001-first-decision.md) - accepted.
- [ADR 0002: Broken link](./not-a-record.txt) - deferred.
"""
        with tempfile.TemporaryDirectory() as directory:
            self.write_records(Path(directory), ["0001-first-decision.md"])
            self.assertEqual(
                validate_sources(readme, Path(directory)),
                [
                    "ADR README has a malformed record bullet: - [ADR 0002: Broken link](./not-a-record.txt) - deferred.",
                    "ADR README ADR 0002 must link ./0001-first-decision.md",
                ],
            )

    def test_rejects_noncanonical_filenames_and_missing_records_section(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            adr_directory = Path(directory)
            (adr_directory / "0001.md").write_text("# record\n", encoding="utf-8")
            self.assertEqual(
                validate_sources("## Records\n\n", adr_directory),
                ["ADR index is invalid: non-canonical ADR filename: 0001.md"],
            )
            (adr_directory / "0001.md").unlink()
            self.assertEqual(
                validate_sources("## Overview\n", adr_directory),
                ["ADR index is invalid: ADR README is missing section: ## Records"],
            )


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
