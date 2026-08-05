package com.sonimesh.sender.capture

import java.util.concurrent.atomic.AtomicLong

/** Counter operations used by bounded capture evidence publication. */
internal interface CaptureEvidenceCounter {
    fun add(delta: Int)

    fun increment()

    fun get(): Long

    fun reset()
}

/** A nonnegative atomic counter that saturates before signed overflow. */
internal class SaturatingAtomicCounter(initialValue: Long = 0) : CaptureEvidenceCounter {
    init {
        require(initialValue >= 0) { "counter initial value must be nonnegative" }
    }

    private val value = AtomicLong(initialValue)

    override fun add(delta: Int) {
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

    override fun increment() {
        add(1)
    }

    override fun get(): Long = value.get()

    override fun reset() {
        value.set(0)
    }
}
