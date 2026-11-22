package com.sonimesh.sender.capture

import android.media.AudioAttributes

enum class CaptureSource(
    val wireValue: String,
    val usages: List<Int>,
) {
    Media(
        wireValue = "media",
        usages = listOf(AudioAttributes.USAGE_MEDIA),
    ),
    Game(
        wireValue = "game",
        usages = listOf(AudioAttributes.USAGE_GAME),
    ),
    Unknown(
        wireValue = "unknown",
        usages = listOf(AudioAttributes.USAGE_UNKNOWN),
    );

    companion object {
        fun fromWireValue(value: String?): CaptureSource =
            entries.firstOrNull { it.wireValue == value } ?: Media
    }
}
