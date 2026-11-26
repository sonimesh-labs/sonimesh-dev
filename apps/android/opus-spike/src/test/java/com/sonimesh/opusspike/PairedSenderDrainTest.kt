package com.sonimesh.opusspike

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PairedSenderDrainTest {
    @Test fun completedSenderLeavesSessionActiveForStop() {
        val active = AtomicBoolean(true)
        val sender = Thread { }.apply { start(); join() }
        assertEquals(SenderDrainResult.Complete, drainPairedSender(sender, active, 100, 100))
        assertTrue(active.get())
    }

    @Test fun failedSenderReportsIncomplete() {
        val active = AtomicBoolean(false)
        val sender = Thread { }.apply { start(); join() }
        assertEquals(SenderDrainResult.Incomplete, drainPairedSender(sender, active, 100, 100))
        assertFalse(active.get())
    }

    @Test fun timeoutRetiresSessionAndInterruptsSender() {
        val active = AtomicBoolean(true)
        val started = CountDownLatch(1)
        val sender = Thread {
            started.countDown()
            try { Thread.sleep(10_000) } catch (_: InterruptedException) { }
        }.apply { start() }
        try {
            assertTrue(started.await(1, TimeUnit.SECONDS))
            assertEquals(SenderDrainResult.Incomplete, drainPairedSender(sender, active, 10, 1_000))
            assertFalse(active.get())
            assertFalse(sender.isAlive)
        } finally {
            sender.interrupt()
            sender.join(1_000)
        }
    }

    @Test fun blockedSenderIsReportedUntilItActuallyExits() {
        val active = AtomicBoolean(true)
        val started = CountDownLatch(1)
        val release = CountDownLatch(1)
        val sender = Thread {
            started.countDown()
            while (true) {
                try { release.await(); break } catch (_: InterruptedException) { }
            }
        }.apply { isDaemon = true; start() }
        try {
            assertTrue(started.await(1, TimeUnit.SECONDS))
            assertEquals(SenderDrainResult.StillRunning, drainPairedSender(sender, active, 10, 10))
            assertFalse(active.get())
            assertTrue(sender.isAlive)
        } finally {
            release.countDown()
            sender.join(1_000)
        }
    }
}
