#!/usr/bin/env python3
"""Cross-language check of the sender sequence guard and receiver replay window.

`MediaSequence` (crates/sonimesh-protocol/src/media_key.rs) issues each media
sequence exactly once and refuses reuse after `u32::MAX`; `ReplayWindow`
(crates/sonimesh-protocol/src/replay.rs) keeps a fixed 64-packet authenticated
replay window that advances only for authenticated in-window packets. The
Android host-JVM mirrors (`MediaSequence.kt`, `ReplayWindow.kt`) and their
vector tests pin the same boundary semantics. This script reimplements both
boundaries in pure standard-library Python, verifies the published scenarios,
and parses the Rust and Kotlin pins so the three implementations cannot drift
apart (roadmap Phase 0D: replay, nonce, key-epoch, and address-migration
tests; testing doc: sequence exhaustion, duplicate detection, and replay
windows).
"""

from __future__ import annotations

import re
import sys
import unittest
from pathlib import Path

# Published boundary constants (mirror of `replay.rs` and the Kotlin
# `ReplayWindow` mirror; the Rust `WINDOW_BITS` and Kotlin
# `REPLAY_WINDOW_BITS` pins below are re-read from the repo).
REPLAY_WINDOW_BITS = 64
U32_MAX = 2**32 - 1

REPO_ROOT = Path(__file__).resolve().parent.parent
RUST_REPLAY_PATH = REPO_ROOT / "crates" / "sonimesh-protocol" / "src" / "replay.rs"
RUST_MEDIA_KEY_PATH = (
    REPO_ROOT / "crates" / "sonimesh-protocol" / "src" / "media_key.rs"
)
RUST_MEDIA_CRYPTO_PATH = (
    REPO_ROOT / "crates" / "sonimesh-protocol" / "src" / "media_crypto.rs"
)
KOTLIN_DIR = (
    REPO_ROOT
    / "apps"
    / "android"
    / "app"
    / "src"
    / "test"
    / "java"
    / "com"
    / "sonimesh"
    / "sender"
    / "security"
)
KOTLIN_REPLAY_PATH = KOTLIN_DIR / "ReplayWindow.kt"
KOTLIN_REPLAY_TEST_PATH = KOTLIN_DIR / "ReplayWindowVectorTest.kt"
KOTLIN_SEQUENCE_PATH = KOTLIN_DIR / "MediaSequence.kt"
KOTLIN_SEQUENCE_TEST_PATH = KOTLIN_DIR / "MediaSequenceVectorTest.kt"

# Canonical `ReplayDecision` variant names in declaration order.
RUST_REPLAY_DECISIONS = ("Accepted", "Duplicate", "TooOld")
KOTLIN_REPLAY_DECISIONS = ("ACCEPTED", "DUPLICATE", "TOO_OLD")

# Test names that pin the same boundary semantics on each side. The names are
# pairs so a rename or deletion on either side fails the check.
RUST_REPLAY_TEST_NAMES = (
    "accepts_reordering_once",
    "rejects_values_outside_window",
    "large_jump_resets_bitmap_without_allocating",
    "keeps_the_oldest_sequence_in_window_after_a_maximum_in_window_shift",
    "accepts_the_oldest_in_window_sequence_at_the_maximum_value",
)
KOTLIN_REPLAY_TEST_NAMES = (
    "acceptsReorderingOnce",
    "rejectsValuesOutsideWindow",
    "largeJumpResetsBitmapWithoutAllocating",
    "keepsTheOldestSequenceInWindowAfterAMaximumInWindowShift",
    "acceptsTheOldestInWindowSequenceAtTheMaximumValue",
)
KOTLIN_SEQUENCE_TEST_NAMES = (
    "issuesSequencesInOrderFromANonZeroStart",
    "refusesToReuseASequenceAfterMaxValue",
    "freshEpochStartsAtZero",
)
RUST_SEALER_EXHAUSTION_TEST = "sealer_refuses_to_issue_a_sequence_beyond_u32_max"


class ReplayWindow:
    """Independent model of the fixed-size authenticated replay window."""

    def __init__(self) -> None:
        self.highest: int | None = None
        self.seen = 0

    def accept_authenticated(self, sequence: int) -> str:
        """Returns ACCEPTED, DUPLICATE, or TOO_OLD for one authenticated packet."""
        mask = (1 << REPLAY_WINDOW_BITS) - 1
        if self.highest is None:
            self.highest = sequence
            self.seen = 1
            return "ACCEPTED"

        if sequence > self.highest:
            shift = sequence - self.highest
            self.seen = (
                1
                if shift >= REPLAY_WINDOW_BITS
                else ((self.seen << shift) | 1) & mask
            )
            self.highest = sequence
            return "ACCEPTED"

        distance = self.highest - sequence
        if distance >= REPLAY_WINDOW_BITS:
            return "TOO_OLD"
        bit = 1 << distance
        if self.seen & bit:
            return "DUPLICATE"
        self.seen |= bit
        return "ACCEPTED"


class MediaSequence:
    """Independent model of the sender sequence exhaustion guard."""

    def __init__(self, initial: int) -> None:
        self.next: int | None = initial

    def take_next(self) -> int | None:
        """Returns the next sequence or None after u32::MAX has been issued."""
        sequence = self.next
        if sequence is None:
            return None
        self.next = None if sequence == U32_MAX else sequence + 1
        return sequence


# Canonical scenarios shared by the Rust unit tests, the Kotlin vector tests,
# and this independent model.
REPLAY_SCENARIOS: dict[str, list[tuple[int, str]]] = {
    "accepts_reordering_once": [
        (10, "ACCEPTED"),
        (12, "ACCEPTED"),
        (11, "ACCEPTED"),
        (11, "DUPLICATE"),
    ],
    "rejects_values_outside_window": [
        (1, "ACCEPTED"),
        (65, "ACCEPTED"),
        (1, "TOO_OLD"),
    ],
    "large_jump_resets_bitmap_without_allocating": [
        (2, "ACCEPTED"),
        (1000, "ACCEPTED"),
        (999, "ACCEPTED"),
    ],
    "keeps_the_oldest_sequence_in_window_after_a_maximum_in_window_shift": [
        (1, "ACCEPTED"),
        (1 + (REPLAY_WINDOW_BITS - 1), "ACCEPTED"),
        (1, "DUPLICATE"),
        (1 + REPLAY_WINDOW_BITS, "ACCEPTED"),
        (1, "TOO_OLD"),
    ],
    "accepts_the_oldest_in_window_sequence_at_the_maximum_value": [
        (U32_MAX, "ACCEPTED"),
        (U32_MAX - (REPLAY_WINDOW_BITS - 1), "ACCEPTED"),
        (U32_MAX - REPLAY_WINDOW_BITS, "TOO_OLD"),
        (U32_MAX - (REPLAY_WINDOW_BITS - 1), "DUPLICATE"),
    ],
}

SEQUENCE_SCENARIOS: dict[str, tuple[int, list[int | None]]] = {
    "issuesSequencesInOrderFromANonZeroStart": (5, [5, 6, 7]),
    "refusesToReuseASequenceAfterMaxValue": (U32_MAX, [U32_MAX, None, None]),
    "freshEpochStartsAtZero": (0, [0, 1]),
}


def rust_replay_window_bits(source: str) -> int:
    """Reads `const WINDOW_BITS: u32 = ...;` from replay.rs."""
    match = re.search(r"const WINDOW_BITS\s*:\s*u32\s*=\s*(\d+)\s*;", source)
    if match is None:
        raise ValueError("Rust replay.rs does not declare WINDOW_BITS")
    return int(match.group(1))


def rust_replay_decision_variants(source: str) -> list[str]:
    """Reads the `ReplayDecision` variants from replay.rs in declaration order."""
    match = re.search(r"pub enum ReplayDecision\s*\{(.*?)\}", source, re.DOTALL)
    if match is None:
        raise ValueError("Rust replay.rs does not declare ReplayDecision")
    body = "\n".join(
        line for line in match.group(1).splitlines() if not line.lstrip().startswith("///")
    )
    variants = re.findall(r"^\s*([A-Za-z_][A-Za-z0-9_]*)\s*,", body, re.MULTILINE)
    if not variants:
        raise ValueError("Rust ReplayDecision has no parseable variants")
    return variants


def kotlin_replay_window_bits(source: str) -> int:
    """Reads `val REPLAY_WINDOW_BITS = ...u` from ReplayWindow.kt."""
    match = re.search(r"val REPLAY_WINDOW_BITS\s*=\s*(\d+)\s*u", source)
    if match is None:
        raise ValueError("Kotlin ReplayWindow.kt does not declare REPLAY_WINDOW_BITS")
    return int(match.group(1))


def kotlin_replay_decision_variants(source: str) -> list[str]:
    """Reads the `ReplayDecision` enum variants from ReplayWindow.kt."""
    match = re.search(r"enum class ReplayDecision\s*\{(.*?)\}", source, re.DOTALL)
    if match is None:
        raise ValueError("Kotlin ReplayWindow.kt does not declare ReplayDecision")
    variants = re.findall(r"^\s*([A-Za-z_][A-Za-z0-9_]*)\s*,", match.group(1), re.MULTILINE)
    if not variants:
        raise ValueError("Kotlin ReplayDecision has no parseable variants")
    return variants


def contains_rust_test(source: str, name: str) -> bool:
    return re.search(rf"fn {re.escape(name)}\s*\(", source) is not None


def contains_kotlin_test(source: str, name: str) -> bool:
    return re.search(rf"fun {re.escape(name)}\s*\(", source) is not None


def rust_sequence_pins(source: str) -> list[str]:
    """Checks the Rust `MediaSequence` exhaustion-guard expressions."""
    required = (
        "pub struct MediaSequence {",
        "fn take_next(",
        "checked_add(1)",
        "ok_or(MediaCryptoError::SequenceExhausted)",
    )
    return [needle for needle in required if needle not in source]


def kotlin_sequence_pins(source: str) -> list[str]:
    """Checks the Kotlin `MediaSequence` exhaustion-guard expressions."""
    required = (
        "class MediaSequence",
        "fun takeNext(): UInt?",
        "UInt.MAX_VALUE",
    )
    return [needle for needle in required if needle not in source]


def validate_sequence_replay() -> list[str]:
    """Returns every drift error between the model, Rust, and Kotlin pins."""
    errors: list[str] = []

    rust_replay_source = RUST_REPLAY_PATH.read_text(encoding="utf-8")
    try:
        rust_bits = rust_replay_window_bits(rust_replay_source)
    except ValueError as error:
        errors.append(str(error))
        rust_bits = None
    if rust_bits is not None and rust_bits != REPLAY_WINDOW_BITS:
        errors.append(
            f"Rust replay window bits {rust_bits} differ from the published {REPLAY_WINDOW_BITS}"
        )

    kotlin_replay_source = KOTLIN_REPLAY_PATH.read_text(encoding="utf-8")
    try:
        kotlin_bits = kotlin_replay_window_bits(kotlin_replay_source)
    except ValueError as error:
        errors.append(str(error))
        kotlin_bits = None
    if kotlin_bits is not None and kotlin_bits != REPLAY_WINDOW_BITS:
        errors.append(
            f"Kotlin replay window bits {kotlin_bits} differ from the published {REPLAY_WINDOW_BITS}"
        )

    try:
        rust_variants = rust_replay_decision_variants(rust_replay_source)
    except ValueError as error:
        errors.append(str(error))
        rust_variants = []
    if rust_variants != list(RUST_REPLAY_DECISIONS):
        errors.append(
            f"Rust ReplayDecision variants {rust_variants} differ from {list(RUST_REPLAY_DECISIONS)}"
        )

    try:
        kotlin_variants = kotlin_replay_decision_variants(kotlin_replay_source)
    except ValueError as error:
        errors.append(str(error))
        kotlin_variants = []
    if kotlin_variants != list(KOTLIN_REPLAY_DECISIONS):
        errors.append(
            f"Kotlin ReplayDecision variants {kotlin_variants} differ from {list(KOTLIN_REPLAY_DECISIONS)}"
        )

    kotlin_replay_test_source = KOTLIN_REPLAY_TEST_PATH.read_text(encoding="utf-8")
    for name in KOTLIN_REPLAY_TEST_NAMES:
        if not contains_kotlin_test(kotlin_replay_test_source, name):
            errors.append(f"Kotlin ReplayWindowVectorTest is missing the {name} pin")
    for name in RUST_REPLAY_TEST_NAMES:
        if not contains_rust_test(rust_replay_source, name):
            errors.append(f"Rust replay.rs is missing the {name} pin")

    rust_media_key_source = RUST_MEDIA_KEY_PATH.read_text(encoding="utf-8")
    missing_rust = rust_sequence_pins(rust_media_key_source)
    for needle in missing_rust:
        errors.append(f"Rust MediaSequence is missing {needle}")
    rust_media_crypto_source = RUST_MEDIA_CRYPTO_PATH.read_text(encoding="utf-8")
    if not contains_rust_test(rust_media_crypto_source, RUST_SEALER_EXHAUSTION_TEST):
        errors.append(
            f"Rust media_crypto.rs is missing the {RUST_SEALER_EXHAUSTION_TEST} pin"
        )

    kotlin_sequence_source = KOTLIN_SEQUENCE_PATH.read_text(encoding="utf-8")
    missing_kotlin = kotlin_sequence_pins(kotlin_sequence_source)
    for needle in missing_kotlin:
        errors.append(f"Kotlin MediaSequence is missing {needle}")
    kotlin_sequence_test_source = KOTLIN_SEQUENCE_TEST_PATH.read_text(encoding="utf-8")
    for name in KOTLIN_SEQUENCE_TEST_NAMES:
        if not contains_kotlin_test(kotlin_sequence_test_source, name):
            errors.append(f"Kotlin MediaSequenceVectorTest is missing the {name} pin")

    for scenario_name, steps in REPLAY_SCENARIOS.items():
        window = ReplayWindow()
        for sequence, expected in steps:
            actual = window.accept_authenticated(sequence)
            if actual != expected:
                errors.append(
                    f"replay scenario {scenario_name}: sequence {sequence} "
                    f"yielded {actual}, expected {expected}"
                )

    for scenario_name, (initial, expected_sequences) in SEQUENCE_SCENARIOS.items():
        sequence_source = MediaSequence(initial)
        for index, expected in enumerate(expected_sequences):
            actual = sequence_source.take_next()
            if actual != expected:
                errors.append(
                    f"sequence scenario {scenario_name}: take {index} yielded "
                    f"{actual}, expected {expected}"
                )

    return errors


def main() -> int:
    errors = validate_sequence_replay()
    for error in errors:
        print(error, file=sys.stderr)
    if errors:
        print(f"sequence replay: {len(errors)} error(s)", file=sys.stderr)
        return 1
    print("sequence replay: ok")
    return 0


class SequenceReplayTests(unittest.TestCase):
    def test_replay_scenarios_match_the_published_contract(self) -> None:
        for scenario_name, steps in REPLAY_SCENARIOS.items():
            with self.subTest(scenario=scenario_name):
                window = ReplayWindow()
                for sequence, expected in steps:
                    self.assertEqual(window.accept_authenticated(sequence), expected)

    def test_replay_rejects_a_gap_shift_beyond_the_window(self) -> None:
        window = ReplayWindow()
        self.assertEqual(window.accept_authenticated(1), "ACCEPTED")
        self.assertEqual(window.accept_authenticated(1 + REPLAY_WINDOW_BITS), "ACCEPTED")
        self.assertEqual(window.accept_authenticated(1), "TOO_OLD")

    def test_replay_bounds_the_bitmap_at_the_window_width(self) -> None:
        window = ReplayWindow()
        window.accept_authenticated(0)
        window.accept_authenticated(REPLAY_WINDOW_BITS)
        self.assertEqual(window.accept_authenticated(0), "TOO_OLD")
        # No bit outside the window can ever be set by the model.
        self.assertLess(window.seen, 1 << REPLAY_WINDOW_BITS)

    def test_sequence_scenarios_match_the_published_contract(self) -> None:
        for scenario_name, (initial, expected_sequences) in SEQUENCE_SCENARIOS.items():
            with self.subTest(scenario=scenario_name):
                source = MediaSequence(initial)
                for index, expected in enumerate(expected_sequences):
                    self.assertEqual(source.take_next(), expected, index)

    def test_sequence_fails_closed_after_u32_max(self) -> None:
        source = MediaSequence(U32_MAX)
        self.assertEqual(source.take_next(), U32_MAX)
        self.assertIsNone(source.take_next())
        self.assertIsNone(source.take_next())

    def test_rust_parser_reads_window_bits(self) -> None:
        source = RUST_REPLAY_PATH.read_text(encoding="utf-8")
        self.assertEqual(rust_replay_window_bits(source), REPLAY_WINDOW_BITS)

    def test_rust_parser_detects_a_tampered_window_bits(self) -> None:
        source = RUST_REPLAY_PATH.read_text(encoding="utf-8")
        tampered = source.replace("const WINDOW_BITS: u32 = 64;", "const WINDOW_BITS: u32 = 65;")
        self.assertEqual(rust_replay_window_bits(tampered), 65)

    def test_kotlin_parser_reads_window_bits(self) -> None:
        source = KOTLIN_REPLAY_PATH.read_text(encoding="utf-8")
        self.assertEqual(kotlin_replay_window_bits(source), REPLAY_WINDOW_BITS)

    def test_kotlin_parser_detects_a_tampered_window_bits(self) -> None:
        source = KOTLIN_REPLAY_PATH.read_text(encoding="utf-8")
        tampered = source.replace("val REPLAY_WINDOW_BITS = 64u", "val REPLAY_WINDOW_BITS = 32u")
        self.assertEqual(kotlin_replay_window_bits(tampered), 32)

    def test_rust_decision_parser_reads_all_variants(self) -> None:
        source = RUST_REPLAY_PATH.read_text(encoding="utf-8")
        self.assertEqual(rust_replay_decision_variants(source), list(RUST_REPLAY_DECISIONS))

    def test_kotlin_decision_parser_reads_all_variants(self) -> None:
        source = KOTLIN_REPLAY_PATH.read_text(encoding="utf-8")
        self.assertEqual(
            kotlin_replay_decision_variants(source), list(KOTLIN_REPLAY_DECISIONS)
        )

    def test_rust_media_sequence_pins_accept_the_mirror_source(self) -> None:
        source = RUST_MEDIA_KEY_PATH.read_text(encoding="utf-8")
        self.assertEqual(rust_sequence_pins(source), [])

    def test_rust_media_sequence_pins_detect_a_missing_exhaustion_guard(self) -> None:
        source = RUST_MEDIA_KEY_PATH.read_text(encoding="utf-8")
        tampered = source.replace("checked_add(1)", "wrapping_add(1)")
        self.assertIn("checked_add(1)", rust_sequence_pins(tampered))

    def test_kotlin_media_sequence_pins_accept_the_mirror_source(self) -> None:
        source = KOTLIN_SEQUENCE_PATH.read_text(encoding="utf-8")
        self.assertEqual(kotlin_sequence_pins(source), [])

    def test_kotlin_media_sequence_pins_detect_a_missing_exhaustion_guard(self) -> None:
        source = KOTLIN_SEQUENCE_PATH.read_text(encoding="utf-8")
        tampered = source.replace("UInt.MAX_VALUE", "UInt.MASK")
        self.assertIn("UInt.MAX_VALUE", kotlin_sequence_pins(tampered))

    def test_validation_reports_a_tampered_window_width(self) -> None:
        source = KOTLIN_REPLAY_PATH.read_text(encoding="utf-8")
        original = KOTLIN_REPLAY_PATH.read_text(encoding="utf-8")
        try:
            KOTLIN_REPLAY_PATH.write_text(
                source.replace("val REPLAY_WINDOW_BITS = 64u", "val REPLAY_WINDOW_BITS = 32u"),
                encoding="utf-8",
                newline="\n",
            )
            errors = validate_sequence_replay()
            self.assertTrue(any("window bits 32" in error for error in errors))
        finally:
            KOTLIN_REPLAY_PATH.write_text(original, encoding="utf-8", newline="\n")


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
