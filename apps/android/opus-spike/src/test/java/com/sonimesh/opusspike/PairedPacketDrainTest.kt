package com.sonimesh.opusspike

import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PairedPacketDrainTest {
    @Test
    fun sendsQueuedPacketsInOrderBeforeFinishing() {
        val packets = ArrayBlockingQueue<Int>(8)
        val finished = AtomicBoolean(false)
        val firstSend = CountDownLatch(1)
        val release = CountDownLatch(1)
        val sent = mutableListOf<Int>()
        packets.add(1)
        val sender = Thread {
            sendUntilProducerFinished(packets, finished) { packet ->
                if (packet == 1) {
                    firstSend.countDown()
                    check(release.await(2, TimeUnit.SECONDS))
                }
                sent.add(packet)
            }
        }
        sender.start()
        assertTrue(firstSend.await(2, TimeUnit.SECONDS))
        packets.add(2)
        packets.add(3)
        release.countDown()
        finishPairedSender(finished, sender)
        assertFalse(sender.isAlive)
        assertEquals(listOf(1, 2, 3), sent)
        assertTrue(packets.isEmpty())
    }

    @Test
    fun stalledSenderDoesNotBlockShutdownIndefinitely() {
        val finished = AtomicBoolean(false)
        val entered = CountDownLatch(1)
        val interrupted = CountDownLatch(1)
        val sender = Thread {
            entered.countDown()
            try {
                CountDownLatch(1).await()
            } catch (_: InterruptedException) {
                interrupted.countDown()
            }
        }
        sender.start()
        assertTrue(entered.await(2, TimeUnit.SECONDS))
        finishPairedSender(finished, sender)
        assertTrue(finished.get())
        assertTrue(interrupted.await(2, TimeUnit.SECONDS))
        sender.join(2_000)
        assertFalse(sender.isAlive)
    }
}
