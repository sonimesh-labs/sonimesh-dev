#!/usr/bin/env python3
"""Cross-language check of the version-one hello capability negotiation.

`negotiate_hello` (crates/sonimesh-protocol/src/negotiation.rs) selects the
current control protocol version and the canonical capability intersection
between the local sender policy and the receiver hello. The Rust tests and the
Android host-JVM `HelloNegotiation` mirror pin the same canonical vector and
the bounded hello admission rules. This script reimplements the version
selection and canonical intersection in pure standard-library Python, verifies
the published vector, and parses the Rust and Kotlin pins so the three
implementations cannot drift apart (roadmap Phase 0D: cross-language crypto and
packet test vectors; testing doc: bounded version-one hello negotiation).
"""

from __future__ import annotations

import re
import sys
import unittest
from pathlib import Path

CONTROL_PROTOCOL_VERSION = 1
MAX_CONTROL_CAPABILITIES = 16
MAX_CONTROL_SUPPORTED_VERSIONS = 8
MAX_CONTROL_DISPLAY_NAME_BYTES = 128

# Capability discriminants from proto/control.proto and the Kotlin mirror.
CAPABILITY_BY_NAME = {
    "Unspecified": 0,
    "FrameDuration10Ms": 1,
    "FrameDuration20Ms": 2,
    "ManualAddress": 3,
    "NetworkDiagnostics": 4,
}

# Canonical order in which known capabilities are selected by negotiate_hello.
KNOWN_CAPABILITIES = [
    CAPABILITY_BY_NAME["FrameDuration10Ms"],
    CAPABILITY_BY_NAME["FrameDuration20Ms"],
    CAPABILITY_BY_NAME["ManualAddress"],
    CAPABILITY_BY_NAME["NetworkDiagnostics"],
]

REPO_ROOT = Path(__file__).resolve().parent.parent
RUST_SRC_PATH = REPO_ROOT / "crates" / "sonimesh-protocol" / "src" / "negotiation.rs"
RUST_CONSTANTS_PATH = REPO_ROOT / "crates" / "sonimesh-protocol" / "src" / "control.rs"
RUST_TEST_PATH = (
    REPO_ROOT / "crates" / "sonimesh-protocol" / "tests" / "negotiation.rs"
)
KOTLIN_MIRROR_PATH = (
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
    / "HelloNegotiation.kt"
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
    / "HelloNegotiationVectorTest.kt"
)

RUST_CONSTANT_NAMES = (
    "CONTROL_PROTOCOL_VERSION",
    "MAX_CONTROL_CAPABILITIES",
    "MAX_CONTROL_SUPPORTED_VERSIONS",
    "MAX_CONTROL_DISPLAY_NAME_BYTES",
)

KOTLIN_CAPABILITY_CONSTANTS = (
    ("CAPABILITY_FRAME_DURATION_10_MS", "FrameDuration10Ms"),
    ("CAPABILITY_FRAME_DURATION_20_MS", "FrameDuration20Ms"),
    ("CAPABILITY_MANUAL_ADDRESS", "ManualAddress"),
    ("CAPABILITY_NETWORK_DIAGNOSTICS", "NetworkDiagnostics"),
)


class NegotiationError(Exception):
    """Base error mirroring ControlNegotiationError."""


class LocalTooManyCapabilities(NegotiationError):
    """The local capability policy exceeds the fixed maximum."""


class PeerHelloInvalid(NegotiationError):
    """The peer hello violated the bounded control schema."""


class NoCommonProtocolVersion(NegotiationError):
    """The peer did not advertise the exact version this build supports."""


def validate_hello(
    peer_versions: list[int],
    peer_capabilities: list[int],
    peer_display_name: str,
) -> None:
    """Matches `validate_hello`: bounded capability, version, and name checks."""
    if len(peer_capabilities) > MAX_CONTROL_CAPABILITIES:
        raise PeerHelloInvalid("peer hello has too many capabilities")
    if len(peer_versions) > MAX_CONTROL_SUPPORTED_VERSIONS:
        raise PeerHelloInvalid("peer hello has too many supported versions")
    if len(peer_display_name.encode("utf-8")) > MAX_CONTROL_DISPLAY_NAME_BYTES:
        raise PeerHelloInvalid("peer hello display name is too long")


def negotiate_hello(
    peer_versions: list[int],
    peer_capabilities: list[int],
    peer_display_name: str,
    local_capabilities: list[int],
) -> tuple[int, list[int]]:
    """Matches `negotiate_hello`: version selection and canonical intersection."""
    if len(local_capabilities) > MAX_CONTROL_CAPABILITIES:
        raise LocalTooManyCapabilities
    validate_hello(peer_versions, peer_capabilities, peer_display_name)
    if CONTROL_PROTOCOL_VERSION not in peer_versions:
        raise NoCommonProtocolVersion
    capabilities = [
        capability
        for capability in KNOWN_CAPABILITIES
        if capability in local_capabilities and capability in peer_capabilities
    ]
    return CONTROL_PROTOCOL_VERSION, capabilities


def published_vector() -> dict[str, object]:
    """The canonical vector pinned by the Rust and Kotlin negotiation tests."""
    return {
        "peer_versions": [99, CONTROL_PROTOCOL_VERSION, CONTROL_PROTOCOL_VERSION],
        "peer_capabilities": [
            CAPABILITY_BY_NAME["NetworkDiagnostics"],
            CAPABILITY_BY_NAME["FrameDuration20Ms"],
            CAPABILITY_BY_NAME["FrameDuration20Ms"],
            9_999,
        ],
        "peer_display_name": "Lab sender",
        "local_capabilities": [
            CAPABILITY_BY_NAME["NetworkDiagnostics"],
            CAPABILITY_BY_NAME["FrameDuration20Ms"],
            CAPABILITY_BY_NAME["ManualAddress"],
        ],
        "version": CONTROL_PROTOCOL_VERSION,
        "capabilities": [
            CAPABILITY_BY_NAME["FrameDuration20Ms"],
            CAPABILITY_BY_NAME["NetworkDiagnostics"],
        ],
    }


def rust_constants(source: str) -> dict[str, int]:
    """Extracts the bounded hello constants from src/control.rs."""
    values: dict[str, int] = {}
    for name in RUST_CONSTANT_NAMES:
        match = re.search(rf"pub const {name}: [^;=]+= (\d+);", source)
        if match is not None:
            values[name] = int(match.group(1))
    return values


def rust_known_capabilities(source: str) -> list[int] | None:
    """Extracts the canonical KNOWN_CAPABILITIES order from src/negotiation.rs."""
    match = re.search(
        r"const KNOWN_CAPABILITIES: \[Capability; \d+\] = \[(.*?)\];",
        source,
        re.DOTALL,
    )
    if match is None:
        return None
    values: list[int] = []
    for name in re.findall(r"Capability::(\w+)", match.group(1)):
        if name not in CAPABILITY_BY_NAME:
            return None
        values.append(CAPABILITY_BY_NAME[name])
    return values or None


def rust_canonical_pin(source: str) -> tuple[int | None, list[int] | None]:
    """Extracts the canonical vector result pinned by tests/negotiation.rs."""
    version: int | None = None
    version_match = re.search(
        r"negotiated\.version\(\),\s*(CONTROL_PROTOCOL_VERSION|\d+)\)", source
    )
    if version_match is not None:
        version = (
            CONTROL_PROTOCOL_VERSION
            if version_match.group(1) == "CONTROL_PROTOCOL_VERSION"
            else int(version_match.group(1))
        )
    capabilities: list[int] | None = None
    capabilities_match = re.search(
        r"negotiated\.capabilities\(\),\s*\[\s*Capability::(\w+),\s*"
        r"Capability::(\w+),\s*\]",
        source,
    )
    if capabilities_match is not None:
        capabilities = [
            CAPABILITY_BY_NAME[capabilities_match.group(1)],
            CAPABILITY_BY_NAME[capabilities_match.group(2)],
        ]
    return version, capabilities


def kotlin_constants(source: str) -> dict[str, int]:
    """Extracts the bounded hello constants and capability values from the mirror."""
    values: dict[str, int] = {}
    for name in RUST_CONSTANT_NAMES:
        match = re.search(rf"const val {name} = (\d+)", source)
        if match is not None:
            values[name] = int(match.group(1))
    for kotlin_name, _ in KOTLIN_CAPABILITY_CONSTANTS:
        match = re.search(rf"const val {kotlin_name} = (\d+)", source)
        if match is not None:
            values[kotlin_name] = int(match.group(1))
    return values


def kotlin_known_capabilities(
    source: str, kotlin_values: dict[str, int]
) -> list[int] | None:
    """Extracts the canonical KNOWN_CAPABILITIES order from the Kotlin mirror."""
    match = re.search(r"val KNOWN_CAPABILITIES = listOf\((.*?)\)", source, re.DOTALL)
    if match is None:
        return None
    values: list[int] = []
    for name in re.findall(r"CAPABILITY_(\w+)", match.group(1)):
        const_name = "CAPABILITY_" + name
        if const_name not in kotlin_values:
            return None
        values.append(kotlin_values[const_name])
    return values or None


def kotlin_canonical_pin(source: str) -> tuple[int | None, list[int] | None]:
    """Extracts the canonical vector result pinned by HelloNegotiationVectorTest."""
    version: int | None = None
    if re.search(
        r"assertEquals\(HelloNegotiation\.CONTROL_PROTOCOL_VERSION, "
        r"negotiated\.version\)",
        source,
    ):
        version = CONTROL_PROTOCOL_VERSION
    capabilities: list[int] | None = None
    if re.search(
        r"assertEquals\(\s*listOf\(\s*HelloNegotiation\.CAPABILITY_FRAME_DURATION_20_MS,"
        r"\s*HelloNegotiation\.CAPABILITY_NETWORK_DIAGNOSTICS,\s*\),"
        r"\s*negotiated\.capabilities,\s*\)",
        source,
        re.DOTALL,
    ):
        capabilities = [
            CAPABILITY_BY_NAME["FrameDuration20Ms"],
            CAPABILITY_BY_NAME["NetworkDiagnostics"],
        ]
    return version, capabilities


def cross_source_errors(
    rust_constants_text: str,
    rust_src_text: str,
    rust_test_text: str,
    kotlin_text: str,
    kotlin_test_text: str,
) -> list[str]:
    """Returns deterministic mismatches across the recomputation and the pins."""
    errors: list[str] = []
    vector = published_vector()

    try:
        version, capabilities = negotiate_hello(
            vector["peer_versions"],
            vector["peer_capabilities"],
            vector["peer_display_name"],
            vector["local_capabilities"],
        )
    except NegotiationError as exc:
        errors.append(f"published negotiation vector failed to recompute: {exc}")
        version, capabilities = None, None
    if version != vector["version"] or capabilities != vector["capabilities"]:
        errors.append(
            f"negotiation recomputation does not match the published vector: "
            f"got version={version} capabilities={capabilities} expected "
            f"version={vector['version']} capabilities={vector['capabilities']}"
        )

    expected_constants = {
        "CONTROL_PROTOCOL_VERSION": CONTROL_PROTOCOL_VERSION,
        "MAX_CONTROL_CAPABILITIES": MAX_CONTROL_CAPABILITIES,
        "MAX_CONTROL_SUPPORTED_VERSIONS": MAX_CONTROL_SUPPORTED_VERSIONS,
        "MAX_CONTROL_DISPLAY_NAME_BYTES": MAX_CONTROL_DISPLAY_NAME_BYTES,
    }
    rust_values = rust_constants(rust_constants_text)
    for name, expected in expected_constants.items():
        value = rust_values.get(name)
        if value is None:
            errors.append(f"negotiation constant {name}: Rust pin not found")
        elif value != expected:
            errors.append(
                f"negotiation constant {name} differs from the Rust pin: "
                f"python {expected} rust {value}"
            )

    known = rust_known_capabilities(rust_src_text)
    if known is None:
        errors.append("negotiation: Rust KNOWN_CAPABILITIES pin not found")
    elif known != KNOWN_CAPABILITIES:
        errors.append(
            "negotiation KNOWN_CAPABILITIES differs from the Rust pin: "
            f"python {KNOWN_CAPABILITIES} rust {known}"
        )

    rust_version, rust_capabilities = rust_canonical_pin(rust_test_text)
    if rust_version is None or rust_capabilities is None:
        errors.append("negotiation: Rust canonical vector pin not found")
    else:
        if rust_version != vector["version"]:
            errors.append(
                f"negotiation version differs from the Rust pin: "
                f"python {vector['version']} rust {rust_version}"
            )
        if rust_capabilities != vector["capabilities"]:
            errors.append(
                f"negotiation capabilities differ from the Rust pin: "
                f"python {vector['capabilities']} rust {rust_capabilities}"
            )

    kotlin_values = kotlin_constants(kotlin_text)
    for name, expected in expected_constants.items():
        value = kotlin_values.get(name)
        if value is None:
            errors.append(f"negotiation constant {name}: Kotlin pin not found")
        elif value != expected:
            errors.append(
                f"negotiation constant {name} differs from the Kotlin pin: "
                f"python {expected} kotlin {value}"
            )
    for kotlin_name, python_name in KOTLIN_CAPABILITY_CONSTANTS:
        value = kotlin_values.get(kotlin_name)
        expected = CAPABILITY_BY_NAME[python_name]
        if value is None:
            errors.append(f"capability {python_name}: Kotlin value pin not found")
        elif value != expected:
            errors.append(
                f"capability {python_name} differs from the Kotlin pin: "
                f"python {expected} kotlin {value}"
            )

    kotlin_known = kotlin_known_capabilities(kotlin_text, kotlin_values)
    if kotlin_known is None:
        errors.append("negotiation: Kotlin KNOWN_CAPABILITIES pin not found")
    elif kotlin_known != KNOWN_CAPABILITIES:
        errors.append(
            "negotiation KNOWN_CAPABILITIES differs from the Kotlin pin: "
            f"python {KNOWN_CAPABILITIES} kotlin {kotlin_known}"
        )

    kotlin_version, kotlin_capabilities = kotlin_canonical_pin(kotlin_test_text)
    if kotlin_version is None or kotlin_capabilities is None:
        errors.append("negotiation: Kotlin canonical vector pin not found")
    else:
        if kotlin_version != vector["version"]:
            errors.append(
                f"negotiation version differs from the Kotlin pin: "
                f"python {vector['version']} kotlin {kotlin_version}"
            )
        if kotlin_capabilities != vector["capabilities"]:
            errors.append(
                f"negotiation capabilities differ from the Kotlin pin: "
                f"python {vector['capabilities']} kotlin {kotlin_capabilities}"
            )

    return errors


def validate_vectors() -> list[str]:
    """Returns mismatches against the on-disk Rust and Kotlin pins."""
    return cross_source_errors(
        RUST_CONSTANTS_PATH.read_text(encoding="utf-8"),
        RUST_SRC_PATH.read_text(encoding="utf-8"),
        RUST_TEST_PATH.read_text(encoding="utf-8"),
        KOTLIN_MIRROR_PATH.read_text(encoding="utf-8"),
        KOTLIN_TEST_PATH.read_text(encoding="utf-8"),
    )


def main() -> int:
    errors = validate_vectors()
    for error in errors:
        print(error, file=sys.stderr)
    if errors:
        print(f"negotiation vectors: {len(errors)} error(s)", file=sys.stderr)
        return 1
    print("negotiation vectors: ok")
    return 0


class NegotiationVectorTests(unittest.TestCase):
    def test_published_vector_recomputes(self) -> None:
        vector = published_vector()
        version, capabilities = negotiate_hello(
            vector["peer_versions"],
            vector["peer_capabilities"],
            vector["peer_display_name"],
            vector["local_capabilities"],
        )
        self.assertEqual(version, vector["version"])
        self.assertEqual(capabilities, vector["capabilities"])
        self.assertEqual(capabilities, [2, 4])

    def test_rejects_when_peer_lacks_the_current_version(self) -> None:
        with self.assertRaises(NoCommonProtocolVersion):
            negotiate_hello([2], [], "Lab sender", [])

    def test_rejects_oversized_local_policy_and_accepts_the_boundary(self) -> None:
        with self.assertRaises(LocalTooManyCapabilities):
            negotiate_hello(
                [CONTROL_PROTOCOL_VERSION], [], "Lab sender",
                [3] * (MAX_CONTROL_CAPABILITIES + 1),
            )
        negotiate_hello(
            [CONTROL_PROTOCOL_VERSION], [], "Lab sender", [3] * MAX_CONTROL_CAPABILITIES
        )

    def test_rejects_invalid_peer_hello_before_selecting_features(self) -> None:
        with self.assertRaises(PeerHelloInvalid):
            negotiate_hello(
                [CONTROL_PROTOCOL_VERSION],
                [3] * (MAX_CONTROL_CAPABILITIES + 1),
                "Lab sender",
                [3],
            )
        with self.assertRaises(PeerHelloInvalid):
            negotiate_hello(
                [0] * (MAX_CONTROL_SUPPORTED_VERSIONS + 1), [], "Lab sender", []
            )
        with self.assertRaises(PeerHelloInvalid):
            negotiate_hello(
                [CONTROL_PROTOCOL_VERSION],
                [],
                "a" * (MAX_CONTROL_DISPLAY_NAME_BYTES + 1),
                [],
            )
        negotiate_hello(
            [CONTROL_PROTOCOL_VERSION], [], "a" * MAX_CONTROL_DISPLAY_NAME_BYTES, []
        )

    def test_display_name_limit_is_utf8_bytes_not_characters(self) -> None:
        accepted = "界" * 42  # 126 UTF-8 bytes.
        negotiate_hello([CONTROL_PROTOCOL_VERSION], [], accepted, [])
        with self.assertRaises(PeerHelloInvalid):
            negotiate_hello([CONTROL_PROTOCOL_VERSION], [], "界" * 43, [])

    def test_unknown_peer_capabilities_are_ignored(self) -> None:
        version, capabilities = negotiate_hello(
            [CONTROL_PROTOCOL_VERSION],
            [9_999, CAPABILITY_BY_NAME["ManualAddress"]],
            "Lab sender",
            [CAPABILITY_BY_NAME["ManualAddress"]],
        )
        self.assertEqual(version, CONTROL_PROTOCOL_VERSION)
        self.assertEqual(capabilities, [CAPABILITY_BY_NAME["ManualAddress"]])

    def test_rust_constants_parser_round_trips(self) -> None:
        values = rust_constants(RUST_CONSTANTS_PATH.read_text(encoding="utf-8"))
        self.assertEqual(values["CONTROL_PROTOCOL_VERSION"], CONTROL_PROTOCOL_VERSION)
        self.assertEqual(values["MAX_CONTROL_CAPABILITIES"], MAX_CONTROL_CAPABILITIES)
        self.assertEqual(
            values["MAX_CONTROL_SUPPORTED_VERSIONS"], MAX_CONTROL_SUPPORTED_VERSIONS
        )
        self.assertEqual(
            values["MAX_CONTROL_DISPLAY_NAME_BYTES"], MAX_CONTROL_DISPLAY_NAME_BYTES
        )

    def test_rust_constants_parser_detects_a_tampered_pin(self) -> None:
        source = RUST_CONSTANTS_PATH.read_text(encoding="utf-8")
        tampered = source.replace(
            "pub const MAX_CONTROL_CAPABILITIES: usize = 16;",
            "pub const MAX_CONTROL_CAPABILITIES: usize = 17;",
            1,
        )
        self.assertEqual(rust_constants(tampered)["MAX_CONTROL_CAPABILITIES"], 17)

    def test_rust_known_capabilities_parser_round_trips(self) -> None:
        known = rust_known_capabilities(RUST_SRC_PATH.read_text(encoding="utf-8"))
        self.assertEqual(known, KNOWN_CAPABILITIES)

    def test_rust_known_capabilities_parser_detects_a_tampered_pin(self) -> None:
        source = RUST_SRC_PATH.read_text(encoding="utf-8")
        tampered = source.replace(
            "Capability::FrameDuration10Ms,", "Capability::Unspecified,", 1
        )
        self.assertNotEqual(rust_known_capabilities(tampered), KNOWN_CAPABILITIES)

    def test_rust_canonical_pin_parser_round_trips(self) -> None:
        version, capabilities = rust_canonical_pin(
            RUST_TEST_PATH.read_text(encoding="utf-8")
        )
        self.assertEqual(version, CONTROL_PROTOCOL_VERSION)
        self.assertEqual(capabilities, [2, 4])

    def test_rust_canonical_pin_parser_detects_a_tampered_pin(self) -> None:
        source = RUST_TEST_PATH.read_text(encoding="utf-8")
        tampered = source.replace(
            "Capability::NetworkDiagnostics,\n        ]",
            "Capability::ManualAddress,\n        ]",
            1,
        )
        _, capabilities = rust_canonical_pin(tampered)
        self.assertNotEqual(capabilities, [2, 4])

    def test_kotlin_constants_parser_round_trips(self) -> None:
        values = kotlin_constants(KOTLIN_MIRROR_PATH.read_text(encoding="utf-8"))
        self.assertEqual(values["CONTROL_PROTOCOL_VERSION"], CONTROL_PROTOCOL_VERSION)
        self.assertEqual(values["MAX_CONTROL_CAPABILITIES"], MAX_CONTROL_CAPABILITIES)
        self.assertEqual(
            values["MAX_CONTROL_SUPPORTED_VERSIONS"], MAX_CONTROL_SUPPORTED_VERSIONS
        )
        self.assertEqual(
            values["MAX_CONTROL_DISPLAY_NAME_BYTES"], MAX_CONTROL_DISPLAY_NAME_BYTES
        )
        for kotlin_name, python_name in KOTLIN_CAPABILITY_CONSTANTS:
            self.assertEqual(
                values[kotlin_name], CAPABILITY_BY_NAME[python_name], kotlin_name
            )

    def test_kotlin_constants_parser_detects_a_tampered_pin(self) -> None:
        source = KOTLIN_MIRROR_PATH.read_text(encoding="utf-8")
        tampered = source.replace(
            "const val MAX_CONTROL_CAPABILITIES = 16",
            "const val MAX_CONTROL_CAPABILITIES = 17",
            1,
        )
        self.assertEqual(kotlin_constants(tampered)["MAX_CONTROL_CAPABILITIES"], 17)

    def test_kotlin_known_capabilities_parser_round_trips(self) -> None:
        source = KOTLIN_MIRROR_PATH.read_text(encoding="utf-8")
        values = kotlin_constants(source)
        self.assertEqual(kotlin_known_capabilities(source, values), KNOWN_CAPABILITIES)

    def test_kotlin_known_capabilities_parser_detects_a_tampered_pin(self) -> None:
        source = KOTLIN_MIRROR_PATH.read_text(encoding="utf-8")
        tampered = source.replace(
            "CAPABILITY_FRAME_DURATION_10_MS,", "CAPABILITY_MANUAL_ADDRESS,", 1
        )
        values = kotlin_constants(tampered)
        self.assertNotEqual(kotlin_known_capabilities(tampered, values), KNOWN_CAPABILITIES)

    def test_kotlin_canonical_pin_parser_round_trips(self) -> None:
        version, capabilities = kotlin_canonical_pin(
            KOTLIN_TEST_PATH.read_text(encoding="utf-8")
        )
        self.assertEqual(version, CONTROL_PROTOCOL_VERSION)
        self.assertEqual(capabilities, [2, 4])

    def test_kotlin_canonical_pin_parser_detects_a_tampered_pin(self) -> None:
        source = KOTLIN_TEST_PATH.read_text(encoding="utf-8")
        # The canonical assert is the only place the FRAME then NETWORK pair
        # appears consecutively; the input vector lists them in other orders.
        tampered = source.replace(
            "HelloNegotiation.CAPABILITY_FRAME_DURATION_20_MS,\n"
            "                HelloNegotiation.CAPABILITY_NETWORK_DIAGNOSTICS,",
            "HelloNegotiation.CAPABILITY_FRAME_DURATION_20_MS,\n"
            "                HelloNegotiation.CAPABILITY_MANUAL_ADDRESS,",
            1,
        )
        _, capabilities = kotlin_canonical_pin(tampered)
        self.assertNotEqual(capabilities, [2, 4])

    def test_validate_reports_drift_from_each_source(self) -> None:
        self.assertEqual(validate_vectors(), [])

        rust_constants_text = RUST_CONSTANTS_PATH.read_text(encoding="utf-8")
        rust_src_text = RUST_SRC_PATH.read_text(encoding="utf-8")
        rust_test_text = RUST_TEST_PATH.read_text(encoding="utf-8")
        kotlin_text = KOTLIN_MIRROR_PATH.read_text(encoding="utf-8")
        kotlin_test_text = KOTLIN_TEST_PATH.read_text(encoding="utf-8")

        tampered_rust_constants = rust_constants_text.replace(
            "pub const CONTROL_PROTOCOL_VERSION: u32 = 1;",
            "pub const CONTROL_PROTOCOL_VERSION: u32 = 2;",
            1,
        )
        errors = cross_source_errors(
            tampered_rust_constants, rust_src_text, rust_test_text,
            kotlin_text, kotlin_test_text,
        )
        self.assertTrue(
            any("CONTROL_PROTOCOL_VERSION" in e and "Rust pin" in e for e in errors)
        )

        tampered_kotlin = kotlin_text.replace(
            "const val MAX_CONTROL_DISPLAY_NAME_BYTES = 128",
            "const val MAX_CONTROL_DISPLAY_NAME_BYTES = 127",
            1,
        )
        errors = cross_source_errors(
            rust_constants_text, rust_src_text, rust_test_text,
            tampered_kotlin, kotlin_test_text,
        )
        self.assertTrue(
            any("MAX_CONTROL_DISPLAY_NAME_BYTES" in e and "Kotlin pin" in e for e in errors)
        )



if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
