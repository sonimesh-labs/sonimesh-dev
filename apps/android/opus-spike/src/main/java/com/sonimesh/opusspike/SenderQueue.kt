package com.sonimesh.opusspike

import java.util.concurrent.ArrayBlockingQueue

/** Keep the newest packet when the single-producer sender queue is full. */
internal fun <T> enqueueLatest(queue: ArrayBlockingQueue<T>, packet: T): Boolean {
    if (queue.offer(packet)) return false
    val discarded = queue.poll() != null
    check(queue.offer(packet))
    return discarded
}
