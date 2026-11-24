package com.sonimesh.opusspike

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class LabMediaPortTest {
    @Test
    fun acceptsOnlyValidDecimalPorts() {
        for ((text, expected) in listOf("1" to 1, "00001" to 1, "39093" to 39093, "65535" to 65535)) {
            assertEquals(expected, parseLabMediaPort(text))
        }
        for (text in listOf("", "0", "00000", "000001", "65536", "999999", "-1", "+1", " 1", "1.5", "１２")) {
            assertNull(text, parseLabMediaPort(text))
        }
    }
}
