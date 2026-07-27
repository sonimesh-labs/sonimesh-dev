package com.sonimesh.sender.capture

import java.util.concurrent.CountDownLatch
import java.util.concurrent.atomic.AtomicReference
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.fail
import org.junit.Test

class SaturatingAtomicCounterTest {
    @Test
    fun counter_saturates_instead_of_wrapping_negative() {
        val counter = SaturatingAtomicCounter(initialValue = Long.MAX_VALUE - 1)

        counter.add(2)
        counter.increment()

        assertEquals(Long.MAX_VALUE, counter.get())
        counter.reset()
        assertEquals(0, counter.get())
    }

    @Test
    fun concurrent_increments_remain_saturated() {
        val counter = SaturatingAtomicCounter(initialValue = Long.MAX_VALUE - 100)
        val start = CountDownLatch(1)
        val failure = AtomicReference<Throwable?>()
        val workers = List(4) {
            Thread {
                try {
                    start.await()
                    repeat(10_000) {
                        counter.increment()
                    }
                } catch (error: Throwable) {
                    failure.compareAndSet(null, error)
                }
            }
        }

        workers.forEach(Thread::start)
        start.countDown()
        workers.forEach { worker ->
            worker.join(5_000)
            assertFalse("counter worker did not finish", worker.isAlive)
        }
        failure.get()?.let { error ->
            throw AssertionError("counter worker failed", error)
        }
        assertEquals(Long.MAX_VALUE, counter.get())
    }

    @Test
    fun counter_rejects_negative_initial_values_and_nonpositive_increments() {
        try {
            SaturatingAtomicCounter(initialValue = -1)
            fail("expected a negative initial counter value to be rejected")
        } catch (_: IllegalArgumentException) {
            // Expected: evidence counters are nonnegative.
        }

        val counter = SaturatingAtomicCounter()
        for (delta in listOf(0, -1)) {
            try {
                counter.add(delta)
                fail("expected a nonpositive counter increment to be rejected")
            } catch (_: IllegalArgumentException) {
                // Expected: evidence mutations only add completed work.
            }
        }
    }
}
