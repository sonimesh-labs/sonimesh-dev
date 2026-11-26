package com.sonimesh.opusspike

import org.junit.Assert.assertThrows
import org.junit.Test

class LabOpusDurationTest {
    @Test
    fun acceptsTwentyMillisFromDifferentFrameConfigurations() {
        for (packet in listOf(
            byteArrayOf((1 shl 3).toByte()),
            byteArrayOf((13 shl 3).toByte()),
            byteArrayOf((19 shl 3).toByte()),
            byteArrayOf(((18 shl 3) or 1).toByte()),
            byteArrayOf(((18 shl 3) or 2).toByte()),
            byteArrayOf(((16 shl 3) or 3).toByte(), 8),
            byteArrayOf(((18 shl 3) or 3).toByte(), 2),
        )) {
            requireLabOpusDuration(packet)
        }
    }

    @Test
    fun rejectsDifferentDurationsAndIncompleteOrInvalidCount() {
        for (packet in listOf(
            byteArrayOf(),
            byteArrayOf((18 shl 3).toByte()),
            byteArrayOf((3 shl 3).toByte()),
            byteArrayOf(((19 shl 3) or 1).toByte()),
            byteArrayOf(((18 shl 3) or 3).toByte()),
            byteArrayOf(((18 shl 3) or 3).toByte(), 0),
            byteArrayOf(((18 shl 3) or 3).toByte(), 3),
            byteArrayOf(((19 shl 3) or 3).toByte(), 63),
        )) {
            assertThrows(LabOpusDurationException::class.java) { requireLabOpusDuration(packet) }
        }
    }
}
