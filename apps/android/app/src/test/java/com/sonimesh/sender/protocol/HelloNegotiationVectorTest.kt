package com.sonimesh.sender.protocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

/**
 * Cross-language mirror of `sonimesh-protocol`'s `negotiate_hello` and hello
 * validation bounds. The wire vectors already pin the frame bytes; this pins
 * the canonical capability intersection and bounded hello admission so the
 * Android sender and the Rust receiver select the same control capability set.
 */
class HelloNegotiationVectorTest {
    private data class NegotiatedControl(
        val version: Int,
        val capabilities: List<Int>,
    )

    private fun validateHello(
        supportedVersions: List<Long>,
        capabilities: List<Long>,
        displayName: String,
    ) {
        require(capabilities.size <= MAX_CONTROL_CAPABILITIES) {
            "hello capability count exceeds the protocol bound"
        }
        require(supportedVersions.size <= MAX_CONTROL_SUPPORTED_VERSIONS) {
            "hello version count exceeds the protocol bound"
        }
        require(displayName.toByteArray(Charsets.UTF_8).size <= MAX_CONTROL_DISPLAY_NAME_BYTES) {
            "hello display name exceeds the byte bound"
        }
    }

    private fun negotiateHello(
        peerSupportedVersions: List<Long>,
        peerCapabilities: List<Long>,
        peerDisplayName: String,
        localCapabilities: List<Int>,
    ): NegotiatedControl {
        require(localCapabilities.size <= MAX_CONTROL_CAPABILITIES) {
            "local capability policy exceeds the hard limit"
        }
        validateHello(peerSupportedVersions, peerCapabilities, peerDisplayName)
        require(CONTROL_PROTOCOL_VERSION.toLong() in peerSupportedVersions) {
            "peer does not support this control protocol version"
        }
        val capabilities = KNOWN_CAPABILITIES.filter { capability ->
            capability in localCapabilities && capability.toLong() in peerCapabilities
        }
        return NegotiatedControl(CONTROL_PROTOCOL_VERSION, capabilities)
    }

    @Test
    fun selects_current_version_and_canonical_common_capabilities() {
        val negotiated = negotiateHello(
            peerSupportedVersions = listOf(
                99,
                CONTROL_PROTOCOL_VERSION.toLong(),
                CONTROL_PROTOCOL_VERSION.toLong(),
            ),
            peerCapabilities = listOf(
                CAPABILITY_NETWORK_DIAGNOSTICS.toLong(),
                CAPABILITY_FRAME_DURATION_20_MS.toLong(),
                CAPABILITY_FRAME_DURATION_20_MS.toLong(),
                9_999,
            ),
            peerDisplayName = "Lab sender",
            localCapabilities = listOf(
                CAPABILITY_NETWORK_DIAGNOSTICS,
                CAPABILITY_FRAME_DURATION_20_MS,
                CAPABILITY_MANUAL_ADDRESS,
            ),
        )

        assertEquals(CONTROL_PROTOCOL_VERSION, negotiated.version)
        assertEquals(
            listOf(CAPABILITY_FRAME_DURATION_20_MS, CAPABILITY_NETWORK_DIAGNOSTICS),
            negotiated.capabilities,
        )
    }

    @Test
    fun rejects_incompatible_versions_and_bounded_policy_violations() {
        assertThrows(IllegalArgumentException::class.java) {
            negotiateHello(
                peerSupportedVersions = listOf(2),
                peerCapabilities = emptyList(),
                peerDisplayName = "Lab sender",
                localCapabilities = emptyList(),
            )
        }

        val oversizedPolicy = List(MAX_CONTROL_CAPABILITIES + 1) { CAPABILITY_MANUAL_ADDRESS }
        assertThrows(IllegalArgumentException::class.java) {
            negotiateHello(
                peerSupportedVersions = listOf(CONTROL_PROTOCOL_VERSION.toLong()),
                peerCapabilities = emptyList(),
                peerDisplayName = "Lab sender",
                localCapabilities = oversizedPolicy,
            )
        }
    }

    @Test
    fun rejects_invalid_peer_hello_before_selecting_features() {
        val oversizedPeer = List(MAX_CONTROL_CAPABILITIES + 1) { CAPABILITY_MANUAL_ADDRESS.toLong() }
        assertThrows(IllegalArgumentException::class.java) {
            negotiateHello(
                peerSupportedVersions = listOf(CONTROL_PROTOCOL_VERSION.toLong()),
                peerCapabilities = oversizedPeer,
                peerDisplayName = "Lab sender",
                localCapabilities = listOf(CAPABILITY_MANUAL_ADDRESS),
            )
        }
    }

    @Test
    fun rejects_oversized_version_and_display_name_hello_bounds() {
        val tooManyVersions = List(MAX_CONTROL_SUPPORTED_VERSIONS + 1) { it.toLong() }
        assertThrows(IllegalArgumentException::class.java) {
            negotiateHello(
                peerSupportedVersions = tooManyVersions,
                peerCapabilities = emptyList(),
                peerDisplayName = "Lab sender",
                localCapabilities = emptyList(),
            )
        }

        val oversizedName = "a".repeat(MAX_CONTROL_DISPLAY_NAME_BYTES + 1)
        assertThrows(IllegalArgumentException::class.java) {
            negotiateHello(
                peerSupportedVersions = listOf(CONTROL_PROTOCOL_VERSION.toLong()),
                peerCapabilities = emptyList(),
                peerDisplayName = oversizedName,
                localCapabilities = emptyList(),
            )
        }

        val boundaryName = "a".repeat(MAX_CONTROL_DISPLAY_NAME_BYTES)
        assertEquals(
            CONTROL_PROTOCOL_VERSION,
            negotiateHello(
                peerSupportedVersions = listOf(CONTROL_PROTOCOL_VERSION.toLong()),
                peerCapabilities = emptyList(),
                peerDisplayName = boundaryName,
                localCapabilities = emptyList(),
            ).version,
        )
    }

    companion object {
        const val CONTROL_PROTOCOL_VERSION = 1
        const val MAX_CONTROL_CAPABILITIES = 16
        const val MAX_CONTROL_SUPPORTED_VERSIONS = 8
        const val MAX_CONTROL_DISPLAY_NAME_BYTES = 128

        const val CAPABILITY_FRAME_DURATION_10_MS = 1
        const val CAPABILITY_FRAME_DURATION_20_MS = 2
        const val CAPABILITY_MANUAL_ADDRESS = 3
        const val CAPABILITY_NETWORK_DIAGNOSTICS = 4

        // Canonical known-capability order from sonimesh-protocol negotiation.rs.
        val KNOWN_CAPABILITIES = listOf(
            CAPABILITY_FRAME_DURATION_10_MS,
            CAPABILITY_FRAME_DURATION_20_MS,
            CAPABILITY_MANUAL_ADDRESS,
            CAPABILITY_NETWORK_DIAGNOSTICS,
        )
    }
}
