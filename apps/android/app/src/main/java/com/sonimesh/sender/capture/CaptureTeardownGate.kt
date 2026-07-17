package com.sonimesh.sender.capture

import java.util.concurrent.atomic.AtomicBoolean

/** Coordinates one capture teardown without making resource ownership concurrent. */
class CaptureTeardownGate {
    private val teardownStarted = AtomicBoolean(false)
    private val readerExited = AtomicBoolean(true)
    private val teardownFinished = AtomicBoolean(false)

    fun beginTeardown(): Boolean = teardownStarted.compareAndSet(false, true)

    fun isTeardownStarted(): Boolean = teardownStarted.get()

    fun markReaderRunning() {
        readerExited.set(false)
    }

    fun markReaderExited() {
        readerExited.set(true)
    }

    /** Returns true to exactly one caller after teardown starts and the reader exits. */
    fun claimFinish(): Boolean =
        teardownStarted.get() && readerExited.get() && teardownFinished.compareAndSet(false, true)
}
