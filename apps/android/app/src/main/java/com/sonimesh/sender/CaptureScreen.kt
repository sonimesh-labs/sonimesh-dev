package com.sonimesh.sender

import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.sonimesh.sender.capture.CaptureEvidenceSnapshot
import com.sonimesh.sender.capture.CaptureStatus
import com.sonimesh.sender.capture.CaptureUiState
import com.sonimesh.sender.capture.isActive
import com.sonimesh.sender.capture.isAwaitingAuthorization
import java.util.Locale

private val CardShape = RoundedCornerShape(22.dp)
private val TileShape = RoundedCornerShape(15.dp)
private val StatusSurface = Color(0xFF11171C)
private val StatusText = Color(0xFFF5F8F7)
private val StatusMutedText = Color(0xFFB1BCBE)

@Composable
internal fun CaptureScreen(
    state: CaptureUiState,
    onStart: () -> Unit,
    onStop: () -> Unit,
) {
    Column(
        modifier = Modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.background)
            .statusBarsPadding()
            .verticalScroll(rememberScrollState())
            .navigationBarsPadding()
            .padding(horizontal = 20.dp, vertical = 24.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        CaptureHeader()
        CaptureStatusCard(status = state.status)
        CaptureSummaryCard(status = state.status, evidence = state.evidence)
        if (state.evidence.hasObservations()) {
            CaptureEvidenceCard(evidence = state.evidence)
        }
        CaptureActionCard(
            active = state.status.isActive(),
            starting = state.status.isAwaitingAuthorization(),
            onStart = onStart,
            onStop = onStop,
        )
        CaptureBoundaryCard()
    }
}

@Composable
private fun CaptureHeader() {
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            BrandMark()
            Text(
                "SoniMesh",
                color = MaterialTheme.colorScheme.onSurface,
                fontWeight = FontWeight.Bold,
                style = MaterialTheme.typography.titleMedium,
            )
        }
        Text(
            "Capture phone audio",
            color = MaterialTheme.colorScheme.onSurface,
            fontWeight = FontWeight.SemiBold,
            style = MaterialTheme.typography.headlineMedium,
        )
        Text(
            "Private, on-device control for bounded playback-capture checks.",
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            style = MaterialTheme.typography.bodyMedium,
        )
    }
}

@Composable
private fun BrandMark() {
    Box(
        modifier = Modifier
            .size(30.dp)
            .clip(RoundedCornerShape(9.dp))
            .background(MaterialTheme.colorScheme.primary),
        contentAlignment = Alignment.Center,
    ) {
        Row(
            horizontalArrangement = Arrangement.spacedBy(2.5.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            listOf(6.dp, 12.dp, 8.dp, 14.dp).forEach { barHeight ->
                Box(
                    modifier = Modifier
                        .width(2.5.dp)
                        .height(barHeight)
                        .clip(RoundedCornerShape(2.dp))
                        .background(MaterialTheme.colorScheme.onPrimary),
                )
            }
        }
    }
}

@Composable
private fun CaptureStatusCard(status: CaptureStatus) {
    val palette = status.palette()
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .semantics { liveRegion = LiveRegionMode.Polite },
        colors = CardDefaults.cardColors(containerColor = StatusSurface),
        shape = CardShape,
    ) {
        Column(
            modifier = Modifier.padding(22.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                StatusPill(text = status.label(), palette = palette)
                if (status is CaptureStatus.Capturing) {
                    BreathingStatusIndicator(color = palette.indicator)
                } else {
                    StatusIndicator(color = palette.indicator)
                }
            }
            Text(
                text = status.headline(),
                color = StatusText,
                fontWeight = FontWeight.SemiBold,
                style = MaterialTheme.typography.headlineSmall,
            )
            Text(
                text = status.description(),
                color = StatusMutedText,
                style = MaterialTheme.typography.bodyMedium,
            )
            SignalRail(active = status is CaptureStatus.Capturing, color = palette.indicator)
        }
    }
}

@Composable
private fun SignalRail(active: Boolean, color: Color) {
    val transition = rememberInfiniteTransition(label = "signal-rail")
    val pulse = transition.animateFloat(
        initialValue = 0.45f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(
            animation = tween(durationMillis = 900),
            repeatMode = RepeatMode.Reverse,
        ),
        label = "rail-pulse",
    ).value
    val barHeights = listOf(8, 18, 12, 26, 14, 30, 16, 22, 10, 26, 14, 20, 8, 18, 12, 24)
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .height(34.dp),
        horizontalArrangement = Arrangement.spacedBy(4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        barHeights.forEachIndexed { index, height ->
            val emphasized = index % 5 == 2
            val barColor = if (emphasized) color else color.copy(alpha = 0.35f)
            val alpha = if (active) 0.55f + 0.45f * pulse else 0.55f
            Box(
                modifier = Modifier
                    .weight(1f)
                    .height(height.dp)
                    .clip(RoundedCornerShape(3.dp))
                    .background(barColor.copy(alpha = barColor.alpha * alpha)),
            )
        }
    }
}

@Composable
private fun CaptureSummaryCard(
    status: CaptureStatus,
    evidence: CaptureEvidenceSnapshot,
) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
        shape = CardShape,
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            SectionHeader(
                eyebrow = "SESSION",
                title = "Session signal",
                supporting = "A compact readout of the current capture run.",
            )
            Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                SummaryMetric(
                    modifier = Modifier.weight(1f),
                    label = "State",
                    value = status.metricLabel(),
                )
                SummaryMetric(
                    modifier = Modifier.weight(1f),
                    label = "Signal",
                    value = status.signalLabel(),
                )
            }
            Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                SummaryMetric(
                    modifier = Modifier.weight(1f),
                    label = "Duration",
                    value = formatDuration(evidence.captureDurationMillis),
                )
                SummaryMetric(
                    modifier = Modifier.weight(1f),
                    label = "Read errors",
                    value = formatCount(evidence.readErrors),
                )
            }
        }
    }
}

@Composable
private fun CaptureEvidenceCard(evidence: CaptureEvidenceSnapshot) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
        shape = CardShape,
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            SectionHeader(
                eyebrow = "EVIDENCE",
                title = "Local observations",
                supporting = "Capture counters stay on this device and summarize the latest run.",
            )
            Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                EvidenceItem("Frames", formatCount(evidence.capturedFrames), Modifier.weight(1f))
                EvidenceItem("Silent frames", formatCount(evidence.silentFrames), Modifier.weight(1f))
            }
            Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                EvidenceItem("Silent reads", formatCount(evidence.silentReads), Modifier.weight(1f))
                EvidenceItem("Read errors", formatCount(evidence.readErrors), Modifier.weight(1f))
            }
            EvidenceItem(
                "Latest read",
                evidence.lastReadCompletionOffsetMillis?.let { "+${formatDuration(it)}" } ?: "Unavailable",
                Modifier.fillMaxWidth(),
            )
        }
    }
}

@Composable
private fun CaptureActionCard(
    active: Boolean,
    starting: Boolean,
    onStart: () -> Unit,
    onStop: () -> Unit,
) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
        shape = CardShape,
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            SectionHeader(
                eyebrow = "CONTROL",
                title = when {
                    active -> "Capture is running"
                    starting -> "Waiting for authorization"
                    else -> "Ready to start"
                },
                supporting = when {
                    active -> "Stopping ends the current bounded run and leaves the latest evidence on screen."
                    starting -> "Complete or dismiss the system permission request before starting another run."
                    else -> "Starting asks for permission and a fresh projection consent before capture begins."
                },
            )
            if (active) {
                OutlinedButton(
                    modifier = Modifier
                        .fillMaxWidth()
                        .heightIn(min = 50.dp),
                    onClick = onStop,
                    shape = TileShape,
                    colors = ButtonDefaults.outlinedButtonColors(contentColor = MaterialTheme.colorScheme.error),
                ) {
                    Text("Stop capture", fontWeight = FontWeight.SemiBold)
                }
            } else {
                Button(
                    modifier = Modifier
                        .fillMaxWidth()
                        .heightIn(min = 50.dp),
                    enabled = !starting,
                    onClick = onStart,
                    shape = TileShape,
                    colors = ButtonDefaults.buttonColors(
                        containerColor = MaterialTheme.colorScheme.primary,
                        contentColor = MaterialTheme.colorScheme.onPrimary,
                    ),
                ) {
                    Text(
                        if (starting) "Waiting for system consent" else "Start capture",
                        fontWeight = FontWeight.SemiBold,
                    )
                }
            }
        }
    }
}

@Composable
private fun CaptureBoundaryCard() {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
        shape = CardShape,
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                "Capture-only boundary",
                color = MaterialTheme.colorScheme.onSurface,
                fontWeight = FontWeight.SemiBold,
                style = MaterialTheme.typography.titleSmall,
            )
            Text(
                "Audio samples are discarded locally. This screen reports observations without attributing policy limits to a specific source app.",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodyMedium,
            )
        }
    }
}

@Composable
private fun SectionHeader(
    eyebrow: String,
    title: String,
    supporting: String,
) {
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text(
            text = eyebrow,
            color = MaterialTheme.colorScheme.primary,
            fontWeight = FontWeight.Bold,
            letterSpacing = 1.4.sp,
            style = MaterialTheme.typography.labelSmall,
        )
        Text(
            text = title,
            color = MaterialTheme.colorScheme.onSurface,
            fontWeight = FontWeight.SemiBold,
            style = MaterialTheme.typography.titleMedium,
        )
        Text(
            text = supporting,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            style = MaterialTheme.typography.bodySmall,
        )
    }
}

@Composable
private fun SummaryMetric(
    modifier: Modifier,
    label: String,
    value: String,
) {
    Surface(
        modifier = modifier,
        shape = TileShape,
        color = MaterialTheme.colorScheme.surfaceContainerLow,
    ) {
        Column(
            modifier = Modifier.padding(horizontal = 14.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Text(label, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.labelSmall)
            Text(value, color = MaterialTheme.colorScheme.onSurface, fontWeight = FontWeight.SemiBold, style = MaterialTheme.typography.titleMedium)
        }
    }
}

@Composable
private fun EvidenceItem(
    label: String,
    value: String,
    modifier: Modifier = Modifier,
) {
    Surface(
        modifier = modifier,
        shape = TileShape,
        color = MaterialTheme.colorScheme.surfaceContainerLow,
    ) {
        Column(
            modifier = Modifier.padding(horizontal = 14.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Text(label, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.labelSmall)
            Text(value, color = MaterialTheme.colorScheme.onSurface, fontWeight = FontWeight.SemiBold, style = MaterialTheme.typography.titleMedium)
        }
    }
}

@Composable
private fun StatusPill(
    text: String,
    palette: StatusPalette,
) {
    Surface(
        color = palette.indicator.copy(alpha = 0.16f),
        shape = RoundedCornerShape(999.dp),
    ) {
        Text(
            modifier = Modifier.padding(horizontal = 10.dp, vertical = 5.dp),
            text = text,
            color = palette.indicator,
            fontWeight = FontWeight.Bold,
            letterSpacing = 1.2.sp,
            style = MaterialTheme.typography.labelSmall,
        )
    }
}

@Composable
private fun BreathingStatusIndicator(color: Color) {
    val transition = rememberInfiniteTransition(label = "capture-status")
    val scale = transition.animateFloat(
        initialValue = 1f,
        targetValue = 1.22f,
        animationSpec = infiniteRepeatable(
            animation = tween(durationMillis = 1_200),
            repeatMode = RepeatMode.Reverse,
        ),
        label = "status-pulse",
    ).value

    Box(
        modifier = Modifier
            .width(14.dp)
            .height(14.dp)
            .graphicsLayer {
                scaleX = scale
                scaleY = scale
            }
            .clip(CircleShape)
            .background(color),
    )
}

@Composable
private fun StatusIndicator(color: Color) {
    Box(
        modifier = Modifier
            .width(14.dp)
            .height(14.dp)
            .clip(CircleShape)
            .background(color),
    )
}

private data class StatusPalette(
    val indicator: Color,
)

@Composable
private fun CaptureStatus.palette(): StatusPalette = when (this) {
    CaptureStatus.Idle,
    CaptureStatus.AwaitingPermission,
    CaptureStatus.AwaitingNotificationPermission,
    CaptureStatus.AwaitingProjectionConsent,
    -> StatusPalette(indicator = Color(0xFFA5B1B5))
    is CaptureStatus.Capturing -> StatusPalette(indicator = Color(0xFF36C8B6))
    is CaptureStatus.NoCapturableAudio -> StatusPalette(indicator = Color(0xFFE6AE54))
    is CaptureStatus.Stopped, is CaptureStatus.Failed -> StatusPalette(indicator = Color(0xFFE66F7F))
}

private fun CaptureStatus.label(): String = when (this) {
    CaptureStatus.Idle -> "READY"
    CaptureStatus.AwaitingPermission -> "PERMISSION"
    CaptureStatus.AwaitingNotificationPermission -> "NOTIFICATIONS"
    CaptureStatus.AwaitingProjectionConsent -> "CONSENT"
    is CaptureStatus.Capturing -> "ACTIVE"
    is CaptureStatus.NoCapturableAudio -> "NO SIGNAL"
    is CaptureStatus.Stopped -> "STOPPED"
    is CaptureStatus.Failed -> "ATTENTION"
}

private fun CaptureStatus.metricLabel(): String = when (this) {
    CaptureStatus.Idle -> "Idle"
    CaptureStatus.AwaitingPermission -> "Permission"
    CaptureStatus.AwaitingNotificationPermission -> "Notifications"
    CaptureStatus.AwaitingProjectionConsent -> "Consent"
    is CaptureStatus.Capturing -> "Capturing"
    is CaptureStatus.NoCapturableAudio -> "No signal"
    is CaptureStatus.Stopped -> "Stopped"
    is CaptureStatus.Failed -> "Attention"
}

private fun CaptureStatus.headline(): String = when (this) {
    CaptureStatus.Idle -> "Ready for a fresh capture session"
    CaptureStatus.AwaitingPermission -> "Waiting for microphone permission"
    CaptureStatus.AwaitingNotificationPermission -> "Waiting for notification permission"
    CaptureStatus.AwaitingProjectionConsent -> "Waiting for screen-capture consent"
    is CaptureStatus.Capturing -> "Capture is active"
    is CaptureStatus.NoCapturableAudio -> "No capturable audio detected"
    is CaptureStatus.Stopped -> "Capture has stopped"
    is CaptureStatus.Failed -> "Capture needs attention"
}

private fun CaptureStatus.signalLabel(): String = when (this) {
    is CaptureStatus.Capturing -> "Observed"
    is CaptureStatus.NoCapturableAudio -> "None"
    else -> "Waiting"
}

private fun formatCount(value: Long): String = String.format(Locale.US, "%,d", value)
