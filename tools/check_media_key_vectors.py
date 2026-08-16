#!/usr/bin/env python3
"""Cross-language check of the media key derivation and nonce vectors.

`derive_media_key` and `media_nonce` (crates/sonimesh-protocol/src/media_key.rs)
define the media-plane key and AEAD nonce boundary. The in-crate tests
`matches_published_media_key_derivation_vector`,
`matches_published_media_nonce_vector`, and their reverse-direction
counterparts (`matches_published_reverse_media_key_vector`,
`matches_published_reverse_media_nonce_vector`) pin the golden bytes for both
media directions. This script reimplements the HKDF-SHA256 derivation and the
96-bit nonce layout from the documented labels and constants in pure
standard-library Python, verifies the same published vectors, and parses the
Rust unit-test and Kotlin JVM pins so the three implementations cannot drift
apart (roadmap Phase 0D: cross-language crypto and packet test vectors;
testing doc: published vectors for media key derivation and nonce
construction).
"""

from __future__ import annotations

import hashlib
import hmac
import re
import sys
import unittest
from pathlib import Path

ROOT_SALT = b"sonimesh media root v1"
STREAM_KEY_LABEL = b"sonimesh media stream key v1"
MEDIA_VERSION = 1
ANDROID_TO_DESKTOP_DIRECTION = 0
DESKTOP_TO_ANDROID_DIRECTION = 1

REPO_ROOT = Path(__file__).resolve().parent.parent
RUST_TEST_PATH = REPO_ROOT / "crates" / "sonimesh-protocol" / "src" / "media_key.rs"
KOTLIN_TEST_PATH = (
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
    / "MediaKeyVectorTest.kt"
)


def hkdf_extract(salt: bytes, ikm: bytes) -> bytes:
    """RFC 5869 HKDF-Extract: PRK = HMAC-Hash(salt, IKM)."""
    return hmac.new(salt, ikm, hashlib.sha256).digest()


def hkdf_expand(prk: bytes, info: bytes, length: int) -> bytes:
    """RFC 5869 HKDF-Expand: T(i) = HMAC-Hash(PRK, T(i-1) | info | i)."""
    output = bytearray()
    previous = b""
    counter = 1
    while len(output) < length:
        previous = hmac.new(prk, previous + info + bytes([counter]), hashlib.sha256).digest()
        output.extend(previous)
        counter += 1
    return bytes(output[:length])


def derive_media_key(
    secret: bytes,
    session_id: int,
    stream_id: int,
    key_epoch: int,
    direction: int,
) -> bytes:
    """Matches `derive_media_key`: HKDF over the stream-key label and context."""
    info = (
        STREAM_KEY_LABEL
        + bytes([MEDIA_VERSION])
        + session_id.to_bytes(8, "big")
        + stream_id.to_bytes(4, "big")
        + key_epoch.to_bytes(2, "big")
        + bytes([direction])
    )
    prk = hkdf_extract(ROOT_SALT, secret)
    return hkdf_expand(prk, info, 32)


def media_nonce(stream_id: int, key_epoch: int, direction: int, sequence: int) -> bytes:
    """Matches `media_nonce`: 96-bit nonce with one byte per dimension."""
    return (
        key_epoch.to_bytes(2, "big")
        + bytes([direction, 0])
        + stream_id.to_bytes(4, "big")
        + sequence.to_bytes(4, "big")
    )


def published_vectors() -> dict[str, bytes]:
    """The golden bytes pinned by the Rust media-key tests."""
    return {
        "media_key": bytes.fromhex(
            "0B821644F0598E95080E4959E276FFCD5ECFFF53775BCFC804CC17B23344D546"
        ),
        "media_nonce": bytes.fromhex("000300000000000900000001"),
        "media_key_reverse": bytes.fromhex(
            "3EC355C7CDCE6A0AA0A439FFCD53A3E27A2A6A1CA0DFBC5832223AAEB094852C"
        ),
        "media_nonce_reverse": bytes.fromhex("000301000000000900000001"),
    }


def recomputed_vectors() -> dict[str, bytes]:
    """Independent recomputation of the published vectors."""
    secret = bytes([4]) * 32
    return {
        "media_key": derive_media_key(secret, 7, 9, 3, ANDROID_TO_DESKTOP_DIRECTION),
        "media_nonce": media_nonce(9, 3, ANDROID_TO_DESKTOP_DIRECTION, 1),
        "media_key_reverse": derive_media_key(secret, 7, 9, 3, DESKTOP_TO_ANDROID_DIRECTION),
        "media_nonce_reverse": media_nonce(9, 3, DESKTOP_TO_ANDROID_DIRECTION, 1),
    }


def rust_vector_pins(source: str) -> dict[str, bytes]:
    """Extracts the golden byte arrays pinned by each Rust media-key test."""
    pins: dict[str, bytes] = {}
    for test_name, name in (
        ("matches_published_media_key_derivation_vector", "media_key"),
        ("matches_published_media_nonce_vector", "media_nonce"),
        ("matches_published_reverse_media_key_vector", "media_key_reverse"),
        ("matches_published_reverse_media_nonce_vector", "media_nonce_reverse"),
    ):
        function_start = source.index(f"fn {test_name}(")
        # Anchor at assert_eq! so SessionSecret::new([4; 32]) or similar
        # helper arrays are never mistaken for the golden pin.
        assert_start = source.index("assert_eq!(", function_start)
        bracket_start = source.index("[", assert_start)
        bracket_end = source.index("]", bracket_start)
        body = re.sub(r"//[^\r\n]*", "", source[bracket_start + 1 : bracket_end])
        hex_tokens = re.findall(r"0x([0-9a-fA-F]{2})", body)
        pins[name] = bytes(int(token, 16) for token in hex_tokens)
    return pins


def kotlin_vector_pins(source: str) -> dict[str, bytes]:
    """Extracts the golden byte arrays pinned by the Kotlin media-key test."""
    pins: dict[str, bytes] = {}
    for marker, name in (
        ("val EXPECTED_MEDIA_KEY = hex(", "media_key"),
        ("val EXPECTED_MEDIA_NONCE = hex(", "media_nonce"),
        ("val EXPECTED_REVERSE_MEDIA_KEY = hex(", "media_key_reverse"),
        ("val EXPECTED_REVERSE_MEDIA_NONCE = hex(", "media_nonce_reverse"),
    ):
        match = re.search(re.escape(marker) + r"(.*?)\)", source, re.DOTALL)
        if match is None:
            continue
        pieces = re.findall(r'"([0-9a-fA-F]+)"', match.group(1))
        if not pieces:
            continue
        pins[name] = bytes.fromhex("".join(pieces))
    return pins


def validate_vectors() -> list[str]:
    """Returns deterministic mismatches between recomputation and the pins."""
    recomputed = recomputed_vectors()
    published = published_vectors()
    errors: list[str] = []
    for name, expected in published.items():
        if recomputed[name] != expected:
            errors.append(
                f"media {name} does not match the published vector: "
                f"recomputed {recomputed[name].hex().upper()} "
                f"published {expected.hex().upper()}"
            )
    rust_source = RUST_TEST_PATH.read_text(encoding="utf-8")
    rust_pins = rust_vector_pins(rust_source)
    for name, expected in published.items():
        rust_pin = rust_pins.get(name)
        if rust_pin is None:
            errors.append(f"media {name}: Rust unit-test pin not found")
        elif rust_pin != expected:
            errors.append(
                f"media {name} differs from the Rust pin: "
                f"python {expected.hex().upper()} "
                f"rust {rust_pin.hex().upper()}"
            )
    kotlin_source = KOTLIN_TEST_PATH.read_text(encoding="utf-8")
    kotlin_pins = kotlin_vector_pins(kotlin_source)
    for name, expected in published.items():
        kotlin_pin = kotlin_pins.get(name)
        if kotlin_pin is None:
            errors.append(f"media {name}: Kotlin unit-test pin not found")
        elif kotlin_pin != expected:
            errors.append(
                f"media {name} differs from the Kotlin pin: "
                f"python {expected.hex().upper()} "
                f"kotlin {kotlin_pin.hex().upper()}"
            )
    return errors


def main() -> int:
    errors = validate_vectors()
    for error in errors:
        print(error, file=sys.stderr)
    if errors:
        print(f"media key vectors: {len(errors)} error(s)", file=sys.stderr)
        return 1
    print("media key vectors: ok")
    return 0


class MediaKeyVectorTests(unittest.TestCase):
    def test_hkdf_matches_rfc_5869_sha256_case_1(self) -> None:
        ikm = bytes([0x0B]) * 22
        salt = bytes(range(0x00, 0x0D))
        info = bytes(range(0xF0, 0xFA))
        length = 42
        expected = bytes.fromhex(
            "3CB25F25FAACD57A90434F64D0362F2A2D2D0A90CF1A5A4C5DB02D56ECC4C5BF"
            "34007208D5B887185865"
        )
        self.assertEqual(hkdf_expand(hkdf_extract(salt, ikm), info, length), expected)

    def test_published_vectors_recompute(self) -> None:
        recomputed = recomputed_vectors()
        published = published_vectors()
        for name, expected in published.items():
            self.assertEqual(recomputed[name], expected, name)

    def test_nonce_layout_uses_one_byte_per_dimension(self) -> None:
        nonce = media_nonce(9, 3, ANDROID_TO_DESKTOP_DIRECTION, 1)
        self.assertEqual(nonce[0:2], (3).to_bytes(2, "big"))
        self.assertEqual(nonce[2], ANDROID_TO_DESKTOP_DIRECTION)
        self.assertEqual(nonce[3], 0, "reserved padding byte")
        self.assertEqual(nonce[4:8], (9).to_bytes(4, "big"))
        self.assertEqual(nonce[8:12], (1).to_bytes(4, "big"))
        self.assertEqual(len(nonce), 12)

    def test_direction_changes_only_the_direction_byte(self) -> None:
        forward = media_nonce(9, 3, ANDROID_TO_DESKTOP_DIRECTION, 1)
        reverse = media_nonce(9, 3, DESKTOP_TO_ANDROID_DIRECTION, 1)
        self.assertEqual(forward[0:2], reverse[0:2])
        self.assertEqual(forward[4:12], reverse[4:12])
        self.assertEqual(reverse[2], DESKTOP_TO_ANDROID_DIRECTION)

    def test_rust_parser_extracts_all_pinned_vectors(self) -> None:
        source = RUST_TEST_PATH.read_text(encoding="utf-8")
        pins = rust_vector_pins(source)
        self.assertEqual(set(pins), set(published_vectors()))
        for name, expected in published_vectors().items():
            self.assertEqual(pins[name], expected, name)

    def test_rust_parser_detects_a_tampered_pin(self) -> None:
        source = RUST_TEST_PATH.read_text(encoding="utf-8")
        tampered = source.replace("0x3E, 0xC3, 0x55, 0xC7", "0x3E, 0xC3, 0x55, 0xC8", 1)
        pins = rust_vector_pins(tampered)
        self.assertNotEqual(pins["media_key_reverse"], published_vectors()["media_key_reverse"])

    def test_kotlin_parser_extracts_all_pinned_vectors(self) -> None:
        source = KOTLIN_TEST_PATH.read_text(encoding="utf-8")
        pins = kotlin_vector_pins(source)
        self.assertEqual(set(pins), set(published_vectors()))
        for name, expected in published_vectors().items():
            self.assertEqual(pins[name], expected, name)

    def test_kotlin_parser_detects_a_tampered_pin(self) -> None:
        source = KOTLIN_TEST_PATH.read_text(encoding="utf-8")
        tampered = source.replace(
            '"0B821644F0598E95080E4959E276FFCD"', '"0B821644F0598E95080E4959E276FFCE"', 1
        )
        pins = kotlin_vector_pins(tampered)
        self.assertNotEqual(pins["media_key"], published_vectors()["media_key"])


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
