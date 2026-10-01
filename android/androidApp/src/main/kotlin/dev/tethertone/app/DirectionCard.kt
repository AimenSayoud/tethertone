package dev.tethertone.app

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import dev.tethertone.shared.StreamMode
import dev.tethertone.shared.UpSource
import dev.tethertone.shared.UplinkStatus

/** Which way audio flows, as the user thinks of it on the phone. */
enum class Direction(val label: String) {
    ToPhone("Mac → phone"),
    ToMac("Phone → Mac"),
    Both("Both ways");

    fun mode(source: UpSource): StreamMode {
        val up = if (source == UpSource.OFF) UpSource.MIC else source
        return when (this) {
            ToPhone -> StreamMode(down = true, up = UpSource.OFF)
            ToMac -> StreamMode(down = false, up = up)
            Both -> StreamMode(down = true, up = up)
        }
    }

    companion object {
        fun of(mode: StreamMode) = when {
            mode.up == UpSource.OFF -> ToPhone
            !mode.down -> ToMac
            else -> Both
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DirectionCard(
    mode: StreamMode,
    onModeChange: (StreamMode) -> Unit,
    uplink: UplinkStatus,
    upLevel: Float,
    connected: Boolean,
    playbackSupported: Boolean,
    needsCaptureConsent: Boolean,
    onAllowCapture: () -> Unit,
    note: String?,
) {
    val direction = Direction.of(mode)
    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface)) {
        Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text("Direction", style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.SemiBold)

            SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                Direction.entries.forEachIndexed { index, option ->
                    SegmentedButton(
                        selected = option == direction,
                        onClick = { onModeChange(option.mode(mode.up)) },
                        shape = SegmentedButtonDefaults.itemShape(index, Direction.entries.size),
                        label = { Text(option.label, maxLines = 1) },
                    )
                }
            }

            if (mode.up != UpSource.OFF) {
                Text("Send from this phone", style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
                val sources = if (playbackSupported) listOf(UpSource.MIC, UpSource.PLAYBACK) else listOf(UpSource.MIC)
                SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                    sources.forEachIndexed { index, source ->
                        SegmentedButton(
                            selected = source == mode.up,
                            onClick = { onModeChange(mode.copy(up = source)) },
                            shape = SegmentedButtonDefaults.itemShape(index, sources.size),
                            label = { Text(if (source == UpSource.MIC) "Microphone" else "Its audio", maxLines = 1) },
                        )
                    }
                }

                if (mode.up == UpSource.MIC && mode.down) {
                    Text(
                        "Use headphones: otherwise this phone's speaker is heard by its own microphone.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                if (mode.up == UpSource.PLAYBACK) {
                    Text(
                        "Sends what other apps play. Some apps block capture, and calls are never captured.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }

                if (needsCaptureConsent) {
                    OutlinedButton(onClick = onAllowCapture, modifier = Modifier.fillMaxWidth()) {
                        Text("Allow capturing this phone's audio")
                    }
                }

                when {
                    uplink.active -> {
                        Text(
                            "Sending ${if (uplink.source == UpSource.MIC) "microphone" else "phone audio"} · ${uplink.kbps} kbit/s",
                            style = MaterialTheme.typography.bodySmall,
                            color = StatusColors.ok,
                        )
                        LevelMeter(upLevel)
                    }
                    uplink.problem != null -> Text(
                        uplink.problem!!, style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                    !connected -> Text(
                        "Starts when you connect.", style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }

            note?.let {
                Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
            }
        }
    }
}
