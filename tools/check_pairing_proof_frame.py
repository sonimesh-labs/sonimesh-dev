#!/usr/bin/env python3
"""Cross-language check of the version-one pairing proof-frame vector.

`PairingProofFrame`/`parse_pairing_proof_frame`
(crates/sonimesh-protocol/src/pairing_proof.rs) encode and decode the bounded,
fixed-length frame that carries one 32-byte transcript proof across the pinned
TLS channel during ADR 0008 first pairing. The Rust unit tests
`matches_published_pairing_proof_frame_vector` and
`decode_round_trips_the_published_vector` pin the golden bytes. This script
reimplements the documented layout in pure standard-library Python, re-encodes
and decodes both role vectors from the published TLS-binding proofs, rejects
mutated header fields, and re-reads the pins from
docs/protocol/pairing-proof-frame-vectors.md, the Rust source, and the Kotlin
test, so the document, the Rust implementation, the Kotlin mirror, and this
verifier cannot drift apart.
"""

from __future__ import annotations

import re
import sys
import unittest
from pathlib import Path

CONTROL_PROTOCOL_VERSION = 1
FRAME_VERSION = 1
FRAME_MAGIC = b"SMPE"
FRAME_LEN = 55

ROLE_ANDROID_CLIENT = 0
ROLE_DESKTOP_SERVER = 1

VECTOR_PAIRING_ID = bytes(range(16))
VECTOR_CLIENT_PROOF = bytes.fromhex(
    "c274dfe72135ae558034ca1e8a27a4eab7aac9b87a0cd93c17bf17126e27f4f2"
)
VECTOR_SERVER_PROOF = bytes.fromhex(
    "f347ab32006e8acb220f91b0e826bbd58257c55773d7284f8794217f896b4601"
)


class FrameError(ValueError):
    """Raised when a frame violates the documented version-one layout."""


def encode_frame(role: int, pairing_id: bytes, proof: bytes) -> bytes:
    """Reimplements the documented encoder layout and validation order."""
    if len(pairing_id) != 16:
        raise FrameError("pairing id length is invalid")
    if len(proof) != 32:
        raise FrameError("proof length is invalid")
    if role not in (ROLE_ANDROID_CLIENT, ROLE_DESKTOP_SERVER):
        raise FrameError("invalid pairing role byte")
    return (
        FRAME_MAGIC
        + bytes([FRAME_VERSION, CONTROL_PROTOCOL_VERSION, role])
        + pairing_id
        + proof
    )


def parse_frame(frame: bytes) -> dict[str, object]:
    """Reimplements the documented decoder layout and validation order."""
    if len(frame) != FRAME_LEN:
        raise FrameError("pairing proof frame is truncated or oversized")
    if frame[0:4] != FRAME_MAGIC:
        raise FrameError("pairing proof frame has an invalid magic value")
    if frame[4] != FRAME_VERSION:
        raise FrameError("unsupported pairing proof frame version")
    protocol_version = frame[5]
    if protocol_version != CONTROL_PROTOCOL_VERSION:
        raise FrameError("unsupported pairing protocol version")
    role = frame[6]
    if role not in (ROLE_ANDROID_CLIENT, ROLE_DESKTOP_SERVER):
        raise FrameError("invalid pairing role byte")
    return {
        "protocol_version": protocol_version,
        "role": role,
        "pairing_id": frame[7:23],
        "proof": frame[23:55],
    }


VECTOR_CLIENT_FRAME = encode_frame(
    ROLE_ANDROID_CLIENT, VECTOR_PAIRING_ID, VECTOR_CLIENT_PROOF
)
VECTOR_SERVER_FRAME = encode_frame(
    ROLE_DESKTOP_SERVER, VECTOR_PAIRING_ID, VECTOR_SERVER_PROOF
)
VECTOR_CLIENT_FRAME_HEX = VECTOR_CLIENT_FRAME.hex()
VECTOR_SERVER_FRAME_HEX = VECTOR_SERVER_FRAME.hex()


def validate_vectors() -> list[str]:
    """Re-encodes and decodes both role vectors from documented inputs."""
    errors: list[str] = []
    for label, encoded, role, proof in (
        ("client", VECTOR_CLIENT_FRAME, ROLE_ANDROID_CLIENT, VECTOR_CLIENT_PROOF),
        ("server", VECTOR_SERVER_FRAME, ROLE_DESKTOP_SERVER, VECTOR_SERVER_PROOF),
    ):
        view = parse_frame(encoded)
        expected = {
            "protocol_version": CONTROL_PROTOCOL_VERSION,
            "role": role,
            "pairing_id": VECTOR_PAIRING_ID,
            "proof": proof,
        }
        for name, value in expected.items():
            if view[name] != value:
                errors.append(
                    f"pairing proof {label} frame decoded {name} differs "
                    "from the documented input"
                )

    for mutation_offset, expected_error in (
        (0, "magic"),
        (4, "version"),
        (5, "protocol version"),
    ):
        mutated = bytearray(VECTOR_CLIENT_FRAME)
        mutated[mutation_offset] ^= 0x01
        try:
            parse_frame(bytes(mutated))
        except FrameError as error:
            if expected_error.split()[0] in str(error):
                continue
            errors.append(
                f"pairing proof frame decoder rejected a mutated "
                f"{expected_error} with the wrong message: {error}"
            )
            continue
        errors.append(f"pairing proof frame decoder accepted a mutated {expected_error}")

    # Role bytes 2..255 are outside the two defined roles; note that flipping
    # bit zero maps one defined role onto the other and must stay accepted.
    for bad_role in (2, 3, 0xFF):
        mutated = bytearray(VECTOR_CLIENT_FRAME)
        mutated[6] = bad_role
        try:
            parse_frame(bytes(mutated))
        except FrameError:
            continue
        errors.append("pairing proof frame decoder accepted an unknown role byte")

    short = VECTOR_CLIENT_FRAME[: FRAME_LEN - 1]
    try:
        parse_frame(short)
    except FrameError:
        pass
    else:
        errors.append("pairing proof frame decoder accepted a truncated frame")
    trailing = VECTOR_CLIENT_FRAME + b"\x00"
    try:
        parse_frame(trailing)
    except FrameError:
        pass
    else:
        errors.append("pairing proof frame decoder accepted an oversized frame")
    return errors


REPO_ROOT = Path(__file__).resolve().parent.parent
DOC_PATH = REPO_ROOT / "docs" / "protocol" / "pairing-proof-frame-vectors.md"
RUST_SOURCE_PATH = REPO_ROOT / "crates" / "sonimesh-protocol" / "src" / "pairing_proof.rs"
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
    / "PairingProofFrameVectorTest.kt"
)


def doc_vectors(source: str) -> list[str]:
    """Extracts both pinned expected-output hex lines from the vector doc."""
    matches = re.findall(r"^([0-9a-f]{100,})$", source, re.MULTILINE)
    if len(matches) < 2:
        raise ValueError("pairing proof frame doc is missing the expected outputs")
    return matches


def rust_vector(source: str) -> str:
    """Extracts the pinned client-frame vector constant from the Rust source."""
    match = re.search(r'VECTOR_ENCODED_HEX:\s*&str\s*=\s*"([0-9a-f]+)"', source)
    if match is None:
        raise ValueError("Rust pairing proof source is missing the encoded vector pin")
    return match.group(1)


def kotlin_vector(source: str) -> str:
    """Extracts the pinned client-frame vector literal from the Kotlin test.

    The Kotlin pin may be written as adjacent string literals joined by
    ``+``; concatenate every quoted segment of the assigned expression.
    """
    match = re.search(
        r"ENCODED_VECTOR_HEX\s*=\s*((?:\"[0-9a-f]*\"\s*(?:\+\s*)?)+)",
        source,
    )
    if match is None:
        raise ValueError("Kotlin pairing proof test is missing the encoded vector pin")
    return "".join(re.findall(r'"([0-9a-f]*)"', match.group(1)))


def cross_source_errors(
    doc_source: str | None = None,
    rust_source: str | None = None,
    kotlin_source: str | None = None,
) -> list[str]:
    """Checks the doc, Rust, and Kotlin pins against the Python pins."""
    errors: list[str] = []
    try:
        if doc_source is None:
            doc_source = DOC_PATH.read_text(encoding="utf-8")
        if rust_source is None:
            rust_source = RUST_SOURCE_PATH.read_text(encoding="utf-8")
        if kotlin_source is None:
            kotlin_source = KOTLIN_TEST_PATH.read_text(encoding="utf-8")
    except OSError as exc:
        return [f"cannot read pairing proof frame sources: {exc}"]

    try:
        doc = doc_vectors(doc_source)
        rust = rust_vector(rust_source)
        kotlin = kotlin_vector(kotlin_source)
    except ValueError as exc:
        return [str(exc)]

    if VECTOR_CLIENT_FRAME_HEX not in doc:
        errors.append(
            f"pairing proof frame doc is missing the client vector: "
            f"pin {VECTOR_CLIENT_FRAME_HEX}"
        )
    if VECTOR_SERVER_FRAME_HEX not in doc:
        errors.append(
            f"pairing proof frame doc is missing the server vector: "
            f"pin {VECTOR_SERVER_FRAME_HEX}"
        )
    if rust != VECTOR_CLIENT_FRAME_HEX:
        errors.append(
            "pairing proof frame Rust pin differs from the Python pin: "
            f"rust {rust} pin {VECTOR_CLIENT_FRAME_HEX}"
        )
    if kotlin != VECTOR_CLIENT_FRAME_HEX:
        errors.append(
            "pairing proof frame Kotlin pin differs from the Python pin: "
            f"kotlin {kotlin} pin {VECTOR_CLIENT_FRAME_HEX}"
        )

    # A role-swapped mutation of the pinned client vector must never appear as
    # a pinned output; the decoder rejects it before any proof is used.
    swapped = bytearray(VECTOR_CLIENT_FRAME)
    swapped[6] = ROLE_DESKTOP_SERVER
    if bytes(swapped).hex() in doc:
        errors.append("pairing proof frame doc pins a role-swapped frame as valid")
    return errors


def main() -> int:
    errors = validate_vectors()
    errors.extend(cross_source_errors())
    for error in errors:
        print(error, file=sys.stderr)
    if errors:
        print(f"pairing proof frame vectors: {len(errors)} error(s)", file=sys.stderr)
        return 1
    print("pairing proof frame vectors: ok")
    return 0


class PairingProofFrameTests(unittest.TestCase):
    def test_layout_constants_match_the_rust_module(self) -> None:
        self.assertEqual(FRAME_MAGIC, b"SMPE")
        self.assertEqual(FRAME_VERSION, 1)
        self.assertEqual(len(FRAME_MAGIC) + 3 + 16 + 32, FRAME_LEN)

    def test_both_role_vectors_recompute_from_published_proofs(self) -> None:
        expected_client_hex = (
            "534d5045010100000102030405060708090a0b0c0d0e0f"
            "c274dfe72135ae558034ca1e8a27a4eab7aac9b87a0cd93c17bf17126e27f4f2"
        )
        self.assertEqual(VECTOR_CLIENT_FRAME_HEX, expected_client_hex)
        self.assertNotEqual(VECTOR_CLIENT_FRAME, VECTOR_SERVER_FRAME)

    def test_decoder_rejects_mutated_headers(self) -> None:
        self.assertEqual(validate_vectors(), [])

    def test_doc_parser_extracts_both_pinned_outputs(self) -> None:
        source = DOC_PATH.read_text(encoding="utf-8")
        vectors = doc_vectors(source)
        self.assertIn(VECTOR_CLIENT_FRAME_HEX, vectors)
        self.assertIn(VECTOR_SERVER_FRAME_HEX, vectors)

    def test_rust_parser_extracts_the_client_pin(self) -> None:
        source = RUST_SOURCE_PATH.read_text(encoding="utf-8")
        self.assertEqual(rust_vector(source), VECTOR_CLIENT_FRAME_HEX)

    def test_kotlin_parser_extracts_the_client_pin(self) -> None:
        source = KOTLIN_TEST_PATH.read_text(encoding="utf-8")
        self.assertEqual(kotlin_vector(source), VECTOR_CLIENT_FRAME_HEX)

    def test_cross_source_detects_kotlin_drift(self) -> None:
        kotlin_source = KOTLIN_TEST_PATH.read_text(encoding="utf-8")
        self.assertEqual(cross_source_errors(kotlin_source=kotlin_source), [])
        # The proof bytes appear in both the field literal and the pinned
        # constant; tamper every occurrence so the pin itself must shift.
        tampered_kotlin = kotlin_source.replace(
            "c274dfe72135ae55", "c274dfe72135ae54"
        )
        self.assertNotIn("c274dfe72135ae558034", tampered_kotlin)
        errors = cross_source_errors(kotlin_source=tampered_kotlin)
        self.assertTrue(any("Kotlin pin" in error for error in errors))

    def test_cross_source_detects_doc_and_rust_drift(self) -> None:
        doc_source = DOC_PATH.read_text(encoding="utf-8")
        rust_source = RUST_SOURCE_PATH.read_text(encoding="utf-8")
        self.assertEqual(cross_source_errors(doc_source, rust_source), [])
        tampered_doc = doc_source.replace("c274dfe7", "c274dfe6", 1)
        errors = cross_source_errors(tampered_doc, rust_source)
        self.assertTrue(any("client vector" in error for error in errors))
        tampered_rust = rust_source.replace(
            "c274dfe72135ae55", "c274dfe72135ae54", 1
        )
        errors = cross_source_errors(doc_source, tampered_rust)
        self.assertTrue(any("Rust pin" in error for error in errors))


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
