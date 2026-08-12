#!/usr/bin/env python3
"""Cross-language check of the version-one pairing binding vectors.

`PairingBinding` (crates/sonimesh-protocol/src/pairing_binding.rs) derives
per-role proofs and a media root from HMAC-SHA256/HKDF-SHA256 over a pairing
id, pairing secret, and TLS exporter. The Rust unit test
`matches_published_role_proof_and_media_root_vector` pins the golden bytes.
This script reimplements the derivation from the documented labels and
boundaries in pure standard-library Python and verifies the same published
vectors, so a Rust implementation change is caught by an independent
implementation before it becomes a release boundary (roadmap Phase 0D:
cross-language crypto and packet test vectors).
"""

from __future__ import annotations

import hashlib
import hmac
import sys
import unittest

CONTROL_PROTOCOL_VERSION = 1
PAIRING_ID_LEN = 16
PAIRING_SECRET_LEN = 32
PAIRING_TLS_EXPORTER_LEN = 32
PROOF_LABEL = b"sonimesh pairing proof v1"
MEDIA_ROOT_LABEL = b"sonimesh pairing media root v1"

ANDROID_CLIENT_ROLE = 0
DESKTOP_SERVER_ROLE = 1


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


def pairing_prk(pairing_secret: bytes, tls_exporter: bytes) -> bytes:
    """Matches `PairingBinding::new`: HMAC with the secret as the key."""
    if len(pairing_secret) != PAIRING_SECRET_LEN:
        raise ValueError("pairing secret length")
    if len(tls_exporter) != PAIRING_TLS_EXPORTER_LEN:
        raise ValueError("tls exporter length")
    return hkdf_extract(pairing_secret, tls_exporter)


def role_proof(
    prk: bytes,
    protocol_version: int,
    pairing_id: bytes,
    role: int,
) -> bytes:
    """Matches `PairingBinding::proof_for_role` info layout."""
    info = PROOF_LABEL + bytes([protocol_version, role]) + pairing_id
    return hkdf_expand(prk, info, PAIRING_SECRET_LEN)


def media_root(prk: bytes, protocol_version: int, pairing_id: bytes) -> bytes:
    """Matches `PairingBinding::media_root` info layout."""
    info = MEDIA_ROOT_LABEL + bytes([protocol_version]) + pairing_id
    return hkdf_expand(prk, info, PAIRING_SECRET_LEN)


def derive_vectors() -> dict[str, bytes]:
    """Recomputes the published vectors from the documented fixed inputs."""
    pairing_id = bytes(range(PAIRING_ID_LEN))
    pairing_secret = bytes(range(0x10, 0x10 + PAIRING_SECRET_LEN))
    tls_exporter = bytes(range(0xA0, 0xA0 + PAIRING_TLS_EXPORTER_LEN))
    prk = pairing_prk(pairing_secret, tls_exporter)
    return {
        "client_proof": role_proof(
            prk, CONTROL_PROTOCOL_VERSION, pairing_id, ANDROID_CLIENT_ROLE
        ),
        "server_proof": role_proof(
            prk, CONTROL_PROTOCOL_VERSION, pairing_id, DESKTOP_SERVER_ROLE
        ),
        "media_root": media_root(prk, CONTROL_PROTOCOL_VERSION, pairing_id),
    }


def published_vectors() -> dict[str, bytes]:
    """The golden bytes pinned by the Rust unit test."""
    return {
        "client_proof": bytes.fromhex(
            "C274DFE72135AE558034CA1E8A27A4EAB7AAC9B87A0CD93C17BF17126E27F4F2"
        ),
        "server_proof": bytes.fromhex(
            "F347AB32006E8ACB220F91B0E826BBD58257C55773D7284F8794217F896B4601"
        ),
        "media_root": bytes.fromhex(
            "25B4E89FB655ED5A9A9AD9B28E6AC7E6D49C1246757B75F20A120167BFD2452A"
        ),
    }


def validate_vectors() -> list[str]:
    """Returns deterministic mismatches between the recomputation and the pins."""
    recomputed = derive_vectors()
    published = published_vectors()
    errors: list[str] = []
    for name in ("client_proof", "server_proof", "media_root"):
        if recomputed[name] != published[name]:
            errors.append(
                f"pairing {name} does not match the published vector: "
                f"recomputed {recomputed[name].hex().upper()} "
                f"published {published[name].hex().upper()}"
            )
    if recomputed["client_proof"] == recomputed["server_proof"]:
        errors.append("pairing role proofs must differ")
    if recomputed["media_root"] == recomputed["client_proof"]:
        errors.append("pairing media root must differ from the client proof")
    if recomputed["media_root"] == recomputed["server_proof"]:
        errors.append("pairing media root must differ from the server proof")
    return errors


def main() -> int:
    errors = validate_vectors()
    for error in errors:
        print(error, file=sys.stderr)
    if errors:
        print(f"pairing vectors: {len(errors)} error(s)", file=sys.stderr)
        return 1
    print("pairing vectors: ok")
    return 0


class PairingVectorTests(unittest.TestCase):
    def test_hkdf_matches_rfc_5869_sha256_case_1(self) -> None:
        ikm = bytes([0x0B]) * 22
        salt = bytes(range(0x00, 0x0D))
        info = bytes(range(0xF0, 0xFA))
        length = 42
        expected = bytes.fromhex(
            "3CB25F25FAACD57A90434F64D0362F2A2D2D0A90CF1A5A4C5DB02D56ECC4C5BF"
            "34007208D5B887185865"
        )
        prk = hkdf_extract(salt, ikm)
        self.assertEqual(
            prk.hex(),
            "077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5",
        )
        self.assertEqual(hkdf_expand(prk, info, length), expected)

    def test_vectors_recompute_and_roles_are_distinct(self) -> None:
        recomputed = derive_vectors()
        published = published_vectors()
        for name, value in recomputed.items():
            self.assertEqual(value, published[name], name)
        self.assertNotEqual(recomputed["client_proof"], recomputed["server_proof"])
        self.assertNotEqual(recomputed["media_root"], recomputed["client_proof"])
        self.assertNotEqual(recomputed["media_root"], recomputed["server_proof"])
        for value in recomputed.values():
            self.assertEqual(len(value), PAIRING_SECRET_LEN)

    def test_rejects_noncanonical_input_lengths(self) -> None:
        with self.assertRaises(ValueError):
            pairing_prk(bytes(PAIRING_SECRET_LEN), bytes(PAIRING_TLS_EXPORTER_LEN - 1))
        with self.assertRaises(ValueError):
            pairing_prk(bytes(PAIRING_SECRET_LEN - 1), bytes(PAIRING_TLS_EXPORTER_LEN))

    def test_derivation_is_sensitive_to_every_input(self) -> None:
        for index in range(PAIRING_ID_LEN):
            pairing_id = bytearray(range(PAIRING_ID_LEN))
            pairing_id[index] ^= 0x01
            prk = pairing_prk(
                bytes(range(0x10, 0x10 + PAIRING_SECRET_LEN)), bytes(range(0xA0, 0xC0))
            )
            proof = role_proof(prk, CONTROL_PROTOCOL_VERSION, bytes(pairing_id), ANDROID_CLIENT_ROLE)
            base = role_proof(
                prk, CONTROL_PROTOCOL_VERSION, bytes(range(PAIRING_ID_LEN)), ANDROID_CLIENT_ROLE
            )
            self.assertNotEqual(proof, base, f"pairing id byte {index}")


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
