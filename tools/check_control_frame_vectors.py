#!/usr/bin/env python3
"""Cross-language check of the version-one control-frame packet vectors.

The Rust protocol crate pins canonical wire bytes for the five version-one
control payloads in `crates/sonimesh-protocol/tests/control_frame.rs`
(`matches_published_cross_language_*_vector`). This script reimplements the
Protobuf wire encoding from `proto/control.proto` in pure standard-library
Python and verifies the same published frame bytes, so schema renumbering or an
encoder change is caught by an independent implementation before it becomes a
release boundary (roadmap Phase 0D: cross-language crypto and packet test
vectors). It also re-reads the frames pinned in
docs/protocol/control-frame-vectors.md, `crates/sonimesh-protocol/tests/
control_frame.rs`, and the Kotlin `ControlFrameVectorTest` and verifies that
all four sources agree, so the doc, the Rust pins, the Kotlin pin, and this
verifier cannot drift apart.
"""

from __future__ import annotations

import re
import sys
import unittest
from pathlib import Path

CONTROL_PROTOCOL_VERSION = 1

# Enum values from proto/control.proto.
CAPABILITY_FRAME_DURATION_20_MS = 2
STREAM_STATE_STREAMING = 2
ERROR_CODE_RETRY_LATER = 5

REPO_ROOT = Path(__file__).resolve().parent.parent
DOC_PATH = REPO_ROOT / "docs" / "protocol" / "control-frame-vectors.md"
RUST_TEST_PATH = (
    REPO_ROOT / "crates" / "sonimesh-protocol" / "tests" / "control_frame.rs"
)
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
    / "protocol"
    / "ControlFrameVectorTest.kt"
)


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


FRAME_NAMES = (
    "hello",
    "start_stream",
    "stop_stream",
    "stream_status",
    "control_error",
)

DOC_HEADING_TO_FRAME = {
    "## Expected frame": "hello",
    "## Epoch-bearing StartStream": "start_stream",
    "## Epoch-bearing StopStream": "stop_stream",
    "## StreamStatus": "stream_status",
    "## ControlError": "control_error",
}

RUST_TEST_TO_FRAME = {
    "matches_published_cross_language_control_frame_vector": "hello",
    "matches_published_epoch_bearing_start_vector": "start_stream",
    "matches_published_epoch_bearing_stop_vector": "stop_stream",
    "matches_published_stream_status_vector": "stream_status",
    "matches_published_control_error_vector": "control_error",
}

KOTLIN_CONSTANT_TO_FRAME = {
    "EXPECTED_HELLO_FRAME": "hello",
    "EXPECTED_START_FRAME": "start_stream",
    "EXPECTED_STOP_FRAME": "stop_stream",
    "EXPECTED_STATUS_FRAME": "stream_status",
    "EXPECTED_ERROR_FRAME": "control_error",
}


def doc_frames(source: str) -> dict[str, bytes]:
    """Extracts the five published frame datagrams from the vector doc."""
    frames: dict[str, bytes] = {}
    for match in re.finditer(r"^## ([^\r\n]+)$", source, re.MULTILINE):
        frame = DOC_HEADING_TO_FRAME.get(match.group(0))
        if frame is None:
            continue
        fence = re.search(
            r"^```text[^\r\n]*\r?\n(.*?)^```[ \t]*$",
            source[match.end() :],
            re.MULTILINE | re.DOTALL,
        )
        if fence is None:
            raise ValueError(f"control frame doc {match.group(1)!r} has no datagram")
        frames[frame] = bytes.fromhex(re.sub(r"\s+", "", fence.group(1)))
    return frames


def rust_test_frames(source: str) -> dict[str, bytes]:
    """Extracts each Rust vector test's pinned frame byte array by name."""
    frames: dict[str, bytes] = {}
    for test_name, frame in RUST_TEST_TO_FRAME.items():
        function_start = source.index(f"fn {test_name}(")
        encode_start = source.index("encode_control_frame(", function_start)
        bracket_start = source.index("[", encode_start)
        bracket_end = source.index("]", bracket_start)
        body = re.sub(r"//[^\r\n]*", "", source[bracket_start + 1 : bracket_end])
        hex_tokens = re.findall(r"0x([0-9a-fA-F]{2})", body)
        frames[frame] = bytes(int(token, 16) for token in hex_tokens)
    return frames


def kotlin_frames(source: str) -> dict[str, bytes]:
    """Extracts each Kotlin constant's pinned frame hex literal by name."""
    frames: dict[str, bytes] = {}
    for constant, frame in KOTLIN_CONSTANT_TO_FRAME.items():
        match = re.search(
            rf"val {constant} = hex\((.*?)\)", source, re.DOTALL
        )
        if match is None:
            continue
        pieces = re.findall(r'"([0-9a-fA-F]+)"', match.group(1))
        if not pieces:
            continue
        frames[frame] = bytes.fromhex("".join(pieces))
    return frames


def cross_source_errors(
    doc_source: str | None = None,
    rust_source: str | None = None,
    kotlin_source: str | None = None,
) -> list[str]:
    """Checks the doc, Rust, and Kotlin pins against the recomputation.

    Reads the pinned sources when not supplied, so the published doc frames,
    the Rust test frames, the Kotlin `ControlFrameVectorTest` pins, and the
    Python pins can never drift apart without failing CI.
    """
    errors: list[str] = []
    try:
        if doc_source is None:
            doc_source = DOC_PATH.read_text(encoding="utf-8")
        if rust_source is None:
            rust_source = RUST_TEST_PATH.read_text(encoding="utf-8")
        if kotlin_source is None:
            kotlin_source = KOTLIN_TEST_PATH.read_text(encoding="utf-8")
    except OSError as exc:
        return [f"cannot read control frame vector sources: {exc}"]

    published = published_frames()
    try:
        doc = doc_frames(doc_source)
    except ValueError as exc:
        return [str(exc)]
    rust = rust_test_frames(rust_source)
    kotlin = kotlin_frames(kotlin_source)
    if set(doc) != set(FRAME_NAMES):
        errors.append("control frame doc must pin all five published frames")
    if set(rust) != set(FRAME_NAMES):
        errors.append("Rust control frame test must pin all five published frames")
    if set(kotlin) != set(FRAME_NAMES):
        errors.append("Kotlin control frame test must pin all five published frames")

    for frame in FRAME_NAMES:
        expected = published[frame]
        doc_frame = doc.get(frame)
        if doc_frame is not None and doc_frame != expected:
            errors.append(
                f"control {frame} doc frame differs from the Python pin: "
                f"doc {doc_frame.hex().upper()} "
                f"pin {expected.hex().upper()}"
            )
        rust_frame = rust.get(frame)
        if rust_frame is not None and rust_frame != expected:
            errors.append(
                f"control {frame} Rust frame differs from the Python pin: "
                f"rust {rust_frame.hex().upper()} "
                f"pin {expected.hex().upper()}"
            )
        kotlin_frame = kotlin.get(frame)
        if kotlin_frame is not None and kotlin_frame != expected:
            errors.append(
                f"control {frame} Kotlin frame differs from the Python pin: "
                f"kotlin {kotlin_frame.hex().upper()} "
                f"pin {expected.hex().upper()}"
            )
        recomputed = recomputed_frames()[frame]
        if recomputed != expected:
            errors.append(
                f"control {frame} frame does not match the independent recomputation"
            )
    return errors


def main() -> int:
    errors = validate_vectors()
    errors.extend(cross_source_errors())
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

    def test_doc_parser_round_trips_all_published_frames(self) -> None:
        source = DOC_PATH.read_text(encoding="utf-8")
        frames = doc_frames(source)
        self.assertEqual(set(frames), set(published_frames()))
        for name, expected in published_frames().items():
            self.assertEqual(frames[name], expected, name)

    def test_doc_parser_detects_a_tampered_frame(self) -> None:
        source = DOC_PATH.read_text(encoding="utf-8")
        tampered = source.replace("4c61622073656e646572", "4c61622073656e646573", 1)
        frames = doc_frames(tampered)
        self.assertNotEqual(frames["hello"], published_frames()["hello"])

    def test_rust_parser_extracts_all_published_frames(self) -> None:
        source = RUST_TEST_PATH.read_text(encoding="utf-8")
        frames = rust_test_frames(source)
        self.assertEqual(set(frames), set(published_frames()))
        for name, expected in published_frames().items():
            self.assertEqual(frames[name], expected, name)

    def test_rust_parser_detects_a_tampered_pin(self) -> None:
        source = RUST_TEST_PATH.read_text(encoding="utf-8")
        tampered = source.replace("0x4C, 0x61, 0x62, 0x20", "0x4C, 0x61, 0x62, 0x21", 1)
        frames = rust_test_frames(tampered)
        self.assertNotEqual(frames["hello"], published_frames()["hello"])

    def test_kotlin_parser_extracts_all_published_frames(self) -> None:
        source = KOTLIN_TEST_PATH.read_text(encoding="utf-8")
        frames = kotlin_frames(source)
        self.assertEqual(set(frames), set(published_frames()))
        for name, expected in published_frames().items():
            self.assertEqual(frames[name], expected, name)

    def test_kotlin_parser_detects_a_tampered_pin(self) -> None:
        source = KOTLIN_TEST_PATH.read_text(encoding="utf-8")
        tampered = source.replace("4c61622073656e646572", "4c61622073656e646573", 1)
        frames = kotlin_frames(tampered)
        self.assertNotEqual(frames["hello"], published_frames()["hello"])

    def test_cross_source_detects_doc_rust_and_kotlin_drift(self) -> None:
        doc_source = DOC_PATH.read_text(encoding="utf-8")
        rust_source = RUST_TEST_PATH.read_text(encoding="utf-8")
        kotlin_source = KOTLIN_TEST_PATH.read_text(encoding="utf-8")
        self.assertEqual(
            cross_source_errors(doc_source, rust_source, kotlin_source), []
        )
        tampered_doc = doc_source.replace(
            "4c61622073656e646572", "4c61622073656e646573", 1
        )
        errors = cross_source_errors(tampered_doc, rust_source, kotlin_source)
        self.assertTrue(
            any("hello" in error and "Python pin" in error for error in errors)
        )
        tampered_rust = rust_source.replace(
            "0x4C, 0x61, 0x62, 0x20", "0x4C, 0x61, 0x62, 0x21", 1
        )
        errors = cross_source_errors(doc_source, tampered_rust, kotlin_source)
        self.assertTrue(
            any("hello" in error and "Rust frame" in error for error in errors)
        )
        tampered_kotlin = kotlin_source.replace(
            "4c61622073656e646572", "4c61622073656e646573", 1
        )
        errors = cross_source_errors(doc_source, rust_source, tampered_kotlin)
        self.assertTrue(
            any("hello" in error and "Kotlin frame" in error for error in errors)
        )


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
