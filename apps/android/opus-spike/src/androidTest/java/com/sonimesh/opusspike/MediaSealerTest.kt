package com.sonimesh.opusspike

import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertThrows
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class MediaSealerTest {
    @Test
    fun sealsPublishedVectorOnAndroid() {
        val secret = ByteArray(32) { it.toByte() }
        val sealer = LabMediaSealer(secret, 0x0102030405060708L, 0x0a0b0c0d, 0x0e0f)
        val expected = hex("534e4d530102002401020304050607080a0b0c0d0000000000000000000003c0" +
            "00250e0f7cce62ea960fc27cf5a6ffce2aa59343a5f12997791396ca10bdfc7c" +
            "c060c9dfe49fd67131")
        try {
            assertArrayEquals(expected, sealer.seal(0, 960, 2, "SoniMesh media vector".toByteArray()))
        } finally {
            sealer.clear()
        }
    }

    @Test
    fun rejectsInvalidSequenceAndPacketSize() {
        val sealer = LabMediaSealer(ByteArray(32), 1, 1, 0)
        try {
            assertThrows(IllegalArgumentException::class.java) { sealer.seal(-1, 0, 0, byteArrayOf(1)) }
            assertThrows(IllegalArgumentException::class.java) { sealer.seal(0x1_0000_0000, 0, 0, byteArrayOf(1)) }
            assertThrows(IllegalArgumentException::class.java) { sealer.seal(0, 0, 0, ByteArray(1_149)) }
        } finally {
            sealer.clear()
        }
    }

    private fun hex(value: String) = ByteArray(value.length / 2) { index ->
        value.substring(index * 2, index * 2 + 2).toInt(16).toByte()
    }
}
