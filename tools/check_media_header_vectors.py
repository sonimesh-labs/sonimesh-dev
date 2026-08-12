#!/usr/bin/env python3
"""Cross-language check of the version-one media header wire vector.

`MediaHeader::encode` (crates/sonimesh-protocol/src/media_header.rs) produces
the fixed 36-byte header used as AEAD associated data. The Rust test
`matches_published_media_header_vector` pins the canonical bytes. This script
reconstructs the same big-endian layout from the documented constants and
fields in pure standard-library Python and verifies the published vector, so a
header reordering or constant change is caught by an independent
implementation before it becomes a release boundary (roadmap Phase 0D:
cross-language crypto and packet test vectors).
"""

from __future__ import annotations

import sys
import unittest

MEDIA_MAGIC = 0x534E_4D53
MEDIA_VERSION = 1
MEDIA_HEADER_LEN = 36
MAX_MEDIA_DATAGRAM_LEN = 1_200
MAX_ENCRYPTED_PAYLOAD_LEN = MAX_MEDIA_DATAGRAM_LEN - MEDIA_HEADER_LEN


def encode_header(
    flags: int,
    session_id: int,
    stream_id: int,
    sequence: int,
    sample_timestamp: int,
    key_epoch: int,
    payload_len: int,
) -> bytes:
    """Matches `MediaHeader::encode`: fixed big-endian version-one layout."""
    if not 0 <= payload_len <= MAX_ENCRYPTED_PAYLOAD_LEN:
        raise ValueError("payload length exceeds the encrypted bound")
    output = bytearray()
    output.extend(MEDIA_MAGIC.to_bytes(4, "big"))
    output.append(MEDIA_VERSION)
    output.append(flags)
    output.extend(MEDIA_HEADER_LEN.to_bytes(2, "big"))
    output.extend(session_id.to_bytes(8, "big"))
    output.extend(stream_id.to_bytes(4, "big"))
    output.extend(sequence.to_bytes(4, "big"))
    output.extend(sample_timestamp.to_bytes(8, "big"))
    output.extend(payload_len.to_bytes(2, "big"))
    output.extend(key_epoch.to_bytes(2, "big"))
    return bytes(output)


def published_header() -> bytes:
    """The golden bytes pinned by the Rust media-header test."""
    return bytes.fromhex(
        "534E4D5301000024010203040506070800000009000000070000000000000B4000402345"
    )


def recomputed_header() -> bytes:
    """Independent recomputation of the published header."""
    return encode_header(
        flags=0,
        session_id=0x0102_0304_0506_0708,
        stream_id=9,
        sequence=7,
        sample_timestamp=2_880,
        key_epoch=0x2345,
        payload_len=64,
    )


def validate_vectors() -> list[str]:
    """Returns deterministic mismatches between recomputation and the pin."""
    recomputed = recomputed_header()
    published = published_header()
    if recomputed != published:
        return [
            "media header does not match the published vector: "
            f"recomputed {recomputed.hex().upper()} published {published.hex().upper()}"
        ]
    return []


def main() -> int:
    errors = validate_vectors()
    for error in errors:
        print(error, file=sys.stderr)
    if errors:
        print(f"media header vectors: {len(errors)} error(s)", file=sys.stderr)
        return 1
    print("media header vectors: ok")
    return 0


class MediaHeaderVectorTests(unittest.TestCase):
    def test_published_vector_recomputes(self) -> None:
        self.assertEqual(recomputed_header(), published_header())
        self.assertEqual(len(published_header()), MEDIA_HEADER_LEN)

    def test_layout_matches_documented_constants(self) -> None:
        encoded = recomputed_header()
        self.assertEqual(encoded[0:4], MEDIA_MAGIC.to_bytes(4, "big"))
        self.assertEqual(encoded[4], MEDIA_VERSION)
        self.assertEqual(encoded[5], 0, "flags")
        self.assertEqual(int.from_bytes(encoded[6:8], "big"), MEDIA_HEADER_LEN)
        self.assertEqual(int.from_bytes(encoded[8:16], "big"), 0x0102_0304_0506_0708)
        self.assertEqual(int.from_bytes(encoded[16:20], "big"), 9)
        self.assertEqual(int.from_bytes(encoded[20:24], "big"), 7)
        self.assertEqual(int.from_bytes(encoded[24:32], "big"), 2_880)
        self.assertEqual(int.from_bytes(encoded[32:34], "big"), 64)
        self.assertEqual(int.from_bytes(encoded[34:36], "big"), 0x2345)

    def test_rejects_payloads_beyond_the_encrypted_bound(self) -> None:
        with self.assertRaises(ValueError):
            encode_header(0, 1, 1, 1, 0, 0, MAX_ENCRYPTED_PAYLOAD_LEN + 1)
        encode_header(0, 1, 1, 1, 0, 0, MAX_ENCRYPTED_PAYLOAD_LEN)


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
