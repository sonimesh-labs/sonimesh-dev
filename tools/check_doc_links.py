#!/usr/bin/env python3
"""Keep relative Markdown links pointing at files that exist.

The project documentation is the source of truth for architecture, security,
and measurement evidence, so a link that points at a missing or renamed file
silently loses that evidence. This script walks every Markdown file the
repository owns (excluding generated and vendored directories) and fails on any
relative link whose target does not resolve. External URLs, in-page anchors,
and links inside fenced code blocks are not validated because they are not
repository-relative evidence.
"""

from __future__ import annotations

import re
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

LINK_PATTERN = re.compile(r"\[[^\]]*\]\(([^)]+)\)")
MARKDOWN_FENCE_PATTERN = re.compile(r"^[ \t]*(`{3,}|~{3,})")


def document_files(root: Path) -> list[Path]:
    """Returns every owned Markdown file under root, sorted by path."""
    return sorted(
        path
        for path in root.rglob("*.md")
        if not any(part in SKIP_DIRECTORIES for part in path.relative_to(root).parts)
    )


def markdown_links(source: str) -> list[str]:
    """Returns link targets on lines outside fenced code blocks."""
    lines = source.splitlines()
    visible: list[str] = []
    fence: str | None = None
    for line in lines:
        match = MARKDOWN_FENCE_PATTERN.match(line)
        if fence is not None:
            if (
                match is not None
                and match.group(1)[0] == fence[0]
                and len(match.group(1)) >= len(fence)
            ):
                fence = None
            continue
        if match is not None:
            fence = match.group(1)
            continue
        visible.append(line)
    return LINK_PATTERN.findall("\n".join(visible))


def link_target(raw: str) -> str:
    """Normalizes one raw link target, stripping angle brackets and a title."""
    target = raw.strip()
    if target.startswith("<") and target.endswith(">"):
        target = target[1:-1].strip()
    head, separator, tail = target.partition(" ")
    if (
        separator
        and tail.startswith(('"', "'"))
        and tail.endswith(('"', "'"))
        and len(tail) >= 2
    ):
        target = head
    return target


def validate_documents(root: Path) -> list[str]:
    """Returns broken relative links across every owned Markdown document."""
    errors: list[str] = []
    for document in document_files(root):
        relative = document.relative_to(root)
        try:
            source = document.read_text(encoding="utf-8")
        except OSError as error:
            errors.append(f"unable to read {relative}: {error}")
            continue
        for raw in markdown_links(source):
            target = link_target(raw)
            if target.startswith(("http://", "https://", "mailto:", "#")):
                continue
            path_part = target.split("#", 1)[0]
            if not path_part:
                continue
            if not (document.parent / path_part).resolve().exists():
                errors.append(f"{relative} links missing file: {target}")
    return errors


def main() -> int:
    errors = validate_documents(REPO_ROOT)
    for error in errors:
        print(error, file=sys.stderr)
    if errors:
        print(f"doc links: {len(errors)} error(s)", file=sys.stderr)
        return 1
    print("doc links: ok")
    return 0


class DocLinkTests(unittest.TestCase):
    def test_valid_links_are_accepted(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "a.md").write_text(
                "[b](./b.md) [c](./sub/c.md) [web](https://example.com) [anchor](#x)",
                encoding="utf-8",
            )
            (root / "b.md").write_text("b", encoding="utf-8")
            (root / "sub").mkdir()
            (root / "sub" / "c.md").write_text("c", encoding="utf-8")
            self.assertEqual(validate_documents(root), [])

    def test_missing_link_is_reported(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "a.md").write_text("[missing](./missing.md)", encoding="utf-8")
            self.assertEqual(
                validate_documents(root),
                ["a.md links missing file: ./missing.md"],
            )

    def test_fenced_links_are_not_validated(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "a.md").write_text(
                "```text\n[example](./not-a-file.md)\n```\n",
                encoding="utf-8",
            )
            self.assertEqual(validate_documents(root), [])

    def test_skips_generated_directories(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "target").mkdir()
            (root / "target" / "x.md").write_text(
                "[missing](./missing.md)", encoding="utf-8"
            )
            (root / "a.md").write_text("ok", encoding="utf-8")
            self.assertEqual(validate_documents(root), [])

    def test_titled_and_angle_bracket_links_resolve(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "a.md").write_text(
                '[titled](./b.md "Title") [angled](<./b.md>)\n',
                encoding="utf-8",
            )
            (root / "b.md").write_text("b", encoding="utf-8")
            self.assertEqual(validate_documents(root), [])


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
