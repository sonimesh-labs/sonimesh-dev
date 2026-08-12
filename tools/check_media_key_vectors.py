#!/usr/bin/env python3
"""Cross-language check of the media key derivation and nonce vectors.

`derive_media_key` and `media_nonce` (crates/sonimesh-protocol/src/media_key.rs)
define the media-plane key and AEAD nonce boundary. The in-crate tests
`matches_published_media_key_derivation_vector` and
`matches_published_media_nonce_vector` pin the golden bytes. This script
reimplements the HKDF-SHA256 derivation and the 96-bit nonce layout from the
documented labels and constants in pure standard-library Python and verifies
the same published vectors, so a label, salt, or byte-order change is caught by
an independent implementation (roadmap Phase 0D: cross-language crypto and
packet test vectors; testing doc: published vectors for media key derivation
and nonce construction).
"""

from __future__ import annotations

import hashlib
import hmac
import sys
import unittest

ROOT_SALT = b"sonimesh media root v1"
STREAM_KEY_LABEL = b"sonimesh media stream key v1"
MEDIA_VERSION = 1
ANDROID_TO_DESKTOP_DIRECTION = 0
DESKTOP_TO_ANDROID_DIRECTION = 1


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
    }


def recomputed_vectors() -> dict[str, bytes]:
    """Independent recomputation of the published vectors."""
    secret = bytes([4]) * 32
    return {
        "media_key": derive_media_key(secret, 7, 9, 3, ANDROID_TO_DESKTOP_DIRECTION),
        "media_nonce": media_nonce(9, 3, ANDROID_TO_DESKTOP_DIRECTION, 1),
    }


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


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
