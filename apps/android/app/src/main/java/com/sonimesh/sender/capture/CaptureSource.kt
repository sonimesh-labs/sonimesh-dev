package com.sonimesh.sender.capture

import android.media.AudioAttributes

enum class CaptureSource(
    val wireValue: String,
    val displayName: String,
    val usages: List<Int>,
) {
    Media(
        wireValue = "media",
        displayName = "Media",
        usages = listOf(AudioAttributes.USAGE_MEDIA),
    ),
    Game(
        wireValue = "game",
        displayName = "Game",
        usages = listOf(AudioAttributes.USAGE_GAME),
    ),
    Unknown(
        wireValue = "unknown",
        displayName = "Other",
        usages = listOf(AudioAttributes.USAGE_UNKNOWN),
    );

    companion object {
        fun fromWireValue(value: String?): CaptureSource =
            entries.firstOrNull { it.wireValue == value } ?: Media
    }
}
