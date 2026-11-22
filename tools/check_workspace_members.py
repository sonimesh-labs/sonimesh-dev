#!/usr/bin/env python3
"""Keep the Cargo workspace member list in sync with the crates on disk.

A crate directory that is not listed in `[workspace] members` is silently
omitted from `cargo test --workspace` and the other workspace-wide gates, so
its tests and lint checks never run. Conversely a member entry pointing at a
missing directory breaks every workspace command. This script compares the
declared members against every package manifest the repository owns (skipping
generated and vendored directories and independent nested workspace roots) and
fails on drift in either direction.
"""

from __future__ import annotations

import sys
import tempfile
import tomllib
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# Generated, vendored, or tool-owned directories that never host member crates.
# `.dev` also contains read-only reference checkouts used for implementation study.
SKIP_DIRECTORIES = frozenset(
    {
        ".agents",
        ".dev",
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


def load_manifest(path: Path) -> dict:
    """Loads one Cargo.toml as a plain dictionary."""
    with path.open("rb") as manifest:
        return tomllib.load(manifest)


def workspace_members(root: Path) -> list[str]:
    """Returns the declared member paths from the root workspace manifest."""
    manifest = load_manifest(root / "Cargo.toml")
    workspace = manifest.get("workspace")
    if not isinstance(workspace, dict):
        return []
    members = workspace.get("members")
    if not isinstance(members, list):
        return []
    return [str(member) for member in members]


def package_crate_dirs(root: Path) -> list[Path]:
    """Returns directories containing a package that belongs to this workspace.

    A manifest with its own `[workspace]` section is an independent workspace
    root (for example `fuzz/`) and is not a candidate member. The workspace
    root manifest itself has no `[package]` section and is also excluded.
    """
    crates: list[Path] = []
    for manifest in root.rglob("Cargo.toml"):
        relative = manifest.relative_to(root)
        if any(part in SKIP_DIRECTORIES for part in relative.parts):
            continue
        try:
            data = load_manifest(manifest)
        except (OSError, tomllib.TOMLDecodeError):
            continue
        if "package" not in data or "workspace" in data:
            continue
        crates.append(manifest.parent)
    return sorted(crates)


def normalize_member(root: Path, member: str) -> Path:
    """Resolves one declared member path relative to the workspace root."""
    return (root / member).resolve()


def validate_workspace(root: Path) -> list[str]:
    """Returns member-list drift errors, or an empty list when consistent."""
    declared = [normalize_member(root, member) for member in workspace_members(root)]
    errors: list[str] = []

    for member in declared:
        if not member.is_dir():
            errors.append(f"workspace member {member} does not exist")
            continue
        if not (member / "Cargo.toml").is_file():
            errors.append(f"workspace member {member} has no Cargo.toml")

    declared_paths = {member for member in declared if member.is_dir()}
    for crate in package_crate_dirs(root):
        absolute = crate.resolve()
        if absolute not in declared_paths:
            errors.append(
                f"crate {crate.relative_to(root).as_posix()} is not listed "
                "in workspace members"
            )
    return errors


def main() -> int:
    errors = validate_workspace(REPO_ROOT)
    for error in errors:
        print(error, file=sys.stderr)
    if errors:
        print(f"workspace members: {len(errors)} error(s)", file=sys.stderr)
        return 1
    print("workspace members: ok")
    return 0


class WorkspaceMemberTests(unittest.TestCase):
    """Runs against a throwaway directory tree of manifests."""

    def setUp(self) -> None:
        self._directory = tempfile.TemporaryDirectory()
        self.root = Path(self._directory.name)

    def tearDown(self) -> None:
        self._directory.cleanup()

    def write_member(self, relative: str, package: str) -> None:
        path = self.root / relative
        path.mkdir(parents=True, exist_ok=True)
        (path / "Cargo.toml").write_text(
            f"[package]\nname = \"{package}\"\nversion = \"0.1.0\"\n",
            encoding="utf-8",
        )

    def write_root_manifest(self, members: list[str]) -> None:
        quoted = "\n".join(f'    "{member}",' for member in members)
        (self.root / "Cargo.toml").write_text(
            "[workspace]\nmembers = [\n" + quoted + "\n]\n",
            encoding="utf-8",
        )

    def test_matching_members_and_crates_are_valid(self) -> None:
        self.write_root_manifest(["crates/a", "crates/b"])
        self.write_member("crates/a", "a")
        self.write_member("crates/b", "b")
        self.assertEqual(validate_workspace(self.root), [])

    def test_missing_member_directory_is_reported(self) -> None:
        self.write_root_manifest(["crates/missing"])
        errors = validate_workspace(self.root)
        self.assertEqual(len(errors), 1)
        self.assertIn("workspace member", errors[0])
        self.assertIn("does not exist", errors[0])

    def test_member_directory_without_manifest_is_reported(self) -> None:
        self.write_root_manifest(["tools/empty"])
        (self.root / "tools" / "empty").mkdir(parents=True, exist_ok=True)
        errors = validate_workspace(self.root)
        self.assertEqual(len(errors), 1)
        self.assertIn("has no Cargo.toml", errors[0])

    def test_unlisted_crate_is_reported(self) -> None:
        self.write_root_manifest(["crates/a"])
        self.write_member("crates/a", "a")
        self.write_member("crates/forgotten", "forgotten")
        errors = validate_workspace(self.root)
        self.assertEqual(len(errors), 1)
        self.assertIn("crates/forgotten is not listed in workspace members", errors[0])

    def test_independent_workspace_root_is_exempt(self) -> None:
        self.write_root_manifest(["crates/a"])
        self.write_member("crates/a", "a")
        fuzz = self.root / "fuzz"
        fuzz.mkdir(parents=True, exist_ok=True)
        (fuzz / "Cargo.toml").write_text(
            "[package]\nname = \"fuzz\"\n[workspace]\nmembers = [\".\"]\n",
            encoding="utf-8",
        )
        self.assertEqual(validate_workspace(self.root), [])

    def test_root_manifest_without_package_is_exempt(self) -> None:
        self.write_root_manifest([])
        (self.root / "Cargo.toml").write_text(
            "[workspace]\nmembers = []\n", encoding="utf-8"
        )
        self.assertEqual(validate_workspace(self.root), [])

    def test_vendored_directories_are_skipped(self) -> None:
        self.write_root_manifest([])
        vendored = self.root / "node_modules" / "dep"
        vendored.mkdir(parents=True, exist_ok=True)
        (vendored / "Cargo.toml").write_text(
            "[package]\nname = \"dep\"\n", encoding="utf-8"
        )
        self.assertEqual(validate_workspace(self.root), [])

    def test_read_only_reference_directories_are_skipped(self) -> None:
        self.write_root_manifest([])
        reference = self.root / ".dev" / "refs" / "example"
        reference.mkdir(parents=True, exist_ok=True)
        (reference / "Cargo.toml").write_text(
            "[package]\nname = \"example\"\n", encoding="utf-8"
        )
        self.assertEqual(validate_workspace(self.root), [])


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
