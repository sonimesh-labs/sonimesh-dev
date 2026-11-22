package com.sonimesh.sender.pairing

import com.sonimesh.sender.protocol.PairingProofFrame
import com.sonimesh.sender.protocol.PairingQr
import java.io.DataInputStream
import java.net.InetSocketAddress
import java.net.Socket
import java.security.MessageDigest
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLSocket
import javax.net.ssl.X509TrustManager
import org.conscrypt.Conscrypt

/** A verified connection. The caller owns the socket and media root until close. */
class PairedConnection internal constructor(
    val socket: SSLSocket,
    val mediaRoot: ByteArray,
) : AutoCloseable {
    private var helloStarted = false
    @Volatile private var negotiated: NegotiatedHello? = null
    private var nextRequestId = 2L

    /** Exchanges Hello messages after pairing; call off the UI thread. */
    @Synchronized
    fun negotiateHello(displayName: String, capabilities: List<Int>): NegotiatedHello {
        check(!helloStarted) { "hello exchange already started" }
        helloStarted = true
        try {
            socket.soTimeout = 30_000
            val result = exchangeHello(socket.inputStream, socket.outputStream, displayName, capabilities)
            negotiated = result
            socket.soTimeout = 0
            return result
        } catch (error: Throwable) {
            runCatching { close() }
            throw error
        }
    }

    /** Sends a stream command and waits for the desktop's reply; call off the UI thread. */
    @Synchronized
    fun startStream(stream: StreamParameters): StreamControlResult {
        val hello = checkNotNull(negotiated) { "hello has not completed" }
        stream.validate()
        require((if (stream.frameDurationMs == 10) 1 else 2) in hello.capabilities) {
            "frame duration was not negotiated"
        }
        return command(stream.identity, 11) { coded ->
            coded.writeUInt64(1, stream.identity.sessionId)
            coded.writeUInt32(2, stream.identity.streamId.toInt())
            coded.writeUInt32(3, stream.sampleRateHz)
            coded.writeUInt32(4, stream.channels)
            coded.writeUInt32(5, stream.frameDurationMs)
            coded.writeUInt32(6, stream.identity.keyEpoch)
        }
    }

    /** Requests a stop for an exact media identity; call off the UI thread. */
    @Synchronized
    fun stopStream(identity: StreamIdentity): StreamControlResult {
        checkNotNull(negotiated) { "hello has not completed" }
        identity.validate()
        return command(identity, 12) { coded ->
            coded.writeUInt64(1, identity.sessionId)
            coded.writeUInt32(2, identity.streamId.toInt())
            coded.writeUInt32(3, identity.keyEpoch)
        }
    }

    private fun command(
        identity: StreamIdentity,
        field: Int,
        write: (com.google.protobuf.CodedOutputStream) -> Unit,
    ): StreamControlResult {
        check(nextRequestId > 0) { "control request IDs exhausted" }
        val requestId = nextRequestId++
        try {
            socket.soTimeout = 30_000
            return exchangeStreamCommand(socket.inputStream, socket.outputStream, requestId, identity, field, write)
        } catch (error: Throwable) {
            runCatching { close() }
            throw error
        } finally {
            if (!socket.isClosed) socket.soTimeout = 0
        }
    }

    override fun close() {
        negotiated = null
        try {
            socket.close()
        } finally {
            mediaRoot.fill(0)
        }
    }
}

/** Establishes one Android-client pairing exchange from a binary QR payload. */
object PairingClient {
    /** Call off the UI thread. The caller retains ownership of [payload]. */
    fun connect(payload: ByteArray): PairedConnection {
        val invitation = PairingQr.decode(payload)
        val raw = Socket()
        try {
            val context = SSLContext.getInstance("TLS", Conscrypt.newProvider()).apply {
                init(null, arrayOf(ExactSpkiTrustManager(invitation.desktopTlsSpki)), null)
            }
            raw.connect(InetSocketAddress(invitation.controlHost, invitation.controlPort), TIMEOUT_MS)
            raw.soTimeout = TIMEOUT_MS
            val socket = context.socketFactory.createSocket(
                raw, invitation.controlHost, invitation.controlPort, true,
            ) as SSLSocket
            socket.enabledProtocols = arrayOf("TLSv1.3")
            socket.startHandshake()
            check(socket.session.protocol == "TLSv1.3") { "TLS 1.3 required" }
            val exporter = requireNotNull(
                Conscrypt.exportKeyingMaterial(socket, EXPORTER_LABEL, invitation.pairingId, 32),
            )
            try {
                check(exporter.size == 32) { "invalid TLS exporter length" }
                val prk = pairingHmac(invitation.pairingSecret, exporter)
                try {
                    val localProof = pairingDerive(prk, PROOF_LABEL, invitation.pairingId, 0)
                    val expectedProof = pairingDerive(prk, PROOF_LABEL, invitation.pairingId, 1)
                    try {
                        val request = ByteArray(PairingProofFrame.FRAME_BYTES)
                        PairingProofFrame.encode(
                            1, PairingProofFrame.ROLE_ANDROID_CLIENT, invitation.pairingId, localProof, request,
                        )
                        socket.outputStream.write(request)
                        socket.outputStream.flush()
                        val response = ByteArray(PairingProofFrame.FRAME_BYTES)
                        DataInputStream(socket.inputStream).readFully(response)
                        check(verifyDesktopFrame(response, invitation.pairingId, expectedProof)) {
                            "desktop proof rejected"
                        }
                        socket.soTimeout = 0
                        val root = pairingDerive(prk, ROOT_LABEL, invitation.pairingId, null)
                        return PairedConnection(socket, root)
                    } finally {
                        localProof.fill(0)
                        expectedProof.fill(0)
                    }
                } finally {
                    prk.fill(0)
                }
            } finally {
                exporter.fill(0)
            }
        } catch (error: Throwable) {
            runCatching { raw.close() }
            throw error
        } finally {
            invitation.pairingSecret.fill(0)
        }
    }

    private class ExactSpkiTrustManager(private val pin: ByteArray) : X509TrustManager {
        override fun checkClientTrusted(chain: Array<X509Certificate>, authType: String) {
            throw CertificateException("client authentication is not used")
        }
        override fun checkServerTrusted(chain: Array<X509Certificate>, authType: String) {
            if (chain.size != 1 || !MessageDigest.isEqual(chain[0].publicKey.encoded, pin)) {
                throw CertificateException("desktop SPKI mismatch")
            }
        }
        override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
    }

    private const val TIMEOUT_MS = 30_000
    private const val EXPORTER_LABEL = "EXPORTER-SoniMesh Pairing v1"
    private val PROOF_LABEL = "sonimesh pairing proof v1".encodeToByteArray()
    private val ROOT_LABEL = "sonimesh pairing media root v1".encodeToByteArray()
}

internal fun pairingDerive(prk: ByteArray, label: ByteArray, id: ByteArray, role: Byte?): ByteArray {
    val suffix = if (role == null) byteArrayOf(1) else byteArrayOf(1, role)
    return pairingHmac(prk, label + suffix + id + byteArrayOf(1))
}

internal fun pairingHmac(key: ByteArray, input: ByteArray): ByteArray = Mac.getInstance("HmacSHA256").run {
    init(SecretKeySpec(key, "HmacSHA256"))
    doFinal(input)
}

internal fun verifyDesktopFrame(wire: ByteArray, id: ByteArray, expectedProof: ByteArray): Boolean {
    val frame = try {
        PairingProofFrame.decode(wire)
    } catch (_: IllegalArgumentException) {
        return false
    }
    return frame.role == PairingProofFrame.ROLE_DESKTOP_SERVER &&
        MessageDigest.isEqual(frame.pairingId, id) &&
        MessageDigest.isEqual(frame.proof, expectedProof)
}
