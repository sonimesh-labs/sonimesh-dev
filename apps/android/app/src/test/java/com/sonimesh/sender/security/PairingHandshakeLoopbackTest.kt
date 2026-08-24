package com.sonimesh.sender.security

import com.sonimesh.sender.protocol.PairingProofFrame
import com.sonimesh.sender.protocol.PairingQr
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.InputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/**
 * Host-JVM loopback proof of the Android handshake over a real socket pair.
 *
 * This mirrors the Rust `authenticated_control_transport` integration test:
 * the ADR 0019 proof frames travel over a real TCP connection (not in-memory
 * buffers), the desktop-role peer verifies the client frame at the wire
 * boundary and answers with its server frame, the Android handshake verifies
 * that reply before releasing its media root, and the two transcript media
 * roots seal one byte-identical AEAD datagram.
 *
 * It proves the Kotlin composition survives a real channel boundary. It
 * performs no TLS, uses a fixed non-secret exporter, runs both roles in one
 * process over loopback, and claims no device or provider behavior.
 */
class PairingHandshakeLoopbackTest {
    private val invitationId = hex("5a5b5c5d5e5f60616263646566676869")
    private val pairingSecret = ByteArray(32) { 0xA7.toByte() }
    private val tlsExporter = ByteArray(32) { 0xC3.toByte() }

    @Test
    fun frame_exchange_over_loopback_yields_matching_media_roots() {
        val listener = ServerSocket(0, 1, InetAddress.getLoopbackAddress())
        val executor = Executors.newSingleThreadExecutor()

        // The desktop role accepts, verifies the client frame at its boundary,
        // rejects a reflected frame first, then answers with its own frame.
        val peerSealed = executor.submit<ByteArray> {
            listener.accept().use { socket ->
                runDesktopRole(socket)
            }
        }

        // The Android role connects, sends its canonical client frame,
        // verifies the desktop reply through the frame boundary, and only
        // then releases its media root.
        Socket(InetAddress.getLoopbackAddress(), listener.localPort).use { socket ->
            val handshake = beginHandshake()

            assertFalse(
                "an unverified handshake must not send a reply frame yet",
                handshake.verifyDesktopFrame(ByteArray(PairingProofFrame.FRAME_BYTES)),
            )

            val clientFrame = ByteArray(PairingProofFrame.FRAME_BYTES)
            handshake.localFrame(clientFrame)
            socket.getOutputStream().write(clientFrame)

            val reply = readExactly(socket.getInputStream(), PairingProofFrame.FRAME_BYTES)
            assertTrue(handshake.verifyDesktopFrame(reply))
            assertFalse(
                "a truncated reply must fail after the verified one",
                handshake.verifyDesktopFrame(reply.copyOf(reply.size - 1)),
            )
            assertEquals(32, handshake.verifiedMediaRoot().size)

            val context = MediaKeyContext(
                sessionId = 7L,
                streamId = 9,
                keyEpoch = 0,
                direction = MediaDirection.ANDROID_TO_DESKTOP,
            )
            MediaCrypto.sealDatagram(
                sessionSecret = handshake.verifiedMediaRoot(),
                context = context,
                flags = 0.toByte(),
                sampleTimestamp = 1_000L,
                sequence = 0,
                plaintext = ByteArray(16) { 0x42 },
            )
        }

        val desktopSealed = peerSealed.get(10, TimeUnit.SECONDS)
        executor.shutdownNow()
        listener.close()

        // Both transcript derivations seal byte-identical datagrams.
        assertArrayEquals(localSealedForComparison(), desktopSealed)
    }

    /**
     * Desktop role: read the client frame, reject it as a desktop-side proof
     * (wrong role for this endpoint's verification), verify its carried proof
     * against the client role directly, answer with the server frame, and
     * seal one datagram under this side's derived root.
     */
    private fun runDesktopRole(socket: Socket): ByteArray {
        val binding = desktopBinding()
        val input = socket.getInputStream()
        val clientFrameBytes = readExactly(input, PairingProofFrame.FRAME_BYTES)

        // Boundary checks mirror the Rust wrapper: canonical decode, this
        // invitation's identity, and the opposite (client) role byte.
        val decoded = PairingProofFrame.decode(clientFrameBytes)
        assertArrayEquals(invitationId, decoded.pairingId)
        assertEquals(PairingProofFrame.ROLE_ANDROID_CLIENT, decoded.role)

        // The reflected server-role proof must fail verification.
        val serverProof = binding.localProof(PairingRole.DESKTOP_SERVER)
        assertFalse(binding.verifyPeerProof(PairingRole.DESKTOP_SERVER, serverProof))
        // The carried client proof must verify for the desktop's local role.
        assertTrue(binding.verifyPeerProof(PairingRole.DESKTOP_SERVER, decoded.proof))

        // Answer with the canonical server-role frame.
        val reply = ByteArray(PairingProofFrame.FRAME_BYTES)
        PairingProofFrame.encode(
            protocolVersion = 1,
            role = PairingProofFrame.ROLE_DESKTOP_SERVER,
            pairingId = invitationId,
            proof = serverProof,
            out = reply,
        )
        socket.getOutputStream().write(reply)

        // Seal under this side's independently derived media root.
        return MediaCrypto.sealDatagram(
            sessionSecret = binding.mediaRoot(),
            context = MediaKeyContext(
                sessionId = 7L,
                streamId = 9,
                keyEpoch = 0,
                direction = MediaDirection.ANDROID_TO_DESKTOP,
            ),
            flags = 0.toByte(),
            sampleTimestamp = 1_000L,
            sequence = 0,
            plaintext = ByteArray(16) { 0x42 },
        )
    }

    /** Seals the same fixed datagram from a fresh local Android handshake. */
    private fun localSealedForComparison(): ByteArray {
        val root = beginHandshake().apply {
            val reply = ByteArray(PairingProofFrame.FRAME_BYTES)
            PairingProofFrame.encode(
                protocolVersion = 1,
                role = PairingProofFrame.ROLE_DESKTOP_SERVER,
                pairingId = invitationId,
                proof = desktopBinding().localProof(PairingRole.DESKTOP_SERVER),
                out = reply,
            )
            check(verifyDesktopFrame(reply)) { "comparison handshake must verify" }
        }.verifiedMediaRoot()
        return MediaCrypto.sealDatagram(
            sessionSecret = root,
            context = MediaKeyContext(
                sessionId = 7L,
                streamId = 9,
                keyEpoch = 0,
                direction = MediaDirection.ANDROID_TO_DESKTOP,
            ),
            flags = 0.toByte(),
            sampleTimestamp = 1_000L,
            sequence = 0,
            plaintext = ByteArray(16) { 0x42 },
        )
    }

    private fun desktopBinding(): PairingTlsBinding = PairingTlsBinding(
        protocolVersion = 1,
        pairingId = invitationId,
        pairingSecret = pairingSecret,
        tlsExporter = tlsExporter,
    )

    private fun beginHandshake(): PairingHandshake =
        PairingHandshake.beginFromScannedPayload(
            scannedPayload = canonicalInvitation(),
            expectedPairingId = invitationId,
            tlsExporter = tlsExporter,
        )

    private fun canonicalInvitation(): ByteArray {
        val payload = ByteArray(PairingQr.MAX_PAYLOAD_BYTES)
        val written = PairingQr.encode(
            protocolVersion = 1,
            pairingId = invitationId,
            pairingSecret = pairingSecret,
            controlHost = "localhost",
            controlPort = 47610,
            desktopTlsSpki = ByteArray(64) { 0x40 },
            expiresAtMs = 2_000_000L,
            out = payload,
        )
        return payload.copyOf(written)
    }

    private fun readExactly(input: InputStream, length: Int): ByteArray {
        val buffer = ByteArray(length)
        var filled = 0
        while (filled < length) {
            val read = input.read(buffer, filled, length - filled)
            require(read > 0) { "channel closed before $length bytes arrived" }
            filled += read
        }
        return buffer
    }

    private fun hex(value: String): ByteArray = ByteArray(value.length / 2) { index ->
        value.substring(index * 2, index * 2 + 2).toInt(16).toByte()
    }
}
