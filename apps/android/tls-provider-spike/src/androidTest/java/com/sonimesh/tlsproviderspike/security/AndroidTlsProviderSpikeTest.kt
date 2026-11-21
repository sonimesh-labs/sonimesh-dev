package com.sonimesh.tlsproviderspike.security

import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.filters.SdkSuppress
import java.math.BigInteger
import java.net.InetAddress
import java.net.Socket
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.MessageDigest
import java.security.Principal
import java.security.PrivateKey
import java.security.SecureRandom
import java.security.Security
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import java.util.Date
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference
import javax.net.ssl.KeyManagerFactory
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLEngine
import javax.net.ssl.SSLServerSocket
import javax.net.ssl.SSLSocket
import javax.net.ssl.X509ExtendedKeyManager
import javax.net.ssl.X509TrustManager
import javax.security.auth.x500.X500Principal
import org.conscrypt.Conscrypt
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Device-only TLS exporter feasibility test for ADR 0008.
 *
 * This deliberately exercises only AndroidKeyStore, an explicit Conscrypt
 * context, and loopback TLS. It does not provide a pairing or identity API.
 */
@RunWith(AndroidJUnit4::class)
@SdkSuppress(minSdkVersion = Build.VERSION_CODES.S)
class AndroidTlsProviderSpikeTest {
    @Test
    fun explicit_conscrypt_context_does_not_install_a_global_provider() {
        val before = Security.getProviders().map { provider -> provider.name }

        val provider = Conscrypt.newProvider()
        SSLContext.getInstance("TLS", provider)

        val after = Security.getProviders().map { candidate -> candidate.name }
        assertEquals(before, after)
    }

    @Test
    fun pinned_tls13_exporter_matches_and_altered_pin_is_rejected() {
        val alias = "sonimesh.tls-provider-spike.${UUID.randomUUID()}"
        val keyStore = KeyStore.getInstance(ANDROID_KEY_STORE).apply { load(null) }

        try {
            generateServerKey(alias)
            val certificate = keyStore.getCertificate(alias) as X509Certificate
            val expectedSpki = certificate.publicKey.encoded.copyOf()
            val serverContext = createServerContext(keyStore, alias)

            val positive = runLoopbackProbe(serverContext, expectedSpki)
            val clientExporter = requireNotNull(positive.clientExporter)
            val serverExporter = requireNotNull(positive.serverExporter)
            assertEquals(TLS_13, positive.clientProtocol)
            assertEquals(TLS_13, positive.serverProtocol)
            assertEquals(EXPORTER_BYTES, clientExporter.size)
            assertEquals(EXPORTER_BYTES, serverExporter.size)
            assertArrayEquals(clientExporter, serverExporter)

            val alteredSpki = expectedSpki.copyOf().also { it[0] = (it[0].toInt() xor 0x01).toByte() }
            val alteredPin = runLoopbackProbe(serverContext, alteredSpki)
            assertTrue(alteredPin.serverAcceptedConnection)
            assertTrue(alteredPin.clientRejectedPin)
        } finally {
            if (keyStore.containsAlias(alias)) {
                keyStore.deleteEntry(alias)
            }
        }

        val refreshedKeyStore = KeyStore.getInstance(ANDROID_KEY_STORE).apply { load(null) }
        assertFalse(refreshedKeyStore.containsAlias(alias))
    }

    private fun generateServerKey(alias: String) {
        val now = System.currentTimeMillis()
        val generator = KeyPairGenerator.getInstance(
            KeyProperties.KEY_ALGORITHM_EC,
            ANDROID_KEY_STORE,
        )
        generator.initialize(
            KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_SIGN or KeyProperties.PURPOSE_VERIFY)
                // Conscrypt passes the TLS CertificateVerify digest to AndroidKeyStore as NONEwithECDSA.
                .setDigests(KeyProperties.DIGEST_NONE, KeyProperties.DIGEST_SHA256)
                .setCertificateSubject(X500Principal("CN=SoniMesh TLS provider spike"))
                .setCertificateSerialNumber(BigInteger.valueOf(now))
                .setCertificateNotBefore(Date(now - CERTIFICATE_CLOCK_SKEW_MS))
                .setCertificateNotAfter(Date(now + CERTIFICATE_LIFETIME_MS))
                .build(),
        )
        generator.generateKeyPair()
    }

    private fun createServerContext(keyStore: KeyStore, alias: String): SSLContext {
        val keyManagerFactory = KeyManagerFactory.getInstance(KeyManagerFactory.getDefaultAlgorithm())
        keyManagerFactory.init(keyStore, null)
        return SSLContext.getInstance("TLS", Conscrypt.newProvider()).apply {
            val keyManager = keyManagerFactory.keyManagers.single { it is X509ExtendedKeyManager }
                as X509ExtendedKeyManager
            init(arrayOf(AliasOnlyKeyManager(keyManager, alias)), null, SecureRandom())
        }
    }

    private fun createClientContext(expectedSpki: ByteArray): SSLContext =
        SSLContext.getInstance("TLS", Conscrypt.newProvider()).apply {
            init(null, arrayOf(ExactSpkiTrustManager(expectedSpki)), SecureRandom())
        }

    private fun runLoopbackProbe(serverContext: SSLContext, expectedSpki: ByteArray): ProbeResult {
        val listener = (
            serverContext.serverSocketFactory.createServerSocket(
                0,
                1,
                InetAddress.getLoopbackAddress(),
            ) as SSLServerSocket
        ).apply {
            enabledProtocols = arrayOf(TLS_13)
            soTimeout = SOCKET_TIMEOUT_MS
        }
        val serverFailure = AtomicReference<Throwable?>()
        val serverExporter = AtomicReference<ByteArray?>()
        val serverProtocol = AtomicReference<String?>()
        val acceptedConnection = AtomicBoolean(false)
        val acceptedSocket = AtomicReference<SSLSocket?>()
        val serverHandshakeComplete = CountDownLatch(1)
        val serverThread = Thread {
            try {
                (listener.accept() as SSLSocket).use { socket ->
                    acceptedConnection.set(true)
                    acceptedSocket.set(socket)
                    socket.enabledProtocols = arrayOf(TLS_13)
                    socket.soTimeout = SOCKET_TIMEOUT_MS
                    socket.startHandshake()
                    serverHandshakeComplete.countDown()
                    serverProtocol.set(socket.session.protocol)
                    serverExporter.set(export(socket))
                }
            } catch (failure: Throwable) {
                serverFailure.set(failure)
            } finally {
                acceptedSocket.set(null)
            }
        }

        var clientExporter: ByteArray? = null
        var clientProtocol: String? = null
        var clientRejectedPin = false
        var clientFailure: Throwable? = null
        try {
            serverThread.start()
            try {
                (createClientContext(expectedSpki).socketFactory.createSocket(
                    InetAddress.getLoopbackAddress(),
                    listener.localPort,
                ) as SSLSocket).use { socket ->
                    socket.enabledProtocols = arrayOf(TLS_13)
                    socket.soTimeout = SOCKET_TIMEOUT_MS
                    socket.startHandshake()
                    clientProtocol = socket.session.protocol
                    clientExporter = export(socket)
                    assertTrue(
                        "server TLS handshake did not complete",
                        serverHandshakeComplete.await(THREAD_JOIN_TIMEOUT_MS, TimeUnit.MILLISECONDS),
                    )
                }
            } catch (failure: javax.net.ssl.SSLException) {
                if (containsPinMismatch(failure)) {
                    clientRejectedPin = true
                } else {
                    clientFailure = failure
                }
            }
        } finally {
            listener.close()
            serverThread.join(THREAD_JOIN_TIMEOUT_MS)
            if (serverThread.isAlive) {
                acceptedSocket.get()?.close()
                serverThread.join(THREAD_JOIN_TIMEOUT_MS)
            }
        }

        assertFalse("server thread did not finish", serverThread.isAlive)
        return if (clientRejectedPin) {
            ProbeResult(
                clientProtocol = null,
                serverProtocol = serverProtocol.get(),
                clientExporter = null,
                serverExporter = serverExporter.get(),
                clientRejectedPin = true,
                serverAcceptedConnection = acceptedConnection.get(),
            )
        } else {
            serverFailure.get()?.let { throw AssertionError("loopback TLS server failed", it) }
            clientFailure?.let { throw it }
            ProbeResult(
                clientProtocol = requireNotNull(clientProtocol),
                serverProtocol = requireNotNull(serverProtocol.get()),
                clientExporter = requireNotNull(clientExporter),
                serverExporter = requireNotNull(serverExporter.get()),
                clientRejectedPin = false,
                serverAcceptedConnection = acceptedConnection.get(),
            )
        }
    }

    private fun export(socket: SSLSocket): ByteArray =
        requireNotNull(Conscrypt.exportKeyingMaterial(socket, EXPORTER_LABEL, EXPORTER_CONTEXT, EXPORTER_BYTES))

    private fun containsPinMismatch(failure: Throwable): Boolean {
        var current: Throwable? = failure
        while (current != null) {
            if (current is SpkiPinMismatchException) {
                return true
            }
            current = current.cause
        }
        return false
    }

    private data class ProbeResult(
        val clientProtocol: String?,
        val serverProtocol: String?,
        val clientExporter: ByteArray?,
        val serverExporter: ByteArray?,
        val clientRejectedPin: Boolean,
        val serverAcceptedConnection: Boolean,
    )

    private class ExactSpkiTrustManager(expectedSpki: ByteArray) : X509TrustManager {
        private val expectedSpki = expectedSpki.copyOf()

        override fun checkClientTrusted(chain: Array<X509Certificate>, authType: String) {
            throw CertificateException("client authentication is not used by this probe")
        }

        override fun checkServerTrusted(chain: Array<X509Certificate>, authType: String) {
            if (chain.size != 1 || !MessageDigest.isEqual(chain[0].publicKey.encoded, expectedSpki)) {
                throw SpkiPinMismatchException()
            }
        }

        override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
    }

    private class SpkiPinMismatchException : CertificateException()

    private class AliasOnlyKeyManager(
        private val delegate: X509ExtendedKeyManager,
        private val alias: String,
    ) : X509ExtendedKeyManager() {
        override fun chooseClientAlias(
            keyType: Array<String>,
            issuers: Array<Principal>?,
            socket: Socket?,
        ): String? = delegate.chooseClientAlias(keyType, issuers, socket)

        override fun chooseEngineClientAlias(
            keyType: Array<String>,
            issuers: Array<Principal>?,
            engine: SSLEngine?,
        ): String? = delegate.chooseEngineClientAlias(keyType, issuers, engine)

        override fun chooseEngineServerAlias(
            keyType: String,
            issuers: Array<Principal>?,
            engine: SSLEngine?,
        ): String = alias

        override fun chooseServerAlias(
            keyType: String,
            issuers: Array<Principal>?,
            socket: Socket?,
        ): String = alias

        override fun getCertificateChain(requestedAlias: String): Array<X509Certificate>? =
            delegate.getCertificateChain(requestedAlias)

        override fun getClientAliases(
            keyType: String,
            issuers: Array<Principal>?,
        ): Array<String>? = delegate.getClientAliases(keyType, issuers)

        override fun getPrivateKey(requestedAlias: String): PrivateKey? = delegate.getPrivateKey(requestedAlias)

        override fun getServerAliases(
            keyType: String,
            issuers: Array<Principal>?,
        ): Array<String> = arrayOf(alias)
    }

    private companion object {
        const val ANDROID_KEY_STORE = "AndroidKeyStore"
        const val TLS_13 = "TLSv1.3"
        const val EXPORTER_LABEL = "EXPORTER-SoniMesh Pairing v1"
        const val EXPORTER_BYTES = 32
        const val SOCKET_TIMEOUT_MS = 5_000
        const val THREAD_JOIN_TIMEOUT_MS = 10_000L
        const val CERTIFICATE_CLOCK_SKEW_MS = 60_000L
        const val CERTIFICATE_LIFETIME_MS = 60 * 60 * 1_000L
        val EXPORTER_CONTEXT = byteArrayOf(
            0x00,
            0x01,
            0x02,
            0x03,
            0x04,
            0x05,
            0x06,
            0x07,
            0x08,
            0x09,
            0x0a,
            0x0b,
            0x0c,
            0x0d,
            0x0e,
            0x0f,
        )
    }
}
