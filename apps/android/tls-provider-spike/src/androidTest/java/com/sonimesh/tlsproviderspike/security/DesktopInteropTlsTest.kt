package com.sonimesh.tlsproviderspike.security

import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.filters.SdkSuppress
import androidx.test.platform.app.InstrumentationRegistry
import java.io.DataInputStream
import java.io.DataOutputStream
import java.net.InetAddress
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
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Cross-platform interoperability measurement for ADR 0021 (Android side).
 *
 * Connects an explicit Conscrypt TLS 1.3 client to the desktop Rustls
 * interop server over a configured TCP route, pins the desktop certificate's DER
 * SPKI supplied as an instrumentation argument, and sends its 32-byte
 * role-separated pairing proofs derived from each side's local exporter.
 * The fixed lab secret is public test data, never a real pairing secret.
 *
 * Required instrumentation arguments:
 * - `desktopHost` — the desktop PC's WLAN address
 * - `desktopPort` — the port the interop server printed
 * - `spkiHex` — the SPKI_HEX value the server printed
 */
@RunWith(AndroidJUnit4::class)
@SdkSuppress(minSdkVersion = 31)
class DesktopInteropTlsTest {
    @Test
    fun proof_derivation_matches_published_client_vector() {
        val pairingId = ByteArray(16) { it.toByte() }
        val secret = ByteArray(32) { (it + 0x10).toByte() }
        val exporter = ByteArray(32) { (it + 0xA0).toByte() }
        val expected = "C274DFE72135AE558034CA1E8A27A4EAB7AAC9B87A0CD93C17BF17126E27F4F2"
            .chunked(2).map { it.toInt(16).toByte() }.toByteArray()
        val prk = hmac(secret, exporter)
        assertTrue(MessageDigest.isEqual(expected, proof(prk, 0, pairingId)))
    }

    @Test
    fun conscrypt_client_exports_matching_bytes_against_desktop_rustls() {
        val arguments = InstrumentationRegistry.getArguments()
        assumeTrue(
            "desktop interop probe requires explicit arguments",
            listOf("desktopHost", "desktopPort", "spkiHex").all { arguments.containsKey(it) },
        )
        val host = requireNotNull(arguments.getString("desktopHost")) { "desktopHost required" }
        val port = requireNotNull(arguments.getString("desktopPort")) { "desktopPort required" }.toInt()
        val spkiHex = requireNotNull(arguments.getString("spkiHex")) { "spkiHex required" }
        require(spkiHex.length in 64..1024 && spkiHex.length % 2 == 0)
        require(spkiHex.all { it in '0'..'9' || it in 'a'..'f' || it in 'A'..'F' })
        val expectedSpki = spkiHex.chunked(2).map { it.toInt(16).toByte() }.toByteArray()
        // A DER SPKI opens with a SEQUENCE tag; the exact length varies by
        // key algorithm and is not asserted.
        assertEquals(0x30, expectedSpki[0].toInt() and 0xFF)

        val context = SSLContext.getInstance("TLS", Conscrypt.newProvider()).apply {
            init(null, arrayOf(ExactSpkiTrustManager(expectedSpki)), null)
        }

        // Signal readiness on the raw socket first (the server expects one
        // marker byte), then run TLS 1.3 over the same stream.
        Socket().use { raw ->
            raw.connect(InetSocketAddress(InetAddress.getByName(host), port), CONNECT_TIMEOUT_MS)
            raw.soTimeout = SOCKET_TIMEOUT_MS
            DataOutputStream(raw.getOutputStream()).write(CLIENT_READY)

            val tls = context.socketFactory.createSocket(raw, host, port, true) as SSLSocket
            tls.enabledProtocols = arrayOf(TLS_13)
            tls.startHandshake()
            assertEquals(TLS_13, tls.session.protocol)

            val exporter = requireNotNull(
                Conscrypt.exportKeyingMaterial(tls, EXPORTER_LABEL, EXPORTER_CONTEXT, EXPORTER_BYTES),
            )
            assertEquals(EXPORTER_BYTES, exporter.size)

            try {
                val prk = hmac(LAB_PAIRING_SECRET, exporter)
                try {
                    val clientProof = proof(prk, 0)
                    val expectedServerProof = proof(prk, 1)
                    val serverProof = ByteArray(EXPORTER_BYTES)
                    try {
                        DataOutputStream(tls.getOutputStream()).apply {
                            write(clientProof)
                            flush()
                        }
                        DataInputStream(tls.getInputStream()).readFully(serverProof)
                        assertTrue(
                            "desktop proof rejected",
                            MessageDigest.isEqual(expectedServerProof, serverProof),
                        )
                    } finally {
                        clientProof.fill(0)
                        expectedServerProof.fill(0)
                        serverProof.fill(0)
                    }
                } finally {
                    prk.fill(0)
                }
            } finally {
                exporter.fill(0)
            }
            Log.i(TAG, "handshake ok protocol=${tls.session.protocol}, desktop proof verified")
            tls.close()
        }
        // The desktop process exits nonzero when the bytes differ; a passing
        // instrumentation run plus a passing server exit is the evidence.
    }

    private fun proof(prk: ByteArray, role: Byte, pairingId: ByteArray = EXPORTER_CONTEXT): ByteArray = hmac(
        prk,
        "sonimesh pairing proof v1".encodeToByteArray() + byteArrayOf(1, role) + pairingId + byteArrayOf(1),
    )

    private fun hmac(key: ByteArray, data: ByteArray): ByteArray = Mac.getInstance("HmacSHA256").run {
        init(SecretKeySpec(key, "HmacSHA256"))
        doFinal(data)
    }

    private class ExactSpkiTrustManager(expectedSpki: ByteArray) : X509TrustManager {
        private val expectedSpki = expectedSpki.copyOf()

        override fun checkClientTrusted(chain: Array<X509Certificate>, authType: String) {
            throw CertificateException("client authentication is not used by this probe")
        }

        override fun checkServerTrusted(chain: Array<X509Certificate>, authType: String) {
            if (chain.size != 1 || !MessageDigest.isEqual(chain[0].publicKey.encoded, expectedSpki)) {
                throw CertificateException("server SPKI did not match the supplied pin")
            }
        }

        override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
    }

    private companion object {
        const val TAG = "DesktopInteropTls"
        const val TLS_13 = "TLSv1.3"
        const val CLIENT_READY = 1
        const val EXPORTER_LABEL = "EXPORTER-SoniMesh Pairing v1"
        const val EXPORTER_BYTES = 32
        const val CONNECT_TIMEOUT_MS = 10_000
        const val SOCKET_TIMEOUT_MS = 30_000
        val LAB_PAIRING_SECRET = ByteArray(32) { 0x42 }
        val EXPORTER_CONTEXT = byteArrayOf(
            0x11, 0x22, 0x33, 0x44,
            0x55.toByte(), 0x66.toByte(), 0x77.toByte(), 0x88.toByte(),
            0x99.toByte(), 0xAA.toByte(), 0xBB.toByte(), 0xCC.toByte(),
            0xDD.toByte(), 0xEE.toByte(), 0xFF.toByte(), 0x00,
        )
    }
}
