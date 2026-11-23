package com.sonimesh.opusspike

import android.media.AudioAttributes

internal enum class LabCaptureUsage(val value: String, val label: String, val usages: List<Int>) {
    All(
        "all", "All supported usages",
        listOf(AudioAttributes.USAGE_MEDIA, AudioAttributes.USAGE_GAME, AudioAttributes.USAGE_UNKNOWN),
    ),
    Media("media", "Media", listOf(AudioAttributes.USAGE_MEDIA)),
    Game("game", "Game", listOf(AudioAttributes.USAGE_GAME)),
    Other("other", "Other", listOf(AudioAttributes.USAGE_UNKNOWN)),
    ;

    companion object {
        fun fromValue(value: String?): LabCaptureUsage = entries.firstOrNull { it.value == value } ?: All
    }
}
