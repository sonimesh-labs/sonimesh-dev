#!/usr/bin/env python3
"""Cross-language check of the fixed media-plane and control-frame limits.

The version-one protocol publishes hard numeric bounds: the fixed 36-byte media
header, a 128-byte extended header ceiling, a 1,200-byte complete datagram
limit, the derived 1,164-byte encrypted payload and 1,148-byte plaintext
limits, the 16-byte Poly1305 tag, a 4-byte frame prefix, and the 64 KiB control frame
bound. The Rust
sources (crates/sonimesh-protocol/src/media_header.rs, media_crypto.rs,
control.rs), the Android host-JVM mirrors (MediaHeader.kt, MediaCrypto.kt,
ControlFrame.kt), and the `docs/mvp-baseline.md` capacities table all pin the
same contract. This script re-reads every pin with a tiny expression evaluator,
recomputes the derived bounds, and reports any drift so a renumbering or bound
change on either side fails the job (roadmap Phase 0D: cross-language crypto
and packet test vectors; testing doc: packet header size limits).
"""

from __future__ import annotations

import re
import sys
import unittest
from pathlib import Path

# Published numeric contract (values must equal the resolved Rust/Kotlin pins
# and the documented capacities table).
MEDIA_HEADER_LEN = 36
MAX_HEADER_LEN = 128
MAX_MEDIA_DATAGRAM_LEN = 1_200
MAX_ENCRYPTED_PAYLOAD_LEN = 1_164
POLY1305_TAG_BYTES = 16
MAX_MEDIA_PLAINTEXT_LEN = 1_148
MAX_CONTROL_FRAME_BYTES = 64 * 1024
FRAME_PREFIX_BYTES = 4
MEDIA_MAGIC = 0x534E_4D53
MEDIA_VERSION = 1

REPO_ROOT = Path(__file__).resolve().parent.parent
RUST_MEDIA_HEADER_PATH = (
    REPO_ROOT / "crates" / "sonimesh-protocol" / "src" / "media_header.rs"
)
RUST_MEDIA_CRYPTO_PATH = (
    REPO_ROOT / "crates" / "sonimesh-protocol" / "src" / "media_crypto.rs"
)
RUST_CONTROL_PATH = REPO_ROOT / "crates" / "sonimesh-protocol" / "src" / "control.rs"
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
)
KOTLIN_MEDIA_HEADER_PATH = KOTLIN_DIR / "protocol" / "MediaHeader.kt"
KOTLIN_MEDIA_CRYPTO_PATH = KOTLIN_DIR / "security" / "MediaCrypto.kt"
KOTLIN_CONTROL_PATH = KOTLIN_DIR / "protocol" / "ControlFrame.kt"
MVP_BASELINE_PATH = REPO_ROOT / "docs" / "mvp-baseline.md"

# Every contract constant and the file that declares it on each side.
CONTRACTS: dict[str, int] = {
    "MEDIA_HEADER_LEN": MEDIA_HEADER_LEN,
    "MAX_HEADER_LEN": MAX_HEADER_LEN,
    "MAX_MEDIA_DATAGRAM_LEN": MAX_MEDIA_DATAGRAM_LEN,
    "MAX_ENCRYPTED_PAYLOAD_LEN": MAX_ENCRYPTED_PAYLOAD_LEN,
    "POLY1305_TAG_BYTES": POLY1305_TAG_BYTES,
    "MAX_MEDIA_PLAINTEXT_LEN": MAX_MEDIA_PLAINTEXT_LEN,
    "MAX_CONTROL_FRAME_BYTES": MAX_CONTROL_FRAME_BYTES,
    "FRAME_PREFIX_BYTES": FRAME_PREFIX_BYTES,
    "MEDIA_MAGIC": MEDIA_MAGIC,
    "MEDIA_VERSION": MEDIA_VERSION,
}

# Constants declared in each file; crypto bounds reference the header's
# encrypted-payload bound, so they are resolved with a cross-file seed.
MEDIA_HEADER_NAMES = (
    "MEDIA_HEADER_LEN",
    "MAX_HEADER_LEN",
    "MAX_MEDIA_DATAGRAM_LEN",
    "MAX_ENCRYPTED_PAYLOAD_LEN",
    "MEDIA_MAGIC",
    "MEDIA_VERSION",
)
MEDIA_CRYPTO_NAMES = ("POLY1305_TAG_BYTES", "MAX_MEDIA_PLAINTEXT_LEN")
CONTROL_NAMES = ("FRAME_PREFIX_BYTES", "MAX_CONTROL_FRAME_BYTES")

# (documented boundary, contract constant)
DOC_BOUNDARIES: tuple[tuple[str, str], ...] = (
    ("Media fixed header", "MEDIA_HEADER_LEN"),
    ("Media extended header", "MAX_HEADER_LEN"),
    ("Complete media datagram", "MAX_MEDIA_DATAGRAM_LEN"),
    ("Control Protobuf frame", "MAX_CONTROL_FRAME_BYTES"),
    ("Control frame prefix", "FRAME_PREFIX_BYTES"),
    ("Protected media plaintext", "MAX_MEDIA_PLAINTEXT_LEN"),
    ("Encrypted payload with fixed header", "MAX_ENCRYPTED_PAYLOAD_LEN"),
)

RUST_CONST_PATTERN = re.compile(
    r"(?:pub\s+)?const\s+([A-Z_][A-Z0-9_]*)\s*:\s*[A-Za-z0-9_]+\s*=\s*(.*?);"
)
KOTLIN_CONST_PATTERN = re.compile(
    r"const val\s+([A-Z_][A-Z0-9_]*)(?::\s*[A-Za-z0-9_]+)?\s*=\s*(.*)$",
    re.MULTILINE,
)
DOC_CAPACITY_PATTERN = re.compile(
    r"^\|\s*([A-Za-z][A-Za-z ]+?)\s*\|\s*([0-9][0-9,]*)\s*(bytes|KiB|packets)\s*\|",
    re.MULTILINE,
)

_TOKEN_PATTERN = re.compile(
    r"\s*(?:(?P<num>0[xX][0-9a-fA-F][0-9a-fA-F_]*|\d[\d_]*)|"
    r"(?P<id>[A-Za-z_][A-Za-z0-9_]*)|(?P<op>[+*()-]))"
)


class ExpressionError(ValueError):
    """The constant expression references an unresolved name or is malformed."""


class ExpressionParser:
    """Tiny safe evaluator for the const expressions used by the pins.

    Handles decimal and hexadecimal literals (with underscores), references to
    other pinned constants, and `+`, `-`, `*` with parentheses. No builtins or
    attribute access are reachable.
    """

    def __init__(self, source: str, values: dict[str, int]) -> None:
        self.tokens = self._tokenize(source)
        self.index = 0
        self.values = values

    @staticmethod
    def _tokenize(source: str) -> list[tuple[str, str]]:
        tokens: list[tuple[str, str]] = []
        position = 0
        while position < len(source):
            match = _TOKEN_PATTERN.match(source, position)
            if match is None:
                raise ExpressionError(f"unexpected token at {source[position:]!r}")
            position = match.end()
            if match.lastgroup == "op":
                tokens.append(("op", match.group("op")))
            elif match.lastgroup == "num":
                tokens.append(("num", match.group("num")))
            else:
                tokens.append(("id", match.group("id")))
        tokens.append(("end", ""))
        return tokens

    def parse(self) -> int:
        value = self._additive()
        kind, _ = self.tokens[self.index]
        if kind != "end":
            raise ExpressionError("trailing tokens after expression")
        return value

    def _additive(self) -> int:
        value = self._multiplicative()
        while True:
            kind, token = self.tokens[self.index]
            if kind == "op" and token in ("+", "-"):
                self.index += 1
                right = self._multiplicative()
                value = value + right if token == "+" else value - right
            else:
                return value

    def _multiplicative(self) -> int:
        value = self._primary()
        while True:
            kind, token = self.tokens[self.index]
            if kind == "op" and token == "*":
                self.index += 1
                value *= self._primary()
            else:
                return value

    def _primary(self) -> int:
        kind, token = self.tokens[self.index]
        if kind == "num":
            self.index += 1
            digits = token.replace("_", "")
            return int(digits, 0)
        if kind == "id":
            self.index += 1
            if token not in self.values:
                raise ExpressionError(f"unresolved constant {token}")
            return self.values[token]
        if kind == "op" and token == "(":
            self.index += 1
            value = self._additive()
            if self.tokens[self.index] != ("op", ")"):
                raise ExpressionError("missing closing parenthesis")
            self.index += 1
            return value
        raise ExpressionError(f"unexpected {kind} token {token!r}")


def evaluate_expression(expression: str, values: dict[str, int]) -> int:
    """Evaluates one const expression against already-resolved values."""
    return ExpressionParser(expression, values).parse()


def resolve_constants(
    source: str, names: list[str], seed: dict[str, int] | None = None
) -> dict[str, int]:
    """Resolves the requested const declarations, following same-file refs.

    Expressions may reference other constants declared in the same source;
    resolution iterates over every declaration until a fixed point so
    dependencies are never mistaken for missing or unresolved pins. `seed`
    supplies constants resolved from another file (the crypto bounds reference
    the header's encrypted-payload limit).
    """
    raw: dict[str, str] = {}
    for name, expression in RUST_CONST_PATTERN.findall(source):
        raw[name] = expression
    for name, expression in KOTLIN_CONST_PATTERN.findall(source):
        raw[name] = expression

    values: dict[str, int] = dict(seed or {})
    for _ in range(len(raw) + 1):
        for name, expression in raw.items():
            if name in values:
                continue
            try:
                values[name] = evaluate_expression(expression, values)
            except ExpressionError:
                continue

    missing = [name for name in names if name not in raw]
    if missing:
        raise ValueError(f"constant {', '.join(missing)} is not declared in the source")
    unresolved = [name for name in names if name not in values]
    if unresolved:
        raise ValueError(f"could not resolve constants: {', '.join(sorted(unresolved))}")
    return {name: values[name] for name in names}


def parse_doc_capacities(source: str) -> dict[str, int]:
    """Reads the numeric capacity of each documented boundary row."""
    capacities: dict[str, int] = {}
    for boundary, raw_value, unit in DOC_CAPACITY_PATTERN.findall(source):
        value = int(raw_value.replace(",", ""))
        if unit == "KiB":
            value *= 1024
        capacities[boundary] = value
    return capacities


def validate_media_limits() -> list[str]:
    """Returns every drift error across the Rust, Kotlin, and doc pins."""
    errors: list[str] = []

    rust_values: dict[str, int] = {}
    try:
        rust_values.update(
            resolve_constants(
                RUST_MEDIA_HEADER_PATH.read_text(encoding="utf-8"),
                list(MEDIA_HEADER_NAMES),
            )
        )
    except ValueError as error:
        errors.append(f"Rust media_header.rs: {error}")
    try:
        rust_values.update(
            resolve_constants(
                RUST_MEDIA_CRYPTO_PATH.read_text(encoding="utf-8"),
                list(MEDIA_CRYPTO_NAMES),
                seed=rust_values,
            )
        )
    except ValueError as error:
        errors.append(f"Rust media_crypto.rs: {error}")
    try:
        rust_values.update(
            resolve_constants(
                RUST_CONTROL_PATH.read_text(encoding="utf-8"),
                list(CONTROL_NAMES),
            )
        )
    except ValueError as error:
        errors.append(f"Rust control.rs: {error}")

    kotlin_values: dict[str, int] = {}
    try:
        kotlin_values.update(
            resolve_constants(
                KOTLIN_MEDIA_HEADER_PATH.read_text(encoding="utf-8"),
                list(MEDIA_HEADER_NAMES),
            )
        )
    except ValueError as error:
        errors.append(f"Kotlin MediaHeader.kt: {error}")
    try:
        kotlin_values.update(
            resolve_constants(
                KOTLIN_MEDIA_CRYPTO_PATH.read_text(encoding="utf-8"),
                list(MEDIA_CRYPTO_NAMES),
                seed=kotlin_values,
            )
        )
    except ValueError as error:
        errors.append(f"Kotlin MediaCrypto.kt: {error}")
    try:
        kotlin_values.update(
            resolve_constants(
                KOTLIN_CONTROL_PATH.read_text(encoding="utf-8"),
                list(CONTROL_NAMES),
            )
        )
    except ValueError as error:
        errors.append(f"Kotlin ControlFrame.kt: {error}")

    for name, published in CONTRACTS.items():
        rust = rust_values.get(name)
        if rust is not None and rust != published:
            errors.append(
                f"Rust {name} is {rust}, expected the published {published}"
            )
        kotlin = kotlin_values.get(name)
        if kotlin is not None and kotlin != published:
            errors.append(
                f"Kotlin {name} is {kotlin}, expected the published {published}"
            )
        if rust is not None and kotlin is not None and rust != kotlin:
            errors.append(f"{name} differs between Rust ({rust}) and Kotlin ({kotlin})")

    # Derived-bound invariants hold in every implementation.
    if MAX_ENCRYPTED_PAYLOAD_LEN != MAX_MEDIA_DATAGRAM_LEN - MEDIA_HEADER_LEN:
        errors.append("encrypted payload is not the datagram minus the fixed header")
    if MAX_MEDIA_PLAINTEXT_LEN != MAX_ENCRYPTED_PAYLOAD_LEN - POLY1305_TAG_BYTES:
        errors.append("plaintext limit is not the encrypted payload minus the tag")
    if MAX_CONTROL_FRAME_BYTES != 64 * 1024:
        errors.append("control frame bound is not 64 KiB")
    if FRAME_PREFIX_BYTES != 4:
        errors.append("control frame prefix is not 4 bytes")

    capacities = parse_doc_capacities(MVP_BASELINE_PATH.read_text(encoding="utf-8"))
    for boundary, name in DOC_BOUNDARIES:
        documented = capacities.get(boundary)
        if documented is None:
            errors.append(f"mvp-baseline capacities table is missing {boundary}")
            continue
        expected = CONTRACTS[name]
        if documented != expected:
            errors.append(
                f"mvp-baseline documents {boundary} as {documented}, "
                f"expected {expected}"
            )

    return errors


def main() -> int:
    errors = validate_media_limits()
    for error in errors:
        print(error, file=sys.stderr)
    if errors:
        print(f"media limits: {len(errors)} error(s)", file=sys.stderr)
        return 1
    print("media limits: ok")
    return 0


class MediaLimitsTests(unittest.TestCase):
    def test_expression_parser_handles_hex_and_underscores(self) -> None:
        self.assertEqual(evaluate_expression("0x534E_4D53", {}), 0x534E4D53)
        self.assertEqual(evaluate_expression("1_200", {}), 1_200)

    def test_expression_parser_handles_references_and_operators(self) -> None:
        values = {"A": 1_200, "B": 36}
        self.assertEqual(evaluate_expression("A - B", values), 1_164)
        self.assertEqual(evaluate_expression("64 * 1024", {}), 65_536)
        self.assertEqual(evaluate_expression("(A - B) * 2", values), 2_328)

    def test_expression_parser_rejects_unresolved_names(self) -> None:
        with self.assertRaises(ExpressionError):
            evaluate_expression("MISSING + 1", {})

    def test_rust_resolver_reads_all_pins(self) -> None:
        source = RUST_MEDIA_HEADER_PATH.read_text(encoding="utf-8")
        resolved = resolve_constants(
            source,
            ["MEDIA_HEADER_LEN", "MAX_HEADER_LEN", "MAX_MEDIA_DATAGRAM_LEN",
             "MAX_ENCRYPTED_PAYLOAD_LEN", "MEDIA_MAGIC", "MEDIA_VERSION"],
        )
        self.assertEqual(resolved["MEDIA_HEADER_LEN"], 36)
        self.assertEqual(resolved["MAX_HEADER_LEN"], 128)
        self.assertEqual(resolved["MAX_MEDIA_DATAGRAM_LEN"], 1_200)
        self.assertEqual(resolved["MAX_ENCRYPTED_PAYLOAD_LEN"], 1_164)
        self.assertEqual(resolved["MEDIA_MAGIC"], 0x534E4D53)
        self.assertEqual(resolved["MEDIA_VERSION"], 1)

    def test_rust_resolver_detects_a_tampered_constant(self) -> None:
        source = RUST_MEDIA_HEADER_PATH.read_text(encoding="utf-8")
        tampered = source.replace("pub const MEDIA_HEADER_LEN: usize = 36;", "pub const MEDIA_HEADER_LEN: usize = 40;")
        resolved = resolve_constants(tampered, ["MEDIA_HEADER_LEN"])
        self.assertEqual(resolved["MEDIA_HEADER_LEN"], 40)

    def test_kotlin_resolver_reads_all_pins(self) -> None:
        source = KOTLIN_MEDIA_HEADER_PATH.read_text(encoding="utf-8")
        resolved = resolve_constants(
            source,
            ["MEDIA_HEADER_LEN", "MAX_HEADER_LEN", "MAX_MEDIA_DATAGRAM_LEN",
             "MAX_ENCRYPTED_PAYLOAD_LEN", "MEDIA_MAGIC", "MEDIA_VERSION"],
        )
        self.assertEqual(resolved["MEDIA_HEADER_LEN"], 36)
        self.assertEqual(resolved["MAX_HEADER_LEN"], 128)
        self.assertEqual(resolved["MAX_MEDIA_DATAGRAM_LEN"], 1_200)
        self.assertEqual(resolved["MAX_ENCRYPTED_PAYLOAD_LEN"], 1_164)
        self.assertEqual(resolved["MEDIA_MAGIC"], 0x534E4D53)
        self.assertEqual(resolved["MEDIA_VERSION"], 1)

    def test_kotlin_resolver_detects_a_missing_constant(self) -> None:
        source = KOTLIN_MEDIA_HEADER_PATH.read_text(encoding="utf-8")
        tampered = source.replace("const val MAX_HEADER_LEN = 128\n", "")
        with self.assertRaises(ValueError):
            resolve_constants(tampered, ["MAX_HEADER_LEN"])

    def test_crypto_bounds_resolve_in_rust_and_kotlin(self) -> None:
        header_seed = {
            "MAX_ENCRYPTED_PAYLOAD_LEN": MAX_ENCRYPTED_PAYLOAD_LEN,
        }
        rust = resolve_constants(
            RUST_MEDIA_CRYPTO_PATH.read_text(encoding="utf-8"),
            ["POLY1305_TAG_BYTES", "MAX_MEDIA_PLAINTEXT_LEN"],
            seed=header_seed,
        )
        kotlin = resolve_constants(
            KOTLIN_MEDIA_CRYPTO_PATH.read_text(encoding="utf-8"),
            ["POLY1305_TAG_BYTES", "MAX_MEDIA_PLAINTEXT_LEN"],
            seed=header_seed,
        )
        self.assertEqual(rust["POLY1305_TAG_BYTES"], 16)
        self.assertEqual(rust["MAX_MEDIA_PLAINTEXT_LEN"], 1_148)
        self.assertEqual(kotlin["POLY1305_TAG_BYTES"], 16)
        self.assertEqual(kotlin["MAX_MEDIA_PLAINTEXT_LEN"], 1_148)

    def test_control_frame_bound_resolves_in_rust_and_kotlin(self) -> None:
        rust = resolve_constants(
            RUST_CONTROL_PATH.read_text(encoding="utf-8"),
            ["FRAME_PREFIX_BYTES", "MAX_CONTROL_FRAME_BYTES"],
        )
        kotlin = resolve_constants(
            KOTLIN_CONTROL_PATH.read_text(encoding="utf-8"),
            ["FRAME_PREFIX_BYTES", "MAX_CONTROL_FRAME_BYTES"],
        )
        self.assertEqual(rust["FRAME_PREFIX_BYTES"], 4)
        self.assertEqual(rust["MAX_CONTROL_FRAME_BYTES"], 64 * 1024)
        self.assertEqual(kotlin["FRAME_PREFIX_BYTES"], 4)
        self.assertEqual(kotlin["MAX_CONTROL_FRAME_BYTES"], 64 * 1024)

    def test_doc_parser_reads_all_capacity_rows(self) -> None:
        capacities = parse_doc_capacities(MVP_BASELINE_PATH.read_text(encoding="utf-8"))
        for boundary, name in DOC_BOUNDARIES:
            self.assertEqual(capacities[boundary], CONTRACTS[name], boundary)

    def test_doc_parser_detects_a_tampered_capacity(self) -> None:
        source = MVP_BASELINE_PATH.read_text(encoding="utf-8")
        tampered = source.replace("| Media fixed header | 36 bytes |", "| Media fixed header | 40 bytes |")
        capacities = parse_doc_capacities(tampered)
        self.assertEqual(capacities["Media fixed header"], 40)

    def test_validation_reports_a_tampered_doc_capacity(self) -> None:
        original = MVP_BASELINE_PATH.read_text(encoding="utf-8")
        try:
            MVP_BASELINE_PATH.write_text(
                original.replace("| Media fixed header | 36 bytes |", "| Media fixed header | 40 bytes |"),
                encoding="utf-8",
                newline="\n",
            )
            errors = validate_media_limits()
            self.assertTrue(any("Media fixed header" in error for error in errors))
        finally:
            MVP_BASELINE_PATH.write_text(original, encoding="utf-8", newline="\n")


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
