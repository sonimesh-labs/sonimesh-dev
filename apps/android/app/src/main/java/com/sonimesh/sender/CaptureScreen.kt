package com.sonimesh.sender

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
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
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.sonimesh.sender.capture.CaptureEvidenceSnapshot
import com.sonimesh.sender.capture.CaptureStatus
import com.sonimesh.sender.capture.CaptureUiState
import java.util.Locale

@Composable
internal fun CaptureScreen(
    state: CaptureUiState,
    onStart: () -> Unit,
    onStop: () -> Unit,
) {
    Column(
        modifier = Modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.surface)
            .verticalScroll(rememberScrollState())
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
            onStart = onStart,
            onStop = onStop,
        )
        CaptureBoundaryCard()
    }
}

@Composable
private fun CaptureHeader() {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(
            "SONIMESH",
            color = MaterialTheme.colorScheme.primary,
            fontWeight = FontWeight.Bold,
            style = MaterialTheme.typography.labelMedium,
        )
        Text(
            "Playback capture",
            color = MaterialTheme.colorScheme.onSurface,
            style = MaterialTheme.typography.headlineMedium,
        )
        Text(
            "A bounded mobile control surface for capture-only feasibility runs.",
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            style = MaterialTheme.typography.bodyMedium,
        )
    }
}

@Composable
private fun CaptureStatusCard(status: CaptureStatus) {
    val palette = status.palette()
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = palette.container),
        shape = RoundedCornerShape(20.dp),
    ) {
        Column(
            modifier = Modifier.padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    StatusPill(text = status.label(), palette = palette)
                    Text(
                        text = status.headline(),
                        color = MaterialTheme.colorScheme.onSurface,
                        style = MaterialTheme.typography.titleLarge,
                    )
                }
                StatusIndicator(color = palette.indicator)
            }
            Text(
                text = status.description(),
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodyMedium,
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
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerLow),
        shape = RoundedCornerShape(18.dp),
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            SectionHeader(
                eyebrow = "SESSION",
                title = "Current run",
                supporting = "Keep capture status, observed signal, and local evidence in one place.",
            )
            Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
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
            Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
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
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerLow),
        shape = RoundedCornerShape(18.dp),
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            SectionHeader(
                eyebrow = "EVIDENCE",
                title = "Observed locally",
                supporting = "These counters stay in the control domain and summarize the active capture session.",
            )
            Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                EvidenceItem("Frames", formatCount(evidence.capturedFrames), Modifier.weight(1f))
                EvidenceItem("Silent reads", formatCount(evidence.silentReads), Modifier.weight(1f))
            }
            Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                EvidenceItem("Read errors", formatCount(evidence.readErrors), Modifier.weight(1f))
                EvidenceItem(
                    "Latest read",
                    evidence.lastReadCompletionOffsetMillis?.let { "+${formatDuration(it)}" } ?: "Unavailable",
                    Modifier.weight(1f),
                )
            }
        }
    }
}

@Composable
private fun CaptureActionCard(
    active: Boolean,
    onStart: () -> Unit,
    onStop: () -> Unit,
) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerLowest),
        shape = RoundedCornerShape(18.dp),
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            SectionHeader(
                eyebrow = "CONTROL",
                title = if (active) "Capture is running" else "Ready to start",
                supporting = if (active) {
                    "Stopping ends the current bounded run and leaves the latest evidence on screen."
                } else {
                    "Starting asks for permission and a fresh projection consent before capture begins."
                },
            )
            if (active) {
                OutlinedButton(
                    modifier = Modifier.fillMaxWidth(),
                    onClick = onStop,
                    colors = ButtonDefaults.outlinedButtonColors(contentColor = MaterialTheme.colorScheme.error),
                ) {
                    Text("Stop capture")
                }
            } else {
                Button(
                    modifier = Modifier.fillMaxWidth(),
                    onClick = onStart,
                ) {
                    Text("Start capture")
                }
            }
        }
    }
}

@Composable
private fun CaptureBoundaryCard() {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.secondaryContainer),
        shape = RoundedCornerShape(18.dp),
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                "Capture-only boundary",
                color = MaterialTheme.colorScheme.onSecondaryContainer,
                style = MaterialTheme.typography.titleMedium,
            )
            Text(
                "Audio samples are discarded locally. This screen reports observations without attributing policy limits to a specific source app.",
                color = MaterialTheme.colorScheme.onSecondaryContainer,
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
            style = MaterialTheme.typography.labelSmall,
        )
        Text(
            text = title,
            color = MaterialTheme.colorScheme.onSurface,
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
        shape = RoundedCornerShape(16.dp),
        color = MaterialTheme.colorScheme.surface,
    ) {
        Column(
            modifier = Modifier.padding(horizontal = 14.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Text(label, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.labelSmall)
            Text(value, color = MaterialTheme.colorScheme.onSurface, style = MaterialTheme.typography.titleMedium)
        }
    }
}

@Composable
private fun EvidenceItem(label: String, value: String, modifier: Modifier) {
    Surface(
        modifier = modifier,
        shape = RoundedCornerShape(16.dp),
        color = MaterialTheme.colorScheme.surface,
    ) {
        Column(
            modifier = Modifier.padding(horizontal = 14.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Text(label, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.labelSmall)
            Text(value, color = MaterialTheme.colorScheme.onSurface, style = MaterialTheme.typography.titleMedium)
        }
    }
}

@Composable
private fun StatusPill(
    text: String,
    palette: StatusPalette,
) {
    Surface(
        color = palette.indicator,
        shape = RoundedCornerShape(999.dp),
    ) {
        Text(
            modifier = Modifier.padding(horizontal = 10.dp, vertical = 5.dp),
            text = text,
            color = palette.onIndicator,
            fontWeight = FontWeight.Bold,
            style = MaterialTheme.typography.labelSmall,
        )
    }
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
    val container: Color,
    val indicator: Color,
    val onIndicator: Color,
)

@Composable
private fun CaptureStatus.palette(): StatusPalette = when (this) {
    CaptureStatus.Idle, CaptureStatus.AwaitingPermission, CaptureStatus.AwaitingProjectionConsent -> StatusPalette(
        container = MaterialTheme.colorScheme.surfaceContainerHighest,
        indicator = MaterialTheme.colorScheme.secondary,
        onIndicator = MaterialTheme.colorScheme.onSecondary,
    )
    is CaptureStatus.Capturing -> StatusPalette(
        container = MaterialTheme.colorScheme.primaryContainer,
        indicator = MaterialTheme.colorScheme.primary,
        onIndicator = MaterialTheme.colorScheme.onPrimary,
    )
    is CaptureStatus.NoCapturableAudio -> StatusPalette(
        container = MaterialTheme.colorScheme.tertiaryContainer,
        indicator = MaterialTheme.colorScheme.tertiary,
        onIndicator = MaterialTheme.colorScheme.onTertiary,
    )
    is CaptureStatus.Stopped, is CaptureStatus.Failed -> StatusPalette(
        container = MaterialTheme.colorScheme.errorContainer,
        indicator = MaterialTheme.colorScheme.error,
        onIndicator = MaterialTheme.colorScheme.onError,
    )
}

private fun CaptureStatus.label(): String = when (this) {
    CaptureStatus.Idle -> "READY"
    CaptureStatus.AwaitingPermission -> "PERMISSION"
    CaptureStatus.AwaitingProjectionConsent -> "CONSENT"
    is CaptureStatus.Capturing -> "ACTIVE"
    is CaptureStatus.NoCapturableAudio -> "NO SIGNAL"
    is CaptureStatus.Stopped -> "STOPPED"
    is CaptureStatus.Failed -> "ATTENTION"
}

private fun CaptureStatus.metricLabel(): String = when (this) {
    CaptureStatus.Idle -> "Idle"
    CaptureStatus.AwaitingPermission -> "Permission"
    CaptureStatus.AwaitingProjectionConsent -> "Consent"
    is CaptureStatus.Capturing -> "Capturing"
    is CaptureStatus.NoCapturableAudio -> "No signal"
    is CaptureStatus.Stopped -> "Stopped"
    is CaptureStatus.Failed -> "Attention"
}

private fun CaptureStatus.headline(): String = when (this) {
    CaptureStatus.Idle -> "Ready for a fresh capture session"
    CaptureStatus.AwaitingPermission -> "Waiting for microphone permission"
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
