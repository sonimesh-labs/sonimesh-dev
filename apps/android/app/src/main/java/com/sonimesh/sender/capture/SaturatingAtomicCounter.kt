package com.sonimesh.sender.capture

import java.util.concurrent.atomic.AtomicLong

/** A nonnegative atomic counter that saturates before signed overflow. */
internal class SaturatingAtomicCounter(initialValue: Long = 0) {
    init {
        require(initialValue >= 0) { "counter initial value must be nonnegative" }
    }

    private val value = AtomicLong(initialValue)

    fun add(delta: Int) {
        require(delta > 0) { "counter increment must be positive" }
        val increment = delta.toLong()
        while (true) {
            val previous = value.get()
            val next = if (previous > Long.MAX_VALUE - increment) {
                Long.MAX_VALUE
            } else {
                previous + increment
            }
            if (next == previous || value.compareAndSet(previous, next)) {
                return
            }
        }
    }

    fun increment() {
        add(1)
    }

    fun get(): Long = value.get()

    fun reset() {
        value.set(0)
    }
}
