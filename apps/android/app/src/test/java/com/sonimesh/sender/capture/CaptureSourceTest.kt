package com.sonimesh.sender.capture

import android.media.AudioAttributes
import org.junit.Assert.assertEquals
import org.junit.Test

class CaptureSourceTest {
    @Test
    fun each_source_requests_only_its_selected_playback_usage() {
        assertEquals(listOf(AudioAttributes.USAGE_MEDIA), CaptureSource.Media.usages)
        assertEquals(listOf(AudioAttributes.USAGE_GAME), CaptureSource.Game.usages)
        assertEquals(listOf(AudioAttributes.USAGE_UNKNOWN), CaptureSource.Unknown.usages)
    }

    @Test
    fun unknown_wire_values_fall_back_to_media() {
        assertEquals(CaptureSource.Media, CaptureSource.fromWireValue("unsupported"))
        assertEquals(CaptureSource.Media, CaptureSource.fromWireValue(null))
        assertEquals(CaptureSource.Game, CaptureSource.fromWireValue("game"))
    }
}
