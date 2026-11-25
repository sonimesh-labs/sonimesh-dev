package com.sonimesh.opusspike

import org.junit.Assert.assertEquals
import org.junit.Test

class ForegroundNotificationTest {
    @Test
    fun signalRemainsDistinctFromConnectionAndMute() {
        val (compact, expanded) = foregroundNotificationText(
            "paired UDP", "Reconnecting to desktop", true, "Capture signal detected",
        )
        assertEquals("Transport: paired UDP · Reconnecting to desktop · Sending muted", compact)
        assertEquals("$compact\nCapture signal detected", expanded)
        val (restored, silent) = foregroundNotificationText(
            "paired UDP", "Desktop reconnected", false, "No capturable audio detected",
        )
        assertEquals("Transport: paired UDP · Desktop reconnected", restored)
        assertEquals("$restored\nNo capturable audio detected", silent)
    }
}
