package com.sonimesh.sender.protocol

/**
 * Host-JVM mirror of `sonimesh-protocol`'s `negotiate_hello` and hello
 * validation bounds, shared by the wire-format and negotiation vector tests so
 * both classes pin the same canonical capability intersection.
 */
object HelloNegotiation {
    data class NegotiatedControl(
        val version: Int,
        val capabilities: List<Int>,
    )

    fun validateHello(
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

    fun negotiateHello(
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
