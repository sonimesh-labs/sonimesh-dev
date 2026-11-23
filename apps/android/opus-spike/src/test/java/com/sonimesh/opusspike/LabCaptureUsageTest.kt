package com.sonimesh.opusspike

import android.media.AudioAttributes
import org.junit.Assert.assertEquals
import org.junit.Test

class LabCaptureUsageTest {
    @Test
    fun selectedUsageMapsToTheRequestedPlatformUsages() {
        assertEquals(listOf(AudioAttributes.USAGE_MEDIA, AudioAttributes.USAGE_GAME, AudioAttributes.USAGE_UNKNOWN), LabCaptureUsage.All.usages)
        assertEquals(listOf(AudioAttributes.USAGE_MEDIA), LabCaptureUsage.Media.usages)
        assertEquals(listOf(AudioAttributes.USAGE_GAME), LabCaptureUsage.Game.usages)
        assertEquals(listOf(AudioAttributes.USAGE_UNKNOWN), LabCaptureUsage.Other.usages)
    }

    @Test
    fun savedSelectionsRoundTripAndUnknownValuesKeepTheExistingDefault() {
        LabCaptureUsage.entries.forEach { assertEquals(it, LabCaptureUsage.fromValue(it.value)) }
        assertEquals(LabCaptureUsage.All, LabCaptureUsage.fromValue(null))
        assertEquals(LabCaptureUsage.All, LabCaptureUsage.fromValue("invalid"))
    }
}
