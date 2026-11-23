package com.sonimesh.opusspike

import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/** Runs after the producer has stopped adding packets, until its bounded queue is empty. */
internal fun <T> sendUntilProducerFinished(
    packets: ArrayBlockingQueue<T>,
    producerFinished: AtomicBoolean,
    send: (T) -> Unit,
) {
    while (!producerFinished.get() || packets.isNotEmpty()) {
        val packet = packets.poll(20, TimeUnit.MILLISECONDS) ?: continue
        send(packet)
    }
}

/** Gives a sender a bounded opportunity to finish before the control stop. */
internal fun finishPairedSender(producerFinished: AtomicBoolean, sender: Thread?) {
    producerFinished.set(true)
    if (sender == null) return
    try {
        sender.join(250)
    } catch (_: InterruptedException) {
        Thread.currentThread().interrupt()
    }
    if (sender.isAlive) sender.interrupt()
}
