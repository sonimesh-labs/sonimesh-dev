package com.sonimesh.sender.capture

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class CaptureTeardownGateTest {
    @Test
    fun completion_waits_for_teardown_and_reader_exit() {
        val gate = CaptureTeardownGate()

        assertFalse(gate.claimFinish())
        assertTrue(gate.launchReader {})
        assertTrue(gate.beginTeardown())
        assertFalse(gate.claimFinish())
        gate.markOutcomeReady()
        assertFalse(gate.claimFinish())

        gate.markReaderExited()
        assertTrue(gate.claimFinish())
    }

    @Test
    fun teardown_and_completion_each_have_one_winner() {
        val gate = CaptureTeardownGate()

        assertTrue(gate.beginTeardown())
        assertFalse(gate.beginTeardown())
        gate.markOutcomeReady()
        assertTrue(gate.claimFinish())
        assertFalse(gate.claimFinish())
    }

    @Test
    fun failed_reader_launch_rolls_back_before_teardown_completion() {
        val gate = CaptureTeardownGate()

        assertThrows(IllegalStateException::class.java) {
            gate.launchReader { throw IllegalStateException("thread start failed") }
        }

        assertTrue(gate.beginTeardown())
        gate.markOutcomeReady()
        assertTrue(gate.claimFinish())
    }

    @Test
    fun teardown_refuses_a_late_reader_launch() {
        val gate = CaptureTeardownGate()
        var startCalled = false

        assertTrue(gate.beginTeardown())
        assertFalse(
            gate.launchReader {
                startCalled = true
            },
        )
        assertFalse(startCalled)
        gate.markOutcomeReady()
        assertTrue(gate.claimFinish())
    }

    @Test
    fun duplicate_reader_launch_is_rejected_without_clearing_the_active_reader() {
        val gate = CaptureTeardownGate()

        assertTrue(gate.launchReader {})
        assertThrows(IllegalStateException::class.java) {
            gate.launchReader {}
        }
        assertTrue(gate.beginTeardown())
        gate.markOutcomeReady()
        assertFalse(gate.claimFinish())

        gate.markReaderExited()
        assertTrue(gate.claimFinish())
    }

    @Test
    fun concurrent_reader_launch_and_teardown_preserve_the_finish_barrier() {
        repeat(100) {
            val gate = CaptureTeardownGate()
            val start = CountDownLatch(1)
            val completed = CountDownLatch(2)
            val launchResult = AtomicReference<Boolean>()
            val finishClaimed = AtomicBoolean()

            Thread {
                start.await()
                launchResult.set(gate.launchReader {})
                completed.countDown()
            }.start()
            Thread {
                start.await()
                gate.beginTeardown()
                gate.markOutcomeReady()
                finishClaimed.set(gate.claimFinish())
                completed.countDown()
            }.start()
            start.countDown()

            assertTrue(completed.await(2, TimeUnit.SECONDS))
            if (launchResult.get()) {
                assertFalse(finishClaimed.get())
                gate.markReaderExited()
                assertTrue(gate.claimFinish())
            } else {
                assertTrue(finishClaimed.get())
            }
        }
    }

    @Test
    fun outcome_publication_requires_teardown_admission() {
        val gate = CaptureTeardownGate()

        assertThrows(IllegalStateException::class.java) {
            gate.markOutcomeReady()
        }
        assertFalse(gate.claimFinish())
    }

    @Test
    fun completion_cannot_be_claimed_before_outcome_publication() {
        val gate = CaptureTeardownGate()

        assertTrue(gate.beginTeardown())
        assertFalse(gate.claimFinish())
        gate.markOutcomeReady()
        assertTrue(gate.claimFinish())
    }

    @Test
    fun concurrent_outcome_publication_and_finish_preserve_one_cleanup_claim() {
        repeat(100) {
            val gate = CaptureTeardownGate()
            val start = CountDownLatch(1)
            val completed = CountDownLatch(2)
            val winners = AtomicInteger()

            assertTrue(gate.beginTeardown())
            Thread {
                start.await()
                gate.markOutcomeReady()
                if (gate.claimFinish()) {
                    winners.incrementAndGet()
                }
                completed.countDown()
            }.start()
            Thread {
                start.await()
                if (gate.claimFinish()) {
                    winners.incrementAndGet()
                }
                completed.countDown()
            }.start()
            start.countDown()

            assertTrue(completed.await(2, TimeUnit.SECONDS))
            if (gate.claimFinish()) {
                winners.incrementAndGet()
            }
            assertEquals(1, winners.get())
        }
    }

    @Test
    fun concurrent_finish_claims_have_exactly_one_winner() {
        val gate = CaptureTeardownGate()
        val contenders = 8
        val start = CountDownLatch(1)
        val completed = CountDownLatch(contenders)
        val winners = AtomicInteger()

        assertTrue(gate.beginTeardown())
        gate.markOutcomeReady()
        repeat(contenders) {
            Thread {
                start.await()
                if (gate.claimFinish()) {
                    winners.incrementAndGet()
                }
                completed.countDown()
            }.start()
        }
        start.countDown()

        assertTrue(completed.await(2, TimeUnit.SECONDS))
        assertEquals(1, winners.get())
    }
}
