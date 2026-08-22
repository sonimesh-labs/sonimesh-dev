#!/usr/bin/env python3
"""Cross-language check of the version-one pairing QR payload vector.

`PairingQrPayload`/`parse_pairing_qr_payload`
(crates/sonimesh-protocol/src/pairing_qr.rs) encode and decode the bounded
first-pairing QR payload layout from ADR 0008. The Rust unit tests
`matches_published_pairing_qr_vector` and
`decode_round_trips_the_published_vector` pin the golden bytes. The Kotlin
`PairingQr` mirror and its `PairingQrVectorTest` pin the same bytes on the JVM.
This script reimplements the documented layout in pure standard-library
Python, re-encodes and decodes the published vector, and re-reads the pins
from docs/protocol/pairing-qr-vectors.md, the Rust source, and the Kotlin
test, so the document, the Rust implementation, the Kotlin mirror, and this
verifier cannot drift apart.
"""

from __future__ import annotations

import re
import struct
import sys
import unittest
from pathlib import Path

CONTROL_PROTOCOL_VERSION = 1
PAYLOAD_VERSION = 1
QR_MAGIC = b"SMQR"
PREFIX_LEN = 67
HOST_MAX_LEN = 255
SPKI_MIN_LEN = 32
SPKI_MAX_LEN = 512
MAX_PAYLOAD_LEN = PREFIX_LEN + HOST_MAX_LEN + SPKI_MAX_LEN

HOST_LEN_OFFSET = 6
PORT_OFFSET = 7
SPKI_LEN_OFFSET = 9
PAIRING_ID_OFFSET = 11
PAIRING_SECRET_OFFSET = 27
EXPIRY_OFFSET = 59
HOST_OFFSET = PREFIX_LEN

VECTOR_HOST = "desktop-01.lab.local"
VECTOR_PORT = 47610
VECTOR_EXPIRY_MS = 2_000_000
VECTOR_PAIRING_ID = bytes(range(16))
VECTOR_PAIRING_SECRET = bytes(range(0x10, 0x30))
VECTOR_SPKI = bytes(range(0x40, 0x70))
VECTOR_ENCODED = (
    QR_MAGIC
    + bytes([PAYLOAD_VERSION, CONTROL_PROTOCOL_VERSION, len(VECTOR_HOST)])
    + struct.pack(">H", VECTOR_PORT)
    + struct.pack(">H", len(VECTOR_SPKI))
    + VECTOR_PAIRING_ID
    + VECTOR_PAIRING_SECRET
    + struct.pack(">Q", VECTOR_EXPIRY_MS)
    + VECTOR_HOST.encode("ascii")
    + VECTOR_SPKI
)
VECTOR_ENCODED_HEX = VECTOR_ENCODED.hex()

REPO_ROOT = Path(__file__).resolve().parent.parent
DOC_PATH = REPO_ROOT / "docs" / "protocol" / "pairing-qr-vectors.md"
RUST_SOURCE_PATH = REPO_ROOT / "crates" / "sonimesh-protocol" / "src" / "pairing_qr.rs"
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
    / "PairingQrVectorTest.kt"
)


class QrPayloadError(ValueError):
    """Raised when a payload violates the documented version-one bounds."""


def encode_payload(
    pairing_id: bytes,
    pairing_secret: bytes,
    control_host: str,
    control_port: int,
    desktop_tls_spki: bytes,
    expires_at_ms: int,
    protocol_version: int = CONTROL_PROTOCOL_VERSION,
) -> bytes:
    """Reimplements the documented encoder layout and validation order."""
    if protocol_version != CONTROL_PROTOCOL_VERSION:
        raise QrPayloadError("unsupported pairing protocol version")
    if not 1 <= len(pairing_id) <= 16 or len(pairing_id) != 16:
        raise QrPayloadError("pairing id length is invalid")
    if len(pairing_secret) != 32:
        raise QrPayloadError("pairing secret length is invalid")
    if not control_host or len(control_host.encode()) > HOST_MAX_LEN:
        raise QrPayloadError("pairing QR host length is invalid")
    host_bytes = control_host.encode("utf-8")
    if not all(0x20 <= byte <= 0x7E for byte in host_bytes):
        raise QrPayloadError("pairing QR host is not printable text")
    if not 1 <= control_port <= 0xFFFF:
        raise QrPayloadError("pairing QR control port is zero")
    if not SPKI_MIN_LEN <= len(desktop_tls_spki) <= SPKI_MAX_LEN:
        raise QrPayloadError("pairing QR desktop SPKI length is invalid")
    if expires_at_ms == 0:
        raise QrPayloadError("pairing QR expiry is zero")
    return (
        QR_MAGIC
        + bytes([PAYLOAD_VERSION, protocol_version, len(host_bytes)])
        + struct.pack(">H", control_port)
        + struct.pack(">H", len(desktop_tls_spki))
        + pairing_id
        + pairing_secret
        + struct.pack(">Q", expires_at_ms)
        + host_bytes
        + desktop_tls_spki
    )


def parse_payload(payload: bytes) -> dict[str, object]:
    """Reimplements the documented decoder layout and validation order."""
    if len(payload) > MAX_PAYLOAD_LEN:
        raise QrPayloadError("pairing QR payload exceeds the hard limit")
    if len(payload) < PREFIX_LEN:
        raise QrPayloadError("pairing QR payload is truncated")
    if payload[0:4] != QR_MAGIC:
        raise QrPayloadError("pairing QR payload has an invalid magic value")
    if payload[4] != PAYLOAD_VERSION:
        raise QrPayloadError("unsupported pairing QR payload version")
    protocol_version = payload[5]
    if protocol_version != CONTROL_PROTOCOL_VERSION:
        raise QrPayloadError("unsupported pairing protocol version")
    host_len = payload[HOST_LEN_OFFSET]
    if host_len == 0:
        raise QrPayloadError("pairing QR host length is invalid")
    control_port = struct.unpack(">H", payload[PORT_OFFSET : PORT_OFFSET + 2])[0]
    if control_port == 0:
        raise QrPayloadError("pairing QR control port is zero")
    spki_len = struct.unpack(">H", payload[SPKI_LEN_OFFSET : SPKI_LEN_OFFSET + 2])[0]
    if not SPKI_MIN_LEN <= spki_len <= SPKI_MAX_LEN:
        raise QrPayloadError("pairing QR desktop SPKI length is invalid")
    expires_at_ms = struct.unpack(">Q", payload[EXPIRY_OFFSET : EXPIRY_OFFSET + 8])[0]
    if expires_at_ms == 0:
        raise QrPayloadError("pairing QR expiry is zero")
    if HOST_OFFSET + host_len + spki_len != len(payload):
        raise QrPayloadError("pairing QR payload length does not match its lengths")
    host_bytes = payload[HOST_OFFSET : HOST_OFFSET + host_len]
    if not all(0x20 <= byte <= 0x7E for byte in host_bytes):
        raise QrPayloadError("pairing QR host is not printable text")
    try:
        control_host = host_bytes.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise QrPayloadError("pairing QR host is not UTF-8") from exc
    if len(control_host) != host_len:
        raise QrPayloadError("pairing QR host is not printable text")
    return {
        "protocol_version": protocol_version,
        "pairing_id": payload[PAIRING_ID_OFFSET : PAIRING_ID_OFFSET + 16],
        "pairing_secret": payload[
            PAIRING_SECRET_OFFSET : PAIRING_SECRET_OFFSET + 32
        ],
        "control_host": control_host,
        "control_port": control_port,
        "desktop_tls_spki": payload[HOST_OFFSET + host_len : HOST_OFFSET + host_len + spki_len],
        "expires_at_ms": expires_at_ms,
    }


def validate_vector() -> list[str]:
    """Re-encodes and decodes the published vector from documented inputs."""
    errors: list[str] = []
    encoded = encode_payload(
        VECTOR_PAIRING_ID,
        VECTOR_PAIRING_SECRET,
        VECTOR_HOST,
        VECTOR_PORT,
        VECTOR_SPKI,
        VECTOR_EXPIRY_MS,
    )
    if encoded != VECTOR_ENCODED:
        errors.append(
            "pairing QR re-encoding differs from the published vector: "
            f"recomputed {encoded.hex()} published {VECTOR_ENCODED_HEX}"
        )

    view = parse_payload(VECTOR_ENCODED)
    expected = {
        "protocol_version": CONTROL_PROTOCOL_VERSION,
        "pairing_id": VECTOR_PAIRING_ID,
        "pairing_secret": VECTOR_PAIRING_SECRET,
        "control_host": VECTOR_HOST,
        "control_port": VECTOR_PORT,
        "desktop_tls_spki": VECTOR_SPKI,
        "expires_at_ms": VECTOR_EXPIRY_MS,
    }
    for name, value in expected.items():
        if view[name] != value:
            errors.append(f"pairing QR decoded {name} differs from the documented input")

    for mutation_offset, description in (
        (0, "magic"),
        (4, "payload version"),
        (5, "protocol version"),
        (6, "host length"),
    ):
        mutated = bytearray(VECTOR_ENCODED)
        mutated[mutation_offset] ^= 0x01
        try:
            parse_payload(bytes(mutated))
        except QrPayloadError:
            continue
        errors.append(f"pairing QR decoder accepted a mutated {description}")
    return errors


def doc_vector(source: str) -> str:
    """Extracts the one-line expected output from the vector document."""
    matches = re.findall(
        r"^([0-9a-f]{200,})$",
        source,
        re.MULTILINE,
    )
    if not matches:
        raise ValueError("pairing QR doc is missing the expected output line")
    if len({match for match in matches}) != 1:
        raise ValueError("pairing QR doc repeats inconsistent expected outputs")
    return matches[0]


def rust_vector(source: str) -> str:
    """Extracts the pinned encoded vector constant from the Rust source."""
    match = re.search(r'VECTOR_ENCODED_HEX:\s*&str\s*=\s*"([0-9a-f]+)"', source)
    if match is None:
        raise ValueError("Rust pairing QR source is missing the encoded vector pin")
    return match.group(1)


def kotlin_vector(source: str) -> str:
    """Extracts the pinned encoded vector literal from the Kotlin test.

    The Kotlin pin may be written as adjacent string literals joined by
    ``+``; concatenate every quoted segment of the assigned expression.
    """
    match = re.search(
        r"ENCODED_VECTOR_HEX\s*=\s*((?:\"[0-9a-f]*\"\s*(?:\+\s*)?)+)",
        source,
    )
    if match is None:
        raise ValueError("Kotlin pairing QR test is missing the encoded vector pin")
    return "".join(re.findall(r'"([0-9a-f]*)"', match.group(1)))


def cross_source_errors(
    doc_source: str | None = None,
    rust_source: str | None = None,
    kotlin_source: str | None = None,
) -> list[str]:
    """Checks the doc, Rust, and Kotlin pins against the Python pin."""
    errors: list[str] = []
    try:
        if doc_source is None:
            doc_source = DOC_PATH.read_text(encoding="utf-8")
        if rust_source is None:
            rust_source = RUST_SOURCE_PATH.read_text(encoding="utf-8")
        if kotlin_source is None:
            kotlin_source = KOTLIN_TEST_PATH.read_text(encoding="utf-8")
    except OSError as exc:
        return [f"cannot read pairing QR vector sources: {exc}"]

    try:
        doc = doc_vector(doc_source)
        rust = rust_vector(rust_source)
        kotlin = kotlin_vector(kotlin_source)
    except ValueError as exc:
        return [str(exc)]

    for label, value in (
        ("doc", doc),
        ("Rust pin", rust),
        ("Kotlin pin", kotlin),
    ):
        if value != VECTOR_ENCODED_HEX:
            errors.append(
                f"pairing QR {label} differs from the Python pin: "
                f"{label} {value} pin {VECTOR_ENCODED_HEX}"
            )
    return errors


def main() -> int:
    errors = validate_vector()
    errors.extend(cross_source_errors())
    for error in errors:
        print(error, file=sys.stderr)
    if errors:
        print(f"pairing QR vectors: {len(errors)} error(s)", file=sys.stderr)
        return 1
    print("pairing QR vectors: ok")
    return 0


class PairingQrVectorTests(unittest.TestCase):
    def test_vector_recomputes_and_round_trips(self) -> None:
        self.assertEqual(validate_vector(), [])

    def test_rejects_oversized_truncated_and_foreign_payloads(self) -> None:
        with self.assertRaises(QrPayloadError):
            parse_payload(bytes(MAX_PAYLOAD_LEN + 1))
        with self.assertRaises(QrPayloadError):
            parse_payload(bytes(PREFIX_LEN - 1))
        with self.assertRaises(QrPayloadError):
            parse_payload(b"X" + VECTOR_ENCODED[1:])

    def test_rejects_zero_port_zero_expiry_and_bad_lengths(self) -> None:
        mutated = bytearray(VECTOR_ENCODED)
        mutated[PORT_OFFSET] = 0
        mutated[PORT_OFFSET + 1] = 0
        with self.assertRaises(QrPayloadError):
            parse_payload(bytes(mutated))

        mutated = bytearray(VECTOR_ENCODED)
        mutated[EXPIRY_OFFSET : EXPIRY_OFFSET + 8] = bytes(8)
        with self.assertRaises(QrPayloadError):
            parse_payload(bytes(mutated))

        mutated = bytearray(VECTOR_ENCODED)
        mutated[SPKI_LEN_OFFSET] = 0
        mutated[SPKI_LEN_OFFSET + 1] = SPKI_MIN_LEN - 1
        with self.assertRaises(QrPayloadError):
            parse_payload(bytes(mutated))

        shortened = VECTOR_ENCODED[:-1]
        with self.assertRaises(QrPayloadError):
            parse_payload(shortened)

    def test_rejects_non_printable_host(self) -> None:
        mutated = bytearray(VECTOR_ENCODED)
        mutated[HOST_OFFSET] = 0x7F
        with self.assertRaises(QrPayloadError):
            parse_payload(bytes(mutated))
        with self.assertRaises(QrPayloadError):
            encode_payload(
                VECTOR_PAIRING_ID,
                VECTOR_PAIRING_SECRET,
                "desktop\x7f.lab.local",
                VECTOR_PORT,
                VECTOR_SPKI,
                VECTOR_EXPIRY_MS,
            )

    def test_doc_parser_round_trips_the_expected_output(self) -> None:
        source = DOC_PATH.read_text(encoding="utf-8")
        self.assertEqual(doc_vector(source), VECTOR_ENCODED_HEX)

    def test_doc_parser_detects_a_tampered_value(self) -> None:
        source = DOC_PATH.read_text(encoding="utf-8")
        tampered = source.replace("534d51520101", "534d51520102", 1)
        self.assertNotEqual(doc_vector(tampered), VECTOR_ENCODED_HEX)

    def test_rust_parser_extracts_the_pin(self) -> None:
        source = RUST_SOURCE_PATH.read_text(encoding="utf-8")
        self.assertEqual(rust_vector(source), VECTOR_ENCODED_HEX)

    def test_rust_parser_detects_a_tampered_pin(self) -> None:
        source = RUST_SOURCE_PATH.read_text(encoding="utf-8")
        tampered = source.replace("534d51520101", "534d51520102", 1)
        self.assertNotEqual(rust_vector(tampered), VECTOR_ENCODED_HEX)

    def test_kotlin_parser_extracts_the_pin(self) -> None:
        source = KOTLIN_TEST_PATH.read_text(encoding="utf-8")
        self.assertEqual(kotlin_vector(source), VECTOR_ENCODED_HEX)

    def test_kotlin_parser_detects_a_tampered_pin(self) -> None:
        source = KOTLIN_TEST_PATH.read_text(encoding="utf-8")
        tampered = source.replace("534d51520101", "534d51520102", 1)
        self.assertNotEqual(kotlin_vector(tampered), VECTOR_ENCODED_HEX)
        # A drift inside any concatenated segment must also change the pin.
        drifted = source.replace("00000000001e8480", "00000000001e8481", 1)
        self.assertNotEqual(kotlin_vector(drifted), VECTOR_ENCODED_HEX)

    def test_cross_source_detects_kotlin_drift(self) -> None:
        doc_source = DOC_PATH.read_text(encoding="utf-8")
        rust_source = RUST_SOURCE_PATH.read_text(encoding="utf-8")
        kotlin_source = KOTLIN_TEST_PATH.read_text(encoding="utf-8")
        self.assertEqual(
            cross_source_errors(doc_source, rust_source, kotlin_source), []
        )
        tampered_kotlin = kotlin_source.replace("534d51520101", "534d51520102", 1)
        errors = cross_source_errors(doc_source, rust_source, tampered_kotlin)
        self.assertTrue(any("Kotlin pin" in error for error in errors))

    def test_cross_source_detects_doc_and_rust_drift(self) -> None:
        doc_source = DOC_PATH.read_text(encoding="utf-8")
        rust_source = RUST_SOURCE_PATH.read_text(encoding="utf-8")
        self.assertEqual(cross_source_errors(doc_source, rust_source), [])
        tampered_doc = doc_source.replace("534d51520101", "534d51520102", 1)
        errors = cross_source_errors(tampered_doc, rust_source)
        self.assertTrue(any("doc" in error for error in errors))
        tampered_rust = rust_source.replace("534d51520101", "534d51520102", 1)
        errors = cross_source_errors(doc_source, tampered_rust)
        self.assertTrue(any("Rust pin" in error for error in errors))


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
