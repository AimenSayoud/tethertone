package dev.tethertone.desktop

import dev.tethertone.shared.BridgeClient
import dev.tethertone.shared.BridgeConfig
import dev.tethertone.shared.BridgeState
import dev.tethertone.shared.Pairing
import dev.tethertone.shared.PairingInfo
import dev.tethertone.shared.StreamMode
import dev.tethertone.shared.UpSource
import dev.tethertone.shared.UplinkProvider
import dev.tethertone.shared.UplinkResult
import dev.tethertone.shared.UplinkSource
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import java.io.File

/**
 * Headless sink: the same BridgeClient the Android app runs, playing through
 * javax.sound. If this works and the phone does not, the problem is Android.
 *
 *   ./gradlew :desktopSink:run --args="tethertone://p?..."
 *   ./gradlew :desktopSink:run --args="--seconds 10"
 *   ./gradlew :desktopSink:run --args="--up-tone"            also send a test tone up (duplex)
 *   ./gradlew :desktopSink:run --args="--up-tone --no-down"  send only
 */
fun main(args: Array<String>) = runBlocking {
    var uri: String? = null
    var seconds = 15L
    var host = "127.0.0.1"
    var port = 45678
    var upTone = false
    var down = true

    var i = 0
    while (i < args.size) {
        when (args[i]) {
            "--seconds" -> seconds = args[++i].toLong()
            "--host" -> host = args[++i]
            "--port" -> port = args[++i].toInt()
            "--up-tone" -> upTone = true
            "--no-down" -> down = false
            else -> uri = args[i]
        }
        i++
    }

    val info: PairingInfo = uri?.let(Pairing::parse) ?: run {
        val tokenFile = File(System.getProperty("user.home"), ".tethertone/token")
        val token = if (tokenFile.exists()) tokenFile.readText().trim() else ""
        PairingInfo(listOf(host), port, token, name = "local")
    }

    println("[sink] hosts=${info.hosts} port=${info.port} token=${if (info.token.isEmpty()) "<none>" else "set"}")

    val scope = CoroutineScope(SupervisorJob())
    val uplink = UplinkProvider { source, _ ->
        if (source == UpSource.OFF) UplinkResult.Unavailable("nothing to send")
        else UplinkResult.Opened(ToneSource())
    }
    val client = BridgeClient(scope, "desktopSink", BridgeConfig(autoReconnect = false), uplink)
    client.setMode(StreamMode(down = down, up = if (upTone) UpSource.MIC else UpSource.OFF))
    client.connect(info)

    var settled = false
    repeat(seconds.toInt()) {
        delay(1000)
        when (val state = client.state.value) {
            is BridgeState.Connected -> {
                settled = true
                val s = client.stats.value
                println(
                    "[sink] ${state.serverName}@${state.host} ${state.rate}Hz/${state.channels}ch  " +
                        "buf=${s.bufferMs}ms under=${s.underruns} over=${s.overruns} " +
                        "lost=${s.lostFrames} speed=${s.speed} ${s.kbps}kbit/s t=${s.secondsConnected}s"
                )
                val up = client.uplinkStatus.value
                if (up.source != UpSource.OFF || up.problem != null) {
                    println("[sink]   up: ${if (up.active) "sending ${up.kbps}kbit/s" else "stopped"}" +
                        (up.problem?.let { " — $it" } ?: "") + "  duplex=${client.serverSupportsDuplex.value}")
                }
            }
            is BridgeState.Failed -> {
                println("[sink] FAILED: ${state.reason}")
                client.disconnect()
                return@runBlocking
            }
            is BridgeState.Connecting -> println("[sink] connecting to ${state.detail}...")
            is BridgeState.Reconnecting ->
                println("[sink] reconnecting (attempt ${state.attempt}) in ${state.inSeconds}s: ${state.reason}")
            BridgeState.Idle -> println("[sink] idle")
        }
    }

    val final = client.stats.value
    client.disconnect()
    println(if (settled && final.secondsConnected > 0) "[sink] PASS" else "[sink] FAIL")
}

/**
 * A quiet 440 Hz tone, paced to real time like a microphone would be, so the
 * up stream can be tested end to end without a phone.
 */
private class ToneSource(override val sampleRate: Int = 48000) : UplinkSource {
    override val channels = 1
    private var phase = 0.0
    private var nextAt = 0L

    override fun start() { nextAt = System.nanoTime() }

    override fun read(dest: ByteArray, offset: Int, length: Int): Int {
        val frames = length / 2
        // Block until this chunk would have been captured.
        nextAt += frames * 1_000_000_000L / sampleRate
        val wait = nextAt - System.nanoTime()
        if (wait > 0) Thread.sleep(wait / 1_000_000, (wait % 1_000_000).toInt())
        for (i in 0 until frames) {
            val sample = (kotlin.math.sin(phase) * 3000).toInt()
            phase += 2 * Math.PI * 440 / sampleRate
            dest[offset + i * 2] = sample.toByte()
            dest[offset + i * 2 + 1] = (sample shr 8).toByte()
        }
        return frames * 2
    }

    override fun stop() = Unit
}
