package com.sonimesh.sender.capture

import android.media.AudioAttributes
import org.junit.Assert.assertEquals
import org.junit.Test

class CaptureSourceTest {
    @Test
    fun source_options_request_their_playback_usages() {
        assertEquals(listOf(AudioAttributes.USAGE_MEDIA), CaptureSource.Media.usages)
        assertEquals(listOf(AudioAttributes.USAGE_GAME), CaptureSource.Game.usages)
        assertEquals(listOf(AudioAttributes.USAGE_UNKNOWN), CaptureSource.Unknown.usages)
        assertEquals(
            listOf(AudioAttributes.USAGE_MEDIA, AudioAttributes.USAGE_GAME, AudioAttributes.USAGE_UNKNOWN),
            CaptureSource.AllSupported.usages,
        )
    }

    @Test
    fun unknown_wire_values_fall_back_to_media() {
        assertEquals(CaptureSource.Media, CaptureSource.fromWireValue("unsupported"))
        assertEquals(CaptureSource.Media, CaptureSource.fromWireValue(null))
        assertEquals(CaptureSource.Game, CaptureSource.fromWireValue("game"))
        assertEquals(CaptureSource.AllSupported, CaptureSource.fromWireValue("all_supported"))
    }
}
