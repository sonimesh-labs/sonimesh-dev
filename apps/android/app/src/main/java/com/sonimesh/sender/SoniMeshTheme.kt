package com.sonimesh.sender

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

private val LightColors = lightColorScheme(
    primary = Color(0xFF006B5C),
    onPrimary = Color(0xFFFFFFFF),
    primaryContainer = Color(0xFFB9F3DF),
    onPrimaryContainer = Color(0xFF002019),
    secondary = Color(0xFF4B635B),
    onSecondary = Color(0xFFFFFFFF),
    secondaryContainer = Color(0xFFCDE9DE),
    onSecondaryContainer = Color(0xFF071F18),
    tertiary = Color(0xFF765A00),
    onTertiary = Color(0xFFFFFFFF),
    tertiaryContainer = Color(0xFFFFDEA0),
    onTertiaryContainer = Color(0xFF261A00),
    error = Color(0xFFB3261E),
    onError = Color(0xFFFFFFFF),
    errorContainer = Color(0xFFFFDAD6),
    onErrorContainer = Color(0xFF410002),
    background = Color(0xFFF7FBF8),
    onBackground = Color(0xFF17211E),
    surface = Color(0xFFF7FBF8),
    onSurface = Color(0xFF17211E),
    surfaceVariant = Color(0xFFDDE6E0),
    onSurfaceVariant = Color(0xFF414944),
    outline = Color(0xFF717A74),
)

private val DarkColors = darkColorScheme(
    primary = Color(0xFF9BD8C3),
    onPrimary = Color(0xFF00382F),
    primaryContainer = Color(0xFF005143),
    onPrimaryContainer = Color(0xFFB9F3DF),
    secondary = Color(0xFFB1CCC0),
    onSecondary = Color(0xFF1C352C),
    secondaryContainer = Color(0xFF344C42),
    onSecondaryContainer = Color(0xFFCDE9DE),
    tertiary = Color(0xFFF1C56F),
    onTertiary = Color(0xFF3E2E00),
    tertiaryContainer = Color(0xFF5A4300),
    onTertiaryContainer = Color(0xFFFFDEA0),
    error = Color(0xFFFFB4AB),
    onError = Color(0xFF690005),
    errorContainer = Color(0xFF93000A),
    onErrorContainer = Color(0xFFFFDAD6),
    background = Color(0xFF101512),
    onBackground = Color(0xFFDFE4E0),
    surface = Color(0xFF101512),
    onSurface = Color(0xFFDFE4E0),
    surfaceVariant = Color(0xFF414944),
    onSurfaceVariant = Color(0xFFC1C9C3),
    outline = Color(0xFF8B938D),
)

@Composable
internal fun SoniMeshTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = if (isSystemInDarkTheme()) DarkColors else LightColors,
        content = content,
    )
}
