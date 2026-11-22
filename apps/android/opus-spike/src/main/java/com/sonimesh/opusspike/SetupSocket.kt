package com.sonimesh.opusspike

import java.net.Socket

/** Tracks only the socket owned by an in-progress paired setup. */
internal class SetupSocket {
    private var cancelled = false
    private var current: Socket? = null

    @Synchronized
    fun track(socket: Socket): Boolean {
        if (cancelled) {
            socket.close()
            return false
        }
        current = socket
        return true
    }

    @Synchronized
    fun handoff(): Boolean {
        if (cancelled) return false
        current = null
        return true
    }

    fun cancel() {
        val socket = synchronized(this) {
            cancelled = true
            current.also { current = null }
        }
        try { socket?.close() } catch (_: java.io.IOException) { }
    }
}
