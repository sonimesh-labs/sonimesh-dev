package com.sonimesh.tlsproviderspike.security

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.Signature
import java.util.UUID
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import androidx.test.ext.junit.runners.AndroidJUnit4

/**
 * Device-only AndroidKeyStore feasibility test for ADR 0008.
 *
 * It uses fresh aliases and fixed non-secret data, then removes the key before
 * returning. This does not provide an Android identity or pairing API.
 */
@RunWith(AndroidJUnit4::class)
class AndroidKeystoreSpikeTest {
    @Test
    fun generated_signing_key_is_non_exportable_and_deleted() {
        val alias = "sonimesh.keystore-spike.${UUID.randomUUID()}"
        val keyStore = KeyStore.getInstance(ANDROID_KEY_STORE).apply { load(null) }

        try {
            val generator = KeyPairGenerator.getInstance(
                KeyProperties.KEY_ALGORITHM_EC,
                ANDROID_KEY_STORE,
            )
            generator.initialize(
                KeyGenParameterSpec.Builder(
                    alias,
                    KeyProperties.PURPOSE_SIGN or KeyProperties.PURPOSE_VERIFY,
                )
                    .setDigests(KeyProperties.DIGEST_SHA256)
                    .build(),
            )
            val pair = generator.generateKeyPair()

            val signed = Signature.getInstance(SIGNATURE_ALGORITHM).run {
                initSign(pair.private)
                update(TEST_MESSAGE)
                sign()
            }
            val verified = Signature.getInstance(SIGNATURE_ALGORITHM).run {
                initVerify(pair.public)
                update(TEST_MESSAGE)
                verify(signed)
            }

            assertTrue(verified)
            assertNull(pair.private.encoded)
            assertTrue(keyStore.containsAlias(alias))
        } finally {
            if (keyStore.containsAlias(alias)) {
                keyStore.deleteEntry(alias)
            }
        }

        assertFalse(keyStore.containsAlias(alias))
    }

    private companion object {
        const val ANDROID_KEY_STORE = "AndroidKeyStore"
        const val SIGNATURE_ALGORITHM = "SHA256withECDSA"
        val TEST_MESSAGE = byteArrayOf(
            0x53,
            0x6f,
            0x6e,
            0x69,
            0x4d,
            0x65,
            0x73,
            0x68,
            0x20,
            0x4b,
            0x65,
            0x79,
            0x73,
            0x74,
            0x6f,
            0x72,
            0x65,
            0x20,
            0x53,
            0x70,
            0x69,
            0x6b,
            0x65,
            0x20,
            0x76,
            0x31,
        )
    }
}
