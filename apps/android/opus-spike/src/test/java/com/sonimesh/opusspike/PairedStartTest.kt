package com.sonimesh.opusspike

import com.sonimesh.sender.pairing.StreamControlResult
import com.sonimesh.sender.pairing.StreamIdentity
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class PairedStartTest {
    private val success = StreamControlResult.Status(StreamIdentity(1, 1, 0), 2, "")
    private val retryable = StreamControlResult.Rejected(5, true, "")

    @Test fun retriesAndThenReturnsSuccess() {
        var calls = 0
        var waited = 0L
        val result = startPairedWithRetry({ true }, { waited += it }) {
            if (++calls == 2) success else retryable
        }
        assertEquals(success, result)
        assertEquals(2, calls)
        assertEquals(500L, waited)
    }

    @Test fun limitsRetriesAndDoesNotRetryPermanentRejection() {
        var calls = 0
        assertEquals(retryable, startPairedWithRetry({ true }, {}) { calls++; retryable })
        assertEquals(3, calls)
        calls = 0
        val permanent = StreamControlResult.Rejected(1, false, "")
        assertEquals(permanent, startPairedWithRetry({ true }, {}) { calls++; permanent })
        assertEquals(1, calls)
    }

    @Test fun cancellationDuringWaitPreventsAnotherRequest() {
        var running = true
        var calls = 0
        assertThrows(IllegalStateException::class.java) {
            startPairedWithRetry({ running }, { running = false }) { calls++; retryable }
        }
        assertEquals(1, calls)
    }
}
