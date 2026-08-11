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
    private fun negotiateHello(
        peerSupportedVersions: List<Long>,
        peerCapabilities: List<Long>,
        peerDisplayName: String,
        localCapabilities: List<Int>,
    ): HelloNegotiation.NegotiatedControl =
        HelloNegotiation.negotiateHello(
            peerSupportedVersions,
            peerCapabilities,
            peerDisplayName,
            localCapabilities,
        )

    @Test
    fun selects_current_version_and_canonical_common_capabilities() {
        val negotiated = negotiateHello(
            peerSupportedVersions = listOf(
                99,
                HelloNegotiation.CONTROL_PROTOCOL_VERSION.toLong(),
                HelloNegotiation.CONTROL_PROTOCOL_VERSION.toLong(),
            ),
            peerCapabilities = listOf(
                HelloNegotiation.CAPABILITY_NETWORK_DIAGNOSTICS.toLong(),
                HelloNegotiation.CAPABILITY_FRAME_DURATION_20_MS.toLong(),
                HelloNegotiation.CAPABILITY_FRAME_DURATION_20_MS.toLong(),
                9_999,
            ),
            peerDisplayName = "Lab sender",
            localCapabilities = listOf(
                HelloNegotiation.CAPABILITY_NETWORK_DIAGNOSTICS,
                HelloNegotiation.CAPABILITY_FRAME_DURATION_20_MS,
                HelloNegotiation.CAPABILITY_MANUAL_ADDRESS,
            ),
        )

        assertEquals(HelloNegotiation.CONTROL_PROTOCOL_VERSION, negotiated.version)
        assertEquals(
            listOf(
                HelloNegotiation.CAPABILITY_FRAME_DURATION_20_MS,
                HelloNegotiation.CAPABILITY_NETWORK_DIAGNOSTICS,
            ),
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

        val oversizedPolicy =
            List(HelloNegotiation.MAX_CONTROL_CAPABILITIES + 1) {
                HelloNegotiation.CAPABILITY_MANUAL_ADDRESS
            }
        assertThrows(IllegalArgumentException::class.java) {
            negotiateHello(
                peerSupportedVersions = listOf(HelloNegotiation.CONTROL_PROTOCOL_VERSION.toLong()),
                peerCapabilities = emptyList(),
                peerDisplayName = "Lab sender",
                localCapabilities = oversizedPolicy,
            )
        }
    }

    @Test
    fun rejects_invalid_peer_hello_before_selecting_features() {
        val oversizedPeer =
            List(HelloNegotiation.MAX_CONTROL_CAPABILITIES + 1) {
                HelloNegotiation.CAPABILITY_MANUAL_ADDRESS.toLong()
            }
        assertThrows(IllegalArgumentException::class.java) {
            negotiateHello(
                peerSupportedVersions = listOf(HelloNegotiation.CONTROL_PROTOCOL_VERSION.toLong()),
                peerCapabilities = oversizedPeer,
                peerDisplayName = "Lab sender",
                localCapabilities = listOf(HelloNegotiation.CAPABILITY_MANUAL_ADDRESS),
            )
        }
    }

    @Test
    fun rejects_oversized_version_and_display_name_hello_bounds() {
        val tooManyVersions = List(HelloNegotiation.MAX_CONTROL_SUPPORTED_VERSIONS + 1) {
            it.toLong()
        }
        assertThrows(IllegalArgumentException::class.java) {
            negotiateHello(
                peerSupportedVersions = tooManyVersions,
                peerCapabilities = emptyList(),
                peerDisplayName = "Lab sender",
                localCapabilities = emptyList(),
            )
        }

        val oversizedName = "a".repeat(HelloNegotiation.MAX_CONTROL_DISPLAY_NAME_BYTES + 1)
        assertThrows(IllegalArgumentException::class.java) {
            negotiateHello(
                peerSupportedVersions = listOf(HelloNegotiation.CONTROL_PROTOCOL_VERSION.toLong()),
                peerCapabilities = emptyList(),
                peerDisplayName = oversizedName,
                localCapabilities = emptyList(),
            )
        }

        val boundaryName = "a".repeat(HelloNegotiation.MAX_CONTROL_DISPLAY_NAME_BYTES)
        assertEquals(
            HelloNegotiation.CONTROL_PROTOCOL_VERSION,
            negotiateHello(
                peerSupportedVersions = listOf(HelloNegotiation.CONTROL_PROTOCOL_VERSION.toLong()),
                peerCapabilities = emptyList(),
                peerDisplayName = boundaryName,
                localCapabilities = emptyList(),
            ).version,
        )
    }
}
