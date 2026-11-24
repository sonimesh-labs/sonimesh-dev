package com.sonimesh.opusspike

import java.io.IOException
import java.net.ConnectException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Assert.assertThrows
import org.junit.Test

class PairedConnectTest {
    @Test fun retriesRefusalAndReturnsTheConnection() {
        var attempts = 0
        var waited = 0L
        var notices = 0
        val connection = Any()
        val result = connectPairedWithRetry({ true }, { waited += it }, { notices++ }) {
            if (++attempts < 3) throw ConnectException("refused")
            connection
        }
        assertSame(connection, result)
        assertEquals(3, attempts)
        assertEquals(2_000L, waited)
        assertEquals(2, notices)
    }

    @Test fun refusalHasAFixedLimitAndOtherFailuresAreNotRetried() {
        var attempts = 0
        var waited = 0L
        assertThrows(ConnectException::class.java) {
            connectPairedWithRetry({ true }, { waited += it }) { attempts++; throw ConnectException() }
        }
        assertEquals(6, attempts)
        assertEquals(5_000L, waited)
        attempts = 0
        assertThrows(IOException::class.java) {
            connectPairedWithRetry({ true }, {}) { attempts++; throw IOException() }
        }
        assertEquals(1, attempts)
    }

    @Test fun stoppingDuringWaitPreventsAnotherAttempt() {
        var running = true
        var attempts = 0
        assertThrows(IllegalStateException::class.java) {
            connectPairedWithRetry({ running }, { running = false }) { attempts++; throw ConnectException() }
        }
        assertEquals(1, attempts)
    }
}
