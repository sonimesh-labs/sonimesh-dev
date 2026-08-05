package com.sonimesh.sender.capture

import org.junit.Assert.assertEquals
import org.junit.Test

class CaptureEvidenceTest {
    @Test
    fun silent_evidence_is_published_in_dependency_order() {
        val calls = mutableListOf<String>()
        val evidence = captureEvidence(calls)
        evidence.begin(captureStartedAtMillis = 0)
        calls.clear()

        evidence.recordRead(frames = 1, silent = true, completedAtMillis = 1)

        assertEquals(
            listOf("capturedFrames.add", "silentFrames.add", "silentReads.increment"),
            calls,
        )
    }

    @Test
    fun dependent_evidence_is_read_in_reverse_publication_order() {
        val calls = mutableListOf<String>()
        val evidence = captureEvidence(calls)
        evidence.begin(captureStartedAtMillis = 0)
        calls.clear()

        evidence.snapshot(nowMillis = 1)

        assertEquals(
            listOf("silentReads.get", "silentFrames.get", "capturedFrames.get", "readErrors.get"),
            calls,
        )
    }

    private fun captureEvidence(calls: MutableList<String>): CaptureEvidence = CaptureEvidence(
        capturedFrames = RecordingCounter("capturedFrames", calls),
        silentFrames = RecordingCounter("silentFrames", calls),
        silentReads = RecordingCounter("silentReads", calls),
        readErrors = RecordingCounter("readErrors", calls),
    )

    private class RecordingCounter(
        private val name: String,
        private val calls: MutableList<String>,
    ) : CaptureEvidenceCounter {
        private var value = 0L

        override fun add(delta: Int) {
            calls += "$name.add"
            value += delta
        }

        override fun increment() {
            calls += "$name.increment"
            value += 1
        }

        override fun get(): Long {
            calls += "$name.get"
            return value
        }

        override fun reset() {
            calls += "$name.reset"
            value = 0
        }
    }
}
