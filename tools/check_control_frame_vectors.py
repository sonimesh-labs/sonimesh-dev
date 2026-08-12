#!/usr/bin/env python3
"""Cross-language check of the version-one control-frame packet vectors.

The Rust protocol crate pins canonical wire bytes for the five version-one
control payloads in `crates/sonimesh-protocol/tests/control_frame.rs`
(`matches_published_cross_language_*_vector`). This script reimplements the
Protobuf wire encoding from `proto/control.proto` in pure standard-library
Python and verifies the same published frame bytes, so schema renumbering or an
encoder change is caught by an independent implementation before it becomes a
release boundary (roadmap Phase 0D: cross-language crypto and packet test
vectors).
"""

from __future__ import annotations

import sys
import unittest

CONTROL_PROTOCOL_VERSION = 1

# Enum values from proto/control.proto.
CAPABILITY_FRAME_DURATION_20_MS = 2
STREAM_STATE_STREAMING = 2
ERROR_CODE_RETRY_LATER = 5


def encode_varint(value: int) -> bytes:
    """Protobuf base-128 varint; rejects negative values."""
    if value < 0:
        raise ValueError("varint must be non-negative")
    output = bytearray()
    while value > 0x7F:
        output.append((value & 0x7F) | 0x80)
        value >>= 7
    output.append(value)
    return bytes(output)


def tag(field_number: int, wire_type: int) -> bytes:
    return encode_varint((field_number << 3) | wire_type)


def varint_field(field_number: int, value: int) -> bytes:
    return tag(field_number, 0) + encode_varint(value)


def bytes_field(field_number: int, value: bytes) -> bytes:
    return tag(field_number, 2) + encode_varint(len(value)) + value


def string_field(field_number: int, value: str) -> bytes:
    return bytes_field(field_number, value.encode("utf-8"))


def packed_varint_field(field_number: int, values: list[int]) -> bytes:
    """Proto3 packed repeated scalar: one length-delimited varint payload."""
    payload = b"".join(encode_varint(value) for value in values)
    return bytes_field(field_number, payload)


def encode_hello(supported_versions: list[int], capabilities: list[int], display_name: str) -> bytes:
    output = bytearray()
    output.extend(packed_varint_field(1, supported_versions))
    output.extend(packed_varint_field(2, capabilities))
    output.extend(string_field(3, display_name))
    return bytes(output)


def encode_start_stream(
    media_session_id: int,
    stream_id: int,
    sample_rate_hz: int,
    channels: int,
    frame_duration_ms: int,
    media_key_epoch: int,
) -> bytes:
    output = bytearray()
    output.extend(varint_field(1, media_session_id))
    output.extend(varint_field(2, stream_id))
    output.extend(varint_field(3, sample_rate_hz))
    output.extend(varint_field(4, channels))
    output.extend(varint_field(5, frame_duration_ms))
    output.extend(varint_field(6, media_key_epoch))
    return bytes(output)


def encode_stop_stream(media_session_id: int, stream_id: int, media_key_epoch: int) -> bytes:
    output = bytearray()
    output.extend(varint_field(1, media_session_id))
    output.extend(varint_field(2, stream_id))
    output.extend(varint_field(3, media_key_epoch))
    return bytes(output)


def encode_stream_status(
    media_session_id: int, stream_id: int, state: int, detail: str
) -> bytes:
    output = bytearray()
    output.extend(varint_field(1, media_session_id))
    output.extend(varint_field(2, stream_id))
    output.extend(varint_field(3, state))
    output.extend(string_field(4, detail))
    return bytes(output)


def encode_control_error(code: int, retryable: bool, detail: str) -> bytes:
    output = bytearray()
    output.extend(varint_field(1, code))
    output.extend(varint_field(2, 1 if retryable else 0))
    output.extend(string_field(3, detail))
    return bytes(output)


def encode_envelope(request_id: int, payload_field: int, payload: bytes) -> bytes:
    output = bytearray()
    output.extend(varint_field(1, CONTROL_PROTOCOL_VERSION))
    output.extend(varint_field(2, request_id))
    output.extend(bytes_field(payload_field, payload))
    return bytes(output)


def frame(payload: bytes) -> bytes:
    """Big-endian 32-bit length prefix followed by the Protobuf payload."""
    return len(payload).to_bytes(4, "big") + payload


def published_frames() -> dict[str, bytes]:
    """The golden bytes pinned by the Rust control-frame tests."""
    return {
        "hello": bytes.fromhex(
            "000000180801100752120A01011201021A0A4C61622073656E646572"
        ),
        "start_stream": bytes.fromhex(
            "00000015080110085A0F080710091880F7022002281430C546"
        ),
        "stop_stream": bytes.fromhex(
            "0000000D0801100962070807100918C546"
        ),
        "stream_status": bytes.fromhex(
            "000000170801100A6A110807100918022209726563656976696E67"
        ),
        "control_error": bytes.fromhex(
            "000000150801100B720F080510011A0974727920616761696E"
        ),
    }


def recomputed_frames() -> dict[str, bytes]:
    """Independent recomputation of every published control frame."""
    hello = encode_envelope(
        7,
        10,
        encode_hello(
            [CONTROL_PROTOCOL_VERSION], [CAPABILITY_FRAME_DURATION_20_MS], "Lab sender"
        ),
    )
    start_stream = encode_envelope(
        8,
        11,
        encode_start_stream(7, 9, 48_000, 2, 20, 0x2345),
    )
    stop_stream = encode_envelope(9, 12, encode_stop_stream(7, 9, 0x2345))
    stream_status = encode_envelope(
        10,
        13,
        encode_stream_status(7, 9, STREAM_STATE_STREAMING, "receiving"),
    )
    control_error = encode_envelope(
        11,
        14,
        encode_control_error(ERROR_CODE_RETRY_LATER, True, "try again"),
    )
    return {
        "hello": frame(hello),
        "start_stream": frame(start_stream),
        "stop_stream": frame(stop_stream),
        "stream_status": frame(stream_status),
        "control_error": frame(control_error),
    }


def validate_vectors() -> list[str]:
    """Returns deterministic mismatches between recomputation and the pins."""
    recomputed = recomputed_frames()
    published = published_frames()
    errors: list[str] = []
    for name, expected in published.items():
        if recomputed[name] != expected:
            errors.append(
                f"control {name} frame does not match the published vector: "
                f"recomputed {recomputed[name].hex().upper()} "
                f"published {expected.hex().upper()}"
            )
    return errors


def main() -> int:
    errors = validate_vectors()
    for error in errors:
        print(error, file=sys.stderr)
    if errors:
        print(f"control frame vectors: {len(errors)} error(s)", file=sys.stderr)
        return 1
    print("control frame vectors: ok")
    return 0


class ControlFrameVectorTests(unittest.TestCase):
    def test_varint_encoding_matches_protobuf_known_answers(self) -> None:
        self.assertEqual(encode_varint(0), b"\x00")
        self.assertEqual(encode_varint(1), b"\x01")
        self.assertEqual(encode_varint(127), b"\x7f")
        self.assertEqual(encode_varint(128), b"\x80\x01")
        self.assertEqual(encode_varint(300), b"\xac\x02")
        # Protobuf documentation example: field 1 with value 150 -> 08 96 01.
        self.assertEqual(varint_field(1, 150), bytes.fromhex("089601"))
        self.assertEqual(encode_varint(2**32 - 1), bytes.fromhex("ffffffff0f"))
        self.assertEqual(encode_varint(2**64 - 1), bytes.fromhex("ffffffffffffffffff01"))
        with self.assertRaises(ValueError):
            encode_varint(-1)

    def test_all_published_frames_recompute(self) -> None:
        recomputed = recomputed_frames()
        published = published_frames()
        self.assertEqual(set(recomputed), set(published))
        for name, expected in published.items():
            self.assertEqual(recomputed[name], expected, name)

    def test_length_prefix_matches_payload_size(self) -> None:
        for name, value in recomputed_frames().items():
            declared = int.from_bytes(value[:4], "big")
            self.assertEqual(declared, len(value) - 4, name)


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
