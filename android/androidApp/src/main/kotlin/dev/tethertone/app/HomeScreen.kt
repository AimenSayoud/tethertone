package dev.tethertone.app

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import dev.tethertone.shared.BridgeState
import dev.tethertone.shared.BridgeStats
import dev.tethertone.shared.PairingInfo
import dev.tethertone.shared.TransportRoute
import dev.tethertone.shared.classifyHost
import dev.tethertone.shared.routes
import kotlin.math.roundToInt

@Composable
fun HomeScreen(
    state: BridgeState,
    stats: BridgeStats,
    level: Float,
    pairing: PairingInfo?,
    bufferMs: Int,
    onBufferMsChange: (Int) -> Unit,
    volume: Float,
    onVolumeChange: (Float) -> Unit,
    onScan: () -> Unit,
    onManual: () -> Unit,
    onSettings: () -> Unit,
    onConnect: () -> Unit,
    onDisconnect: () -> Unit,
    onForget: () -> Unit,
    modifier: Modifier = Modifier,
    direction: @Composable () -> Unit = {},
) {
    val live = state is BridgeState.Connected
    val busy = state is BridgeState.Connecting || state is BridgeState.Reconnecting

    Column(
        modifier = modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(start = 20.dp, end = 20.dp, top = 12.dp, bottom = 28.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Row(
            Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column {
                Text("Tethertone", style = MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.Bold)
                Text(
                    "Audio between your Mac and this phone.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            TextButton(onClick = onSettings) { Text("Settings") }
        }

        StatusCard(state)

        if (pairing != null) direction()

        if (live) {
            Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface)) {
                Column(Modifier.fillMaxWidth().padding(16.dp)) {
                    Text("Output level", style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.SemiBold)
                    Spacer(Modifier.height(10.dp))
                    LevelMeter(level)
                }
            }
            StatsGrid(stats)
            DriftCard(stats)
            VolumeSlider(volume, onVolumeChange)
        }

        if (pairing != null) {
            PairedCard(pairing, activeHost = (state as? BridgeState.Connected)?.host)
        }

        BufferSlider(bufferMs, enabled = !live && !busy, onChange = onBufferMsChange)

        // Every control keeps its position in every state, enabled or not.
        // Swapping the button set on connect used to slide "Connect" into the
        // exact spot "Disconnect" had just occupied, so a second tap silently
        // reconnected a session the user had only just stopped.
        Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
            if (live || busy) {
                Button(onClick = onDisconnect, modifier = Modifier.fillMaxWidth()) { Text("Disconnect") }
            } else {
                Button(
                    onClick = onConnect,
                    enabled = pairing != null,
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Text(
                        if (pairing == null) "Not paired yet"
                        else "Connect to ${pairing.name.ifEmpty { pairing.hosts.first() }}"
                    )
                }
            }
            OutlinedButton(
                onClick = onScan,
                enabled = !live && !busy,
                modifier = Modifier.fillMaxWidth(),
            ) {
                Text(if (pairing == null) "Scan pairing QR" else "Pair with a different Mac")
            }
            OutlinedButton(
                onClick = onManual,
                enabled = !live && !busy,
                modifier = Modifier.fillMaxWidth(),
            ) {
                Text("Enter address manually")
            }
            TextButton(
                onClick = onForget,
                enabled = pairing != null && !live && !busy,
                modifier = Modifier.fillMaxWidth(),
            ) {
                Text("Forget this Mac")
            }
        }

        if (pairing == null) {
            Text(
                "On your Mac, open Tethertone, press Start, then scan the QR on its Pairing tab.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

@Composable
private fun StatusCard(state: BridgeState) {
    val (label, detail, tint) = when (state) {
        is BridgeState.Connected -> Triple(
            "Streaming over ${classifyHost(state.host).label.lowercase()}",
            "${state.serverName} · ${state.host} · ${state.rate / 1000} kHz ${state.channels}ch",
            StatusColors.ok,
        )
        is BridgeState.Connecting -> Triple("Connecting", state.detail, StatusColors.warn)
        is BridgeState.Reconnecting -> Triple(
            "Reconnecting",
            "attempt ${state.attempt}, retrying in ${state.inSeconds}s — ${state.reason}",
            StatusColors.warn,
        )
        is BridgeState.Failed -> Triple("Not connected", state.reason, StatusColors.bad)
        BridgeState.Idle -> Triple("Idle", "Nothing playing", StatusColors.idle)
    }

    val hint = when (state) {
        is BridgeState.Reconnecting -> diagnose(state.reason)
        is BridgeState.Failed -> diagnose(state.reason)
        else -> null
    }

    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface)) {
        Column(Modifier.fillMaxWidth().padding(16.dp)) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Box(Modifier.size(12.dp).background(tint, CircleShape))
                Column {
                    Text(
                        label,
                        style = MaterialTheme.typography.titleMedium,
                        color = tint,
                        fontWeight = FontWeight.SemiBold,
                    )
                    Text(
                        detail,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            if (hint != null) {
                Spacer(Modifier.height(10.dp))
                Text(
                    hint,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurface,
                )
            }
        }
    }
}

/**
 * Turn a socket error into something the user can act on. "connect timed out"
 * repeated every few seconds tells them nothing; which of the three things went
 * wrong tells them what to do next.
 */
private fun diagnose(reason: String): String? {
    val text = reason.lowercase()
    return when {
        "invalid token" in text || "rejected" in text ->
            "The Mac has a different pairing token. Tap \"Pair with a different Mac\" and scan the QR again."
        "refused" in text || "econnrefused" in text ->
            "The Mac answered but nothing is listening there. Press Start in Tethertone on the Mac."
        "timed out" in text || "etimedout" in text || "unreachable" in text ||
            "failed to connect" in text || "no route" in text ->
            "Nothing answered. The phone and the Mac may not be on the same network, or the Wi-Fi " +
                "blocks devices from talking to each other. Plug in the USB cable — the app uses it " +
                "automatically when it is there."
        else -> null
    }
}

@Composable
private fun StatsGrid(stats: BridgeStats) {
    Row(horizontalArrangement = Arrangement.spacedBy(10.dp), modifier = Modifier.fillMaxWidth()) {
        StatTile("${stats.bufferMs}", "buffer ms", Modifier.weight(1f))
        StatTile("${stats.kbps}", "kbit/s", Modifier.weight(1f))
    }
    Row(horizontalArrangement = Arrangement.spacedBy(10.dp), modifier = Modifier.fillMaxWidth()) {
        StatTile(
            "${stats.underruns}", "underruns", Modifier.weight(1f),
            tint = if (stats.underruns > 0) StatusColors.warn else null,
        )
        StatTile(
            "${stats.lostFrames}", "lost packets", Modifier.weight(1f),
            tint = if (stats.lostFrames > 0) StatusColors.warn else null,
        )
    }
}

@Composable
private fun StatTile(value: String, label: String, modifier: Modifier = Modifier, tint: Color? = null) {
    Card(modifier, colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant)) {
        Column(Modifier.padding(14.dp)) {
            Text(
                value,
                style = MaterialTheme.typography.titleLarge,
                fontFamily = FontFamily.Monospace,
                color = tint ?: MaterialTheme.colorScheme.onSurface,
            )
            Text(label, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

/**
 * The drift correction is the part of this app most worth seeing work, so it
 * gets its own readout rather than hiding inside a log.
 */
@Composable
private fun DriftCard(stats: BridgeStats) {
    val ppm = ((stats.speed - 1.0f) * 1_000_000f).roundToInt()
    val verdict = when {
        !stats.driftCorrectionActive ->
            "unavailable on this audio path — the buffer is corrected by discarding instead"
        stats.speed == 1.0f -> "holding — no correction needed"
        ppm > 0 -> "playing $ppm ppm fast to drain the buffer"
        else -> "playing ${-ppm} ppm slow to refill the buffer"
    }
    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface)) {
        Column(Modifier.fillMaxWidth().padding(16.dp)) {
            Text("Clock drift", style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.SemiBold)
            Spacer(Modifier.height(4.dp))
            Text(verdict, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Text(
                "up ${stats.secondsConnected}s · ${stats.overruns} overruns",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

@Composable
private fun PairedCard(pairing: PairingInfo, activeHost: String?) {
    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant)) {
        Column(Modifier.fillMaxWidth().padding(16.dp)) {
            Text(
                pairing.name.ifEmpty { "Paired Mac" },
                style = MaterialTheme.typography.titleSmall,
                fontWeight = FontWeight.SemiBold,
            )
            Text(
                "${pairing.hosts.joinToString(", ")} : ${pairing.port}",
                style = MaterialTheme.typography.bodySmall,
                fontFamily = FontFamily.Monospace,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Spacer(Modifier.height(10.dp))
            RouteRow(pairing.routes(), active = activeHost?.let(::classifyHost))
        }
    }
}

/**
 * The three ways in, and which one is carrying audio right now.
 *
 * Worth showing plainly: when a connection fails it is almost always because
 * the route the user assumed was available is not — the phone is on mobile
 * data, or the cable is not in — and a list of what is actually on offer
 * answers that at a glance.
 */
@Composable
private fun RouteRow(offered: List<TransportRoute>, active: TransportRoute?) {
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(
            "Ways in",
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        for (route in TransportRoute.entries) {
            val isOffered = route in offered
            val isActive = route == active
            val tint = when {
                isActive -> StatusColors.ok
                isOffered -> MaterialTheme.colorScheme.onSurface
                else -> MaterialTheme.colorScheme.onSurfaceVariant
            }
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Box(
                    Modifier.size(7.dp).background(
                        if (isActive) StatusColors.ok
                        else if (isOffered) MaterialTheme.colorScheme.onSurfaceVariant
                        else MaterialTheme.colorScheme.surfaceVariant,
                        CircleShape,
                    )
                )
                Text(
                    route.label + when {
                        isActive -> " — in use"
                        isOffered -> " — offered"
                        else -> " — not available"
                    },
                    style = MaterialTheme.typography.bodySmall,
                    color = tint,
                    fontWeight = if (isActive) FontWeight.SemiBold else FontWeight.Normal,
                )
            }
            if (isActive || (isOffered && active == null)) {
                Text(
                    route.detail,
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(start = 15.dp),
                )
            }
        }
    }
}

@Composable
private fun VolumeSlider(volume: Float, onChange: (Float) -> Unit) {
    Column {
        Text(
            "Volume: ${(volume * 100).roundToInt()}%",
            style = MaterialTheme.typography.titleSmall,
            fontWeight = FontWeight.SemiBold,
        )
        Text(
            "Applies to this stream only, on top of the phone's own volume.",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Slider(value = volume, onValueChange = onChange, valueRange = 0f..1f)
    }
}

@Composable
private fun BufferSlider(bufferMs: Int, enabled: Boolean, onChange: (Int) -> Unit) {
    Column {
        Text(
            "Jitter buffer: $bufferMs ms",
            style = MaterialTheme.typography.titleSmall,
            fontWeight = FontWeight.SemiBold,
        )
        Text(
            if (enabled) "Lower is tighter. Raise it if underruns climb."
            else "Disconnect to change — it is fixed for the length of a session.",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Slider(
            value = bufferMs.toFloat(),
            onValueChange = { onChange(it.roundToInt()) },
            valueRange = Prefs.MIN_BUFFER_MS.toFloat()..Prefs.MAX_BUFFER_MS.toFloat(),
            steps = (Prefs.MAX_BUFFER_MS - Prefs.MIN_BUFFER_MS) / 10 - 1,
            enabled = enabled,
        )
    }
}
