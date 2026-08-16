#!/usr/bin/env python3
"""Cross-language check of the published protected media AEAD packet vectors.

The Rust media plane seals packets with ChaCha20-Poly1305 (RFC 8439) using the
HKDF media key and the 96-bit media nonce. All three published vectors are pinned in
crates/sonimesh-protocol/tests/media_crypto.rs
(`matches_published_cross_language_media_vector`,
`matches_published_cross_language_aead_packet_vector`, and
`matches_published_cross_language_reverse_direction_vector`), and vectors 1 and
3 are also pinned by the Kotlin/JVM `MediaCryptoVectorTest`. This script reimplements
ChaCha20, Poly1305, and the AEAD construction plus the media key/nonce/header
boundaries in pure standard-library Python and independently reseals every
published datagram, so an AEAD crate, key, nonce, or header change is caught by
an independent implementation (roadmap Phase 0D: cross-language crypto and
packet test vectors; testing doc: published vectors for AEAD packet
authentication). It also cross-checks the documented datagrams in
docs/protocol/media-crypto-vectors.md, the Rust-pinned datagram arrays in
crates/sonimesh-protocol/tests/media_crypto.rs, and the Kotlin
`MediaCryptoVectorTest` pin against the same recomputation, so the doc, the
Rust pins, the Kotlin pin, and this verifier cannot drift apart.
"""

from __future__ import annotations

import hashlib
import hmac
import re
import struct
import sys
import unittest
from dataclasses import dataclass
from pathlib import Path

ROOT_SALT = b"sonimesh media root v1"
STREAM_KEY_LABEL = b"sonimesh media stream key v1"
MEDIA_VERSION = 1
MEDIA_MAGIC = 0x534E_4D53
MEDIA_HEADER_LEN = 36
ANDROID_TO_DESKTOP_DIRECTION = 0
DESKTOP_TO_ANDROID_DIRECTION = 1
POLY1305_TAG_BYTES = 16

REPO_ROOT = Path(__file__).resolve().parent.parent
DOC_PATH = REPO_ROOT / "docs" / "protocol" / "media-crypto-vectors.md"
RUST_TEST_PATH = (
    REPO_ROOT / "crates" / "sonimesh-protocol" / "tests" / "media_crypto.rs"
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
    / "security"
    / "MediaCryptoVectorTest.kt"
)


def hkdf_extract(salt: bytes, ikm: bytes) -> bytes:
    return hmac.new(salt, ikm, hashlib.sha256).digest()


def hkdf_expand(prk: bytes, info: bytes, length: int) -> bytes:
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
    direction: int = ANDROID_TO_DESKTOP_DIRECTION,
) -> bytes:
    info = (
        STREAM_KEY_LABEL
        + bytes([MEDIA_VERSION])
        + session_id.to_bytes(8, "big")
        + stream_id.to_bytes(4, "big")
        + key_epoch.to_bytes(2, "big")
        + bytes([direction])
    )
    return hkdf_expand(hkdf_extract(ROOT_SALT, secret), info, 32)


def media_nonce(
    stream_id: int,
    key_epoch: int,
    sequence: int,
    direction: int = ANDROID_TO_DESKTOP_DIRECTION,
) -> bytes:
    return (
        key_epoch.to_bytes(2, "big")
        + bytes([direction, 0])
        + stream_id.to_bytes(4, "big")
        + sequence.to_bytes(4, "big")
    )


def encode_media_header(
    flags: int,
    session_id: int,
    stream_id: int,
    sequence: int,
    sample_timestamp: int,
    key_epoch: int,
    payload_len: int,
) -> bytes:
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


def rotl32(value: int, shift: int) -> int:
    return ((value << shift) | (value >> (32 - shift))) & 0xFFFFFFFF


def chacha20_quarter_round(state: list[int], a: int, b: int, c: int, d: int) -> None:
    state[a] = (state[a] + state[b]) & 0xFFFFFFFF
    state[d] = rotl32(state[d] ^ state[a], 16)
    state[c] = (state[c] + state[d]) & 0xFFFFFFFF
    state[b] = rotl32(state[b] ^ state[c], 12)
    state[a] = (state[a] + state[b]) & 0xFFFFFFFF
    state[d] = rotl32(state[d] ^ state[a], 8)
    state[c] = (state[c] + state[d]) & 0xFFFFFFFF
    state[b] = rotl32(state[b] ^ state[c], 7)


def chacha20_block(key: bytes, counter: int, nonce: bytes) -> bytes:
    state = [0x61707865, 0x3320646E, 0x79622D32, 0x6B206574]
    state.extend(struct.unpack("<8I", key))
    state.append(counter & 0xFFFFFFFF)
    state.extend(struct.unpack("<3I", nonce))
    working = list(state)
    for _ in range(10):
        chacha20_quarter_round(working, 0, 4, 8, 12)
        chacha20_quarter_round(working, 1, 5, 9, 13)
        chacha20_quarter_round(working, 2, 6, 10, 14)
        chacha20_quarter_round(working, 3, 7, 11, 15)
        chacha20_quarter_round(working, 0, 5, 10, 15)
        chacha20_quarter_round(working, 1, 6, 11, 12)
        chacha20_quarter_round(working, 2, 7, 8, 13)
        chacha20_quarter_round(working, 3, 4, 9, 14)
    return struct.pack(
        "<16I", *[(working[index] + state[index]) & 0xFFFFFFFF for index in range(16)]
    )


def poly1305(message: bytes, key: bytes) -> bytes:
    r = int.from_bytes(key[:16], "little") & 0x0FFFFFFC0FFFFFFC0FFFFFFC0FFFFFFF
    s = int.from_bytes(key[16:], "little")
    accumulator = 0
    prime = (1 << 130) - 5
    for offset in range(0, len(message), 16):
        block = message[offset : offset + 16]
        number = int.from_bytes(block, "little") + (1 << (8 * len(block)))
        accumulator = ((accumulator + number) * r) % prime
    return ((accumulator + s) % (1 << 128)).to_bytes(16, "little")


def pad16(data: bytes) -> bytes:
    return data + (b"\x00" * ((16 - len(data) % 16) % 16))


def aead_encrypt(key: bytes, nonce: bytes, aad: bytes, plaintext: bytes) -> tuple[bytes, bytes]:
    """RFC 8439 ChaCha20-Poly1305 with a 96-bit nonce and detached tag."""
    poly_key = chacha20_block(key, 0, nonce)[:32]
    keystream = b"".join(
        chacha20_block(key, counter, nonce) for counter in range(1, (len(plaintext) // 64) + 2)
    )
    ciphertext = bytes(p ^ k for p, k in zip(plaintext, keystream))
    mac_data = (
        pad16(aad)
        + pad16(ciphertext)
        + len(aad).to_bytes(8, "little")
        + len(ciphertext).to_bytes(8, "little")
    )
    return ciphertext, poly1305(mac_data, poly_key)


@dataclass(frozen=True)
class PublishedVector:
    """One published sealed media datagram plus the constants that fix it."""

    name: str
    secret: bytes
    session_id: int
    stream_id: int
    key_epoch: int
    flags: int
    sample_timestamp: int
    plaintext: bytes
    datagram: bytes
    direction: int = ANDROID_TO_DESKTOP_DIRECTION

    def recompute(self) -> bytes:
        """Independently reseals this vector from its documented constants."""
        key = derive_media_key(
            self.secret, self.session_id, self.stream_id, self.key_epoch, self.direction
        )
        nonce = media_nonce(self.stream_id, self.key_epoch, 0, self.direction)
        header = encode_media_header(
            flags=self.flags,
            session_id=self.session_id,
            stream_id=self.stream_id,
            sequence=0,
            sample_timestamp=self.sample_timestamp,
            key_epoch=self.key_epoch,
            payload_len=len(self.plaintext) + POLY1305_TAG_BYTES,
        )
        ciphertext, tag = aead_encrypt(key, nonce, header, self.plaintext)
        return header + ciphertext + tag


PUBLISHED_VECTORS: tuple[PublishedVector, ...] = (
    PublishedVector(
        name="vector 1: documented pairing-style constants",
        secret=bytes.fromhex(
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
        ),
        session_id=0x0102030405060708,
        stream_id=0x0A0B0C0D,
        key_epoch=0x0E0F,
        flags=0x02,
        sample_timestamp=960,
        plaintext=b"SoniMesh media vector",
        datagram=bytes.fromhex(
            "534e4d530102002401020304050607080a0b0c0d0000000000000000000003c0"
            "00250e0f7cce62ea960fc27cf5a6ffce2aa59343a5f12997791396ca10bdfc7c"
            "c060c9dfe49fd67131"
        ),
    ),
    PublishedVector(
        name="vector 2: canonical cross-language packet vector",
        secret=bytes([4]) * 32,
        session_id=7,
        stream_id=9,
        key_epoch=3,
        flags=0x2A,
        sample_timestamp=0x1122334455667788,
        plaintext=b"cross-language packet vector",
        datagram=bytes.fromhex(
            "534E4D53012A0024000000000000000700000009000000001122334455667788"
            "002C0003337E8052F30B4E20479A0E4080A74511AA39C2DEABAFA132DBFD6B85"
            "C7C85145360F62F4361EFE335F16149A"
        ),
    ),
    PublishedVector(
        name="vector 3: reverse-direction cross-language packet vector",
        secret=bytes.fromhex(
            "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f"
        ),
        session_id=0x0F0E0D0C0B0A0908,
        stream_id=0x12345678,
        key_epoch=0x002A,
        flags=0x00,
        sample_timestamp=0x8899AABBCCDDEEFF,
        plaintext=b"SoniMesh reverse direction vector",
        direction=DESKTOP_TO_ANDROID_DIRECTION,
        datagram=bytes.fromhex(
            "534E4D53010000240F0E0D0C0B0A090812345678000000008899AABBCCDDEEFF"
            "0031002AA0853360825EFA8AB1529900AA2403A2E82B06C3C8FBB2B80B20539"
            "3DFFF19159430B6A033E699E1613D8689455DC95A99"
        ),
    ),
)


def validate_vectors() -> list[str]:
    """Returns deterministic mismatches between recomputation and the pins."""
    errors: list[str] = []
    for vector in PUBLISHED_VECTORS:
        recomputed = vector.recompute()
        if recomputed != vector.datagram:
            errors.append(
                f"{vector.name} does not match the published datagram: "
                f"recomputed {recomputed.hex().upper()} "
                f"published {vector.datagram.hex().upper()}"
            )
        expected_len = MEDIA_HEADER_LEN + len(vector.plaintext) + POLY1305_TAG_BYTES
        if len(vector.datagram) != expected_len:
            errors.append(
                f"{vector.name} has length {len(vector.datagram)}, expected {expected_len}"
            )
    return errors


def doc_vector_sections(source: str) -> list[tuple[str, str]]:
    """Returns (``"1"``/``"2"``, section) pairs from the vector doc headings."""
    headings = list(
        re.finditer(r"^## Vector (?P<name>[123]):[^\r\n]*$", source, re.MULTILINE)
    )
    sections: list[tuple[str, str]] = []
    for index, heading in enumerate(headings):
        end = headings[index + 1].start() if index + 1 < len(headings) else len(source)
        sections.append((heading.group("name"), source[heading.end() : end]))
    return sections


def doc_table_value(section: str, label: str) -> str:
    """Returns the first backtick-quoted value from one markdown table row."""
    row = re.search(
        rf"^\|\s*{re.escape(label)}\s*\|\s*([^|\r\n]*)\s*\|",
        section,
        re.MULTILINE,
    )
    if row is None:
        raise ValueError(f"media crypto doc is missing table row {label!r}")
    quoted = re.search(r"`([^`]*)`", row.group(1))
    if quoted is None:
        raise ValueError(f"media crypto doc table row {label!r} has no backtick value")
    return quoted.group(1)


def doc_integer(value: str) -> int:
    """Parses a decimal or ``0x``-prefixed integer from a doc table value."""
    value = value.strip()
    return int(value, 16) if value.lower().startswith("0x") else int(value, 10)


def doc_datagram(section: str) -> bytes:
    """Returns the hex datagram from the section's fenced text block."""
    fence = re.search(
        r"^```text[^\r\n]*\r?\n(.*?)^```[ \t]*$", section, re.MULTILINE | re.DOTALL
    )
    if fence is None:
        raise ValueError("media crypto doc is missing a ```text datagram block")
    return bytes.fromhex(re.sub(r"\s+", "", fence.group(1)))


def doc_nonce(section: str) -> bytes:
    """Returns the documented 96-bit nonce hex."""
    match = re.search(r"The 96-bit nonce is `([0-9a-fA-F]+)`\.", section)
    if match is None:
        raise ValueError("media crypto doc is missing a nonce line")
    return bytes.fromhex(match.group(1))


def parse_doc_vector(section: str) -> tuple[PublishedVector, bytes, int, int]:
    """Rebuilds a vector from the doc section.

    Returns the vector, its documented nonce, the documented protocol version,
    and the documented direction value so every doc constant is cross-checked.
    """
    secret = bytes.fromhex(doc_table_value(section, "Control-session secret"))
    if len(secret) != 32:
        raise ValueError("media crypto doc control-session secret is not 32 bytes")
    protocol_version = doc_integer(doc_table_value(section, "Protocol version"))
    direction_value = doc_integer(doc_table_value(section, "Direction"))
    if doc_integer(doc_table_value(section, "Sequence")) != 0:
        raise ValueError("media crypto doc sequence must be 0")
    vector = PublishedVector(
        name="",
        secret=secret,
        session_id=doc_integer(doc_table_value(section, "Session ID")),
        stream_id=doc_integer(doc_table_value(section, "Stream ID")),
        key_epoch=doc_integer(doc_table_value(section, "Key epoch")),
        flags=doc_integer(doc_table_value(section, "Flags")),
        sample_timestamp=doc_integer(doc_table_value(section, "Sample timestamp")),
        plaintext=doc_table_value(section, "Plaintext UTF-8").encode("utf-8"),
        datagram=doc_datagram(section),
        direction=direction_value,
    )
    return vector, doc_nonce(section), protocol_version, direction_value


def rust_test_datagrams(source: str) -> dict[str, bytes]:
    """Extracts each Rust vector test's pinned datagram byte array by number."""
    datagrams: dict[str, bytes] = {}
    for test_name, number in (
        ("matches_published_cross_language_media_vector", "1"),
        ("matches_published_cross_language_aead_packet_vector", "2"),
        ("matches_published_cross_language_reverse_direction_vector", "3"),
    ):
        function_start = source.index(f"fn {test_name}(")
        datagram_start = source.index("protected.datagram,", function_start)
        bracket_start = source.index("[", datagram_start)
        bracket_end = source.index("]", bracket_start)
        body = re.sub(r"//[^\r\n]*", "", source[bracket_start + 1 : bracket_end])
        hex_tokens = re.findall(r"0x([0-9a-fA-F]{2})", body)
        datagrams[number] = bytes(int(token, 16) for token in hex_tokens)
    return datagrams


def kotlin_expected_datagram(source: str) -> bytes | None:
    """Extracts the Kotlin vector test's pinned datagram hex literal."""
    match = re.search(
        r"val EXPECTED_DATAGRAM = hex\((.*?)\)", source, re.DOTALL
    )
    if match is None:
        return None
    pieces = re.findall(r'"([0-9a-fA-F]+)"', match.group(1))
    if not pieces:
        return None
    return bytes.fromhex("".join(pieces))


def kotlin_expected_nonce(source: str) -> bytes | None:
    """Extracts the Kotlin vector test's pinned nonce hex literal."""
    match = re.search(
        r'val EXPECTED_NONCE = hex\("([0-9a-fA-F]+)"\)', source
    )
    if match is None:
        return None
    return bytes.fromhex(match.group(1))


def kotlin_reverse_expected_datagram(source: str) -> bytes | None:
    """Extracts the Kotlin reverse-direction vector's pinned datagram hex."""
    match = re.search(
        r"val EXPECTED_REVERSE_DATAGRAM = hex\((.*?)\)", source, re.DOTALL
    )
    if match is None:
        return None
    pieces = re.findall(r'"([0-9a-fA-F]+)"', match.group(1))
    if not pieces:
        return None
    return bytes.fromhex("".join(pieces))


def kotlin_reverse_expected_nonce(source: str) -> bytes | None:
    """Extracts the Kotlin reverse-direction vector's pinned nonce hex."""
    match = re.search(
        r'val EXPECTED_REVERSE_NONCE = hex\("([0-9a-fA-F]+)"\)', source
    )
    if match is None:
        return None
    return bytes.fromhex(match.group(1))


def cross_source_errors(
    doc_source: str | None = None,
    rust_source: str | None = None,
    kotlin_source: str | None = None,
) -> list[str]:
    """Checks the doc, Rust, and Kotlin pins against the recomputation.

    Reads the pinned sources when not supplied, so the published doc
    datagrams, the Rust test datagrams, the Kotlin vector pin, and the
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
        return [f"cannot read media crypto vector sources: {exc}"]

    sections = doc_vector_sections(doc_source)
    if [number for number, _ in sections] != ["1", "2", "3"]:
        errors.append(
            "media crypto doc must contain exactly Vector 1, Vector 2 then Vector 3 sections"
        )
    rust_datagrams = rust_test_datagrams(rust_source)
    if set(rust_datagrams) != {"1", "2", "3"}:
        errors.append("Rust media crypto test must pin all three vector datagrams")
    kotlin_datagram = kotlin_expected_datagram(kotlin_source)
    if kotlin_datagram is None:
        errors.append("Kotlin media crypto test must pin the vector 1 datagram")
    kotlin_nonce = kotlin_expected_nonce(kotlin_source)
    if kotlin_nonce is None:
        errors.append("Kotlin media crypto test must pin the vector 1 nonce")
    kotlin_reverse_datagram = kotlin_reverse_expected_datagram(kotlin_source)
    if kotlin_reverse_datagram is None:
        errors.append("Kotlin media crypto test must pin the vector 3 datagram")
    kotlin_reverse_nonce = kotlin_reverse_expected_nonce(kotlin_source)
    if kotlin_reverse_nonce is None:
        errors.append("Kotlin media crypto test must pin the vector 3 nonce")
    published = {
        vector.name.split(":", 1)[0].split()[-1]: vector
        for vector in PUBLISHED_VECTORS
    }

    for number, section in sections:
        try:
            vector, nonce, protocol_version, direction_value = parse_doc_vector(section)
        except ValueError as exc:
            errors.append(f"vector {number}: {exc}")
            continue
        expected = published.get(number)
        if expected is None:
            errors.append(f"vector {number}: no published Python pin to compare")
            continue
        if protocol_version != MEDIA_VERSION:
            errors.append(
                f"vector {number}: doc protocol version {protocol_version} "
                f"!= {MEDIA_VERSION}"
            )
        if direction_value != expected.direction:
            errors.append(
                f"vector {number}: doc direction {direction_value} "
                f"!= pin direction {expected.direction}"
            )
        if vector.datagram != expected.datagram:
            errors.append(
                f"vector {number}: doc datagram differs from the Python pin: "
                f"doc {vector.datagram.hex().upper()} "
                f"pin {expected.datagram.hex().upper()}"
            )
        rust_datagram = rust_datagrams.get(number)
        if rust_datagram is None:
            errors.append(f"vector {number}: Rust test datagram not found")
        elif vector.datagram != rust_datagram:
            errors.append(
                f"vector {number}: doc datagram differs from the Rust pin: "
                f"doc {vector.datagram.hex().upper()} "
                f"rust {rust_datagram.hex().upper()}"
            )
        if vector.recompute() != vector.datagram:
            errors.append(
                f"vector {number}: doc datagram does not match the independent "
                "recomputation"
            )
        recomputed_nonce = media_nonce(
            vector.stream_id, vector.key_epoch, 0, vector.direction
        )
        if recomputed_nonce != nonce:
            errors.append(
                f"vector {number}: doc nonce {nonce.hex().upper()} "
                f"!= {recomputed_nonce.hex().upper()}"
            )
        if number == "1":
            if kotlin_datagram is not None and vector.datagram != kotlin_datagram:
                errors.append(
                    f"vector {number}: doc datagram differs from the Kotlin pin: "
                    f"doc {vector.datagram.hex().upper()} "
                    f"kotlin {kotlin_datagram.hex().upper()}"
                )
            if kotlin_nonce is not None and nonce != kotlin_nonce:
                errors.append(
                    f"vector {number}: doc nonce differs from the Kotlin pin: "
                    f"doc {nonce.hex().upper()} "
                    f"kotlin {kotlin_nonce.hex().upper()}"
                )
        if number == "3":
            if kotlin_reverse_datagram is not None and vector.datagram != kotlin_reverse_datagram:
                errors.append(
                    f"vector {number}: doc datagram differs from the Kotlin reverse pin: "
                    f"doc {vector.datagram.hex().upper()} "
                    f"kotlin {kotlin_reverse_datagram.hex().upper()}"
                )
            if kotlin_reverse_nonce is not None and nonce != kotlin_reverse_nonce:
                errors.append(
                    f"vector {number}: doc nonce differs from the Kotlin reverse pin: "
                    f"doc {nonce.hex().upper()} "
                    f"kotlin {kotlin_reverse_nonce.hex().upper()}"
                )
        expected_len = MEDIA_HEADER_LEN + len(vector.plaintext) + POLY1305_TAG_BYTES
        if len(vector.datagram) != expected_len:
            errors.append(
                f"vector {number}: doc datagram length {len(vector.datagram)} "
                f"!= {expected_len}"
            )
    return errors


def main() -> int:
    errors = validate_vectors()
    errors.extend(cross_source_errors())
    for error in errors:
        print(error, file=sys.stderr)
    if errors:
        print(f"media crypto vectors: {len(errors)} error(s)", file=sys.stderr)
        return 1
    print("media crypto vectors: ok")
    return 0


class MediaCryptoVectorTests(unittest.TestCase):
    def test_chacha20_block_matches_rfc_8439(self) -> None:
        key = bytes(range(0x00, 0x20))
        nonce = bytes.fromhex("000000090000004A00000000")
        expected = bytes.fromhex(
            "10F1E7E4D13B5915500FDD1FA32071C4C7D1F4C733C068030422AA9AC3D46C4E"
            "D2826446079FAA0914C2D705D98B02A2B5129CD1DE164EB9CBD083E8A2503C4E"
        )
        self.assertEqual(chacha20_block(key, 1, nonce), expected)

    def test_poly1305_matches_rfc_8439(self) -> None:
        key = bytes.fromhex(
            "85D6BE7857556D337F4452FE42D506A80103808AFB0DB2FD4ABFF6AF4149F51B"
        )
        message = b"Cryptographic Forum Research Group"
        expected = bytes.fromhex("A8061DC1305136C6C22B8BAF0C0127A9")
        self.assertEqual(poly1305(message, key), expected)

    def test_aead_matches_rfc_8439(self) -> None:
        key = bytes(range(0x80, 0xA0))
        nonce = bytes.fromhex("070000004041424344454647")
        aad = bytes.fromhex("50515253C0C1C2C3C4C5C6C7")
        plaintext = (
            b"Ladies and Gentlemen of the class of '99: If I could offer you only "
            b"one tip for the future, sunscreen would be it."
        )
        expected_ciphertext = bytes.fromhex(
            "D31A8D34648E60DB7B86AFBC53EF7EC2A4ADED51296E08FEA9E2B5A736EE62D6"
            "3DBEA45E8CA9671282FAFB69DA92728B1A71DE0A9E060B2905D6A5B67ECD3B36"
            "92DDBD7F2D778B8C9803AEE328091B58FAB324E4FAD675945585808B4831D7BC"
            "3FF4DEF08E4B7A9DE576D26586CEC64B6116"
        )
        expected_tag = bytes.fromhex("1AE10B594F09E26A7E902ECBD0600691")
        ciphertext, tag = aead_encrypt(key, nonce, aad, plaintext)
        self.assertEqual(ciphertext, expected_ciphertext)
        self.assertEqual(tag, expected_tag)

    def test_all_published_datagrams_recompute(self) -> None:
        self.assertEqual(len(PUBLISHED_VECTORS), 3)
        for vector in PUBLISHED_VECTORS:
            self.assertEqual(vector.recompute(), vector.datagram, vector.name)
            self.assertEqual(
                len(vector.datagram),
                MEDIA_HEADER_LEN + len(vector.plaintext) + POLY1305_TAG_BYTES,
                vector.name,
            )

    @staticmethod
    def _doc_section(vector: PublishedVector, datagram: bytes) -> str:
        """Renders one doc vector section from a vector and datagram bytes."""
        lines = [
            f"| Control-session secret | `{vector.secret.hex()}` |",
            f"| Protocol version | `{MEDIA_VERSION}` |",
            f"| Direction | {'Desktop-to-Android' if vector.direction == DESKTOP_TO_ANDROID_DIRECTION else 'Android-to-desktop'} (`{vector.direction}`) |",
            f"| Session ID | `0x{vector.session_id:016x}` |",
            f"| Stream ID | `0x{vector.stream_id:08x}` |",
            f"| Key epoch | `0x{vector.key_epoch:04x}` |",
            "| Sequence | `0` |",
            f"| Flags | `0x{vector.flags:02x}` |",
            f"| Sample timestamp | `0x{vector.sample_timestamp:x}` |",
            f"| Plaintext UTF-8 | `{vector.plaintext.decode('utf-8')}` |",
        ]
        hex_text = datagram.hex().upper()
        hex_lines = [
            hex_text[offset : offset + 64]
            for offset in range(0, len(hex_text), 64)
        ]
        return (
            "\n".join(lines)
            + "\n\nThe 96-bit nonce is `"
            + media_nonce(vector.stream_id, vector.key_epoch, 0, vector.direction).hex()
            + "`.\n\n```text\n"
            + "\n".join(hex_lines)
            + "\n```\n"
        )

    def test_doc_parser_round_trips_a_published_vector(self) -> None:
        for vector in PUBLISHED_VECTORS:
            section = self._doc_section(vector, vector.datagram)
            parsed, nonce, protocol_version, direction_value = parse_doc_vector(section)
            self.assertEqual(parsed.datagram, vector.datagram)
            self.assertEqual(protocol_version, MEDIA_VERSION)
            self.assertEqual(direction_value, vector.direction)
            self.assertEqual(parsed.direction, vector.direction)
            self.assertEqual(
                nonce,
                media_nonce(vector.stream_id, vector.key_epoch, 0, vector.direction),
            )

    def test_doc_parser_detects_a_tampered_datagram(self) -> None:
        vector = PUBLISHED_VECTORS[0]
        tampered = vector.datagram[:-1] + bytes([vector.datagram[-1] ^ 0x01])
        section = self._doc_section(vector, tampered)
        parsed, _, _, _ = parse_doc_vector(section)
        self.assertNotEqual(parsed.recompute(), parsed.datagram)

    def test_rust_parser_extracts_all_pinned_datagrams(self) -> None:
        source = RUST_TEST_PATH.read_text(encoding="utf-8")
        datagrams = rust_test_datagrams(source)
        self.assertEqual(set(datagrams), {"1", "2", "3"})
        by_number = {
            vector.name.split(":", 1)[0].split()[-1]: vector
            for vector in PUBLISHED_VECTORS
        }
        for number, vector in by_number.items():
            self.assertEqual(datagrams[number], vector.datagram)

    def test_rust_parser_detects_a_tampered_pin(self) -> None:
        source = RUST_TEST_PATH.read_text(encoding="utf-8")
        tampered = source.replace("0x53, 0x4E, 0x4D, 0x53", "0x53, 0x4E, 0x4D, 0x54", 1)
        datagrams = rust_test_datagrams(tampered)
        self.assertNotEqual(datagrams["1"], PUBLISHED_VECTORS[0].datagram)

    def test_kotlin_parser_extracts_the_pinned_datagram_and_nonce(self) -> None:
        source = KOTLIN_TEST_PATH.read_text(encoding="utf-8")
        self.assertEqual(
            kotlin_expected_datagram(source), PUBLISHED_VECTORS[0].datagram
        )
        self.assertEqual(
            kotlin_expected_nonce(source),
            media_nonce(
                PUBLISHED_VECTORS[0].stream_id, PUBLISHED_VECTORS[0].key_epoch, 0
            ),
        )

    def test_kotlin_parser_extracts_the_reverse_vector_pin(self) -> None:
        source = KOTLIN_TEST_PATH.read_text(encoding="utf-8")
        vector = PUBLISHED_VECTORS[2]
        self.assertEqual(kotlin_reverse_expected_datagram(source), vector.datagram)
        self.assertEqual(
            kotlin_reverse_expected_nonce(source),
            media_nonce(vector.stream_id, vector.key_epoch, 0, vector.direction),
        )

    def test_kotlin_parser_detects_a_tampered_reverse_pin(self) -> None:
        source = KOTLIN_TEST_PATH.read_text(encoding="utf-8")
        tampered = source.replace("C95A99", "C95A98", 1)
        self.assertNotEqual(
            kotlin_reverse_expected_datagram(tampered), PUBLISHED_VECTORS[2].datagram
        )

    def test_kotlin_parser_detects_a_tampered_pin(self) -> None:
        source = KOTLIN_TEST_PATH.read_text(encoding="utf-8")
        tampered = source.replace("fd67131", "fd67130", 1)
        self.assertNotEqual(
            kotlin_expected_datagram(tampered), PUBLISHED_VECTORS[0].datagram
        )

    def test_cross_source_detects_kotlin_drift(self) -> None:
        doc_source = DOC_PATH.read_text(encoding="utf-8")
        rust_source = RUST_TEST_PATH.read_text(encoding="utf-8")
        kotlin_source = KOTLIN_TEST_PATH.read_text(encoding="utf-8")
        self.assertEqual(
            cross_source_errors(doc_source, rust_source, kotlin_source), []
        )
        tampered_kotlin = kotlin_source.replace("fd67131", "fd67130", 1)
        errors = cross_source_errors(doc_source, rust_source, tampered_kotlin)
        self.assertTrue(
            any("vector 1" in error and "Kotlin pin" in error for error in errors)
        )

    def test_cross_source_detects_doc_and_rust_drift(self) -> None:
        doc_source = DOC_PATH.read_text(encoding="utf-8")
        rust_source = RUST_TEST_PATH.read_text(encoding="utf-8")
        self.assertEqual(cross_source_errors(doc_source, rust_source), [])
        tampered_doc = doc_source.replace("fd67131", "fd67130", 1)
        errors = cross_source_errors(tampered_doc, rust_source)
        self.assertTrue(
            any("vector 1" in error and "doc datagram" in error for error in errors)
        )
        tampered_rust = rust_source.replace(
            "0x53, 0x4E, 0x4D, 0x53", "0x53, 0x4E, 0x4D, 0x54", 1
        )
        errors = cross_source_errors(doc_source, tampered_rust)
        self.assertTrue(
            any("vector 1" in error and "Rust pin" in error for error in errors)
        )


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
