package com.sonimesh.sender

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

/**
 * Shared SoniMesh visual foundation.
 *
 * The product uses a calm, warm studio canvas with a deep receiver/capture
 * surface for the realtime state. Semantic colors stay explicit so status is
 * never communicated through color alone.
 */
private val SoniMeshColors = lightColorScheme(
    primary = Color(0xFF168F82),
    onPrimary = Color(0xFFF5FFFC),
    primaryContainer = Color(0xFFD9F2EE),
    onPrimaryContainer = Color(0xFF075D55),
    secondary = Color(0xFF3379EA),
    onSecondary = Color(0xFFFFFFFF),
    secondaryContainer = Color(0xFFE5EFFF),
    onSecondaryContainer = Color(0xFF164A9C),
    tertiary = Color(0xFFA66712),
    onTertiary = Color(0xFFFFFBF4),
    tertiaryContainer = Color(0xFFFFF1D5),
    onTertiaryContainer = Color(0xFF6A4108),
    error = Color(0xFFBF4053),
    onError = Color(0xFFFFFBFB),
    errorContainer = Color(0xFFFDE7EB),
    onErrorContainer = Color(0xFF7C1F31),
    background = Color(0xFFF2F1ED),
    onBackground = Color(0xFF101418),
    surface = Color(0xFFFFFEFA),
    onSurface = Color(0xFF101418),
    surfaceVariant = Color(0xFFECECE6),
    onSurfaceVariant = Color(0xFF69737C),
    surfaceContainerLowest = Color(0xFFFFFEFA),
    surfaceContainerLow = Color(0xFFF7F7F3),
    surfaceContainer = Color(0xFFF1F1EC),
    surfaceContainerHigh = Color(0xFFECECE6),
    surfaceContainerHighest = Color(0xFFE4E5E0),
    outline = Color(0xFFC9CCC5),
    outlineVariant = Color(0xFFDEDFD9),
)

@Composable
internal fun SoniMeshTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = SoniMeshColors,
        content = content,
    )
}
