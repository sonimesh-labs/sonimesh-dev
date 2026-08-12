#!/usr/bin/env python3
"""Cross-language check of the protected media AEAD packet vector.

The Rust media plane seals packets with ChaCha20-Poly1305 (RFC 8439) using the
HKDF media key and the 96-bit media nonce. The test
`matches_published_cross_language_aead_packet_vector` in
crates/sonimesh-protocol/tests/media_crypto.rs pins the canonical datagram.
This script reimplements ChaCha20, Poly1305, and the AEAD construction plus the
media key/nonce/header boundaries in pure standard-library Python and verifies
the same published datagram, so an AEAD crate, key, nonce, or header change is
caught by an independent implementation (roadmap Phase 0D: cross-language
crypto and packet test vectors; testing doc: published vectors for AEAD packet
authentication).
"""

from __future__ import annotations

import hashlib
import hmac
import struct
import sys
import unittest

ROOT_SALT = b"sonimesh media root v1"
STREAM_KEY_LABEL = b"sonimesh media stream key v1"
MEDIA_VERSION = 1
MEDIA_MAGIC = 0x534E_4D53
MEDIA_HEADER_LEN = 36
ANDROID_TO_DESKTOP_DIRECTION = 0
POLY1305_TAG_BYTES = 16


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


def derive_media_key(secret: bytes, session_id: int, stream_id: int, key_epoch: int) -> bytes:
    info = (
        STREAM_KEY_LABEL
        + bytes([MEDIA_VERSION])
        + session_id.to_bytes(8, "big")
        + stream_id.to_bytes(4, "big")
        + key_epoch.to_bytes(2, "big")
        + bytes([ANDROID_TO_DESKTOP_DIRECTION])
    )
    return hkdf_expand(hkdf_extract(ROOT_SALT, secret), info, 32)


def media_nonce(stream_id: int, key_epoch: int, sequence: int) -> bytes:
    return (
        key_epoch.to_bytes(2, "big")
        + bytes([ANDROID_TO_DESKTOP_DIRECTION, 0])
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


def published_datagram() -> bytes:
    """The golden datagram pinned by the Rust AEAD vector test."""
    return bytes.fromhex(
        "534E4D53012A0024000000000000000700000009000000001122334455667788002C0003"
        "337E8052F30B4E20479A0E4080A74511AA39C2DEABAFA132DBFD6B85"
        "C7C85145360F62F4361EFE335F16149A"
    )


def recomputed_datagram() -> bytes:
    """Independent recomputation of the published sealed datagram."""
    secret = bytes([4]) * 32
    key = derive_media_key(secret, 7, 9, 3)
    nonce = media_nonce(9, 3, 0)
    plaintext = b"cross-language packet vector"
    header = encode_media_header(
        flags=0x2A,
        session_id=7,
        stream_id=9,
        sequence=0,
        sample_timestamp=0x1122_3344_5566_7788,
        key_epoch=3,
        payload_len=len(plaintext) + POLY1305_TAG_BYTES,
    )
    ciphertext, tag = aead_encrypt(key, nonce, header, plaintext)
    return header + ciphertext + tag


def validate_vectors() -> list[str]:
    """Returns deterministic mismatches between recomputation and the pin."""
    recomputed = recomputed_datagram()
    published = published_datagram()
    if recomputed != published:
        return [
            "media AEAD datagram does not match the published vector: "
            f"recomputed {recomputed.hex().upper()} published {published.hex().upper()}"
        ]
    return []


def main() -> int:
    errors = validate_vectors()
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

    def test_published_datagram_recomputes(self) -> None:
        self.assertEqual(recomputed_datagram(), published_datagram())
        self.assertEqual(len(published_datagram()), MEDIA_HEADER_LEN + 44)


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
