package dev.tethertone.shared

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.joinAll
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlin.math.abs
import kotlin.math.min

sealed interface BridgeState {
    data object Idle : BridgeState
    data class Connecting(val detail: String) : BridgeState
    data class Connected(val serverName: String, val host: String, val rate: Int, val channels: Int) : BridgeState
    /** Lost the connection and waiting to try again. [reason] is why it dropped. */
    data class Reconnecting(val attempt: Int, val reason: String, val inSeconds: Int) : BridgeState
    data class Failed(val reason: String) : BridgeState
}

data class BridgeStats(
    val bufferMs: Int = 0,
    val underruns: Int = 0,
    val overruns: Int = 0,
    val lostFrames: Long = 0,
    val speed: Float = 1.0f,
    val kbps: Int = 0,
    val secondsConnected: Long = 0,
    val driftCorrectionActive: Boolean = false,
)

data class BridgeConfig(
    val targetBufferMs: Int = DEFAULT_TARGET_MS,
    val autoReconnect: Boolean = true,
    /**
     * Ask for AudioTrack's low-latency path. Devices that grant it often refuse
     * playback-rate control in exchange, which costs smooth drift correction —
     * so this is off by default and the sink falls back on its own.
     */
    val preferLowLatency: Boolean = false,
    val volume: Float = 1.0f,
) {
    companion object {
        const val DEFAULT_TARGET_MS = 90
    }
}

/**
 * Owns one connection for its whole life: races the candidate hosts, does the
 * handshake, runs a reader and a playback loop, and — unless told otherwise —
 * gets back up when the link drops.
 *
 * With a duplex-capable server it can also send audio up ([UplinkProvider])
 * and switch direction live (PROTOCOL.md §6). The mode belongs to this side:
 * the server can only ask for a change, through [onModeRequested].
 *
 * Nothing here is Android-specific. [createAudioSink], [openStream] and
 * [UplinkProvider] are the only seams, which is what lets the desktop sink run
 * the identical code.
 */
class BridgeClient(
    private val scope: CoroutineScope,
    private val deviceName: String,
    config: BridgeConfig = BridgeConfig(),
    private val uplink: UplinkProvider? = null,
    private val clock: () -> Long = ::monotonicMillis,
) {
    private val json = Json { ignoreUnknownKeys = true; encodeDefaults = true }
    private val controlJson = Json { ignoreUnknownKeys = true; explicitNulls = false }

    private val _mode = MutableStateFlow(StreamMode())
    /** The mode the user asked for. What actually runs is reported by [uplinkStatus]. */
    val mode: StateFlow<StreamMode> = _mode.asStateFlow()

    private val _uplink = MutableStateFlow(UplinkStatus())
    val uplinkStatus: StateFlow<UplinkStatus> = _uplink.asStateFlow()

    private val _upLevel = MutableStateFlow(0f)
    /** Peak level of the audio being sent up, 0..1. */
    val upLevel: StateFlow<Float> = _upLevel.asStateFlow()

    private val _serverDuplex = MutableStateFlow<Boolean?>(null)
    /** Whether the connected server can take audio back. Null while not connected. */
    val serverSupportsDuplex: StateFlow<Boolean?> = _serverDuplex.asStateFlow()

    /**
     * The server asked for [StreamMode]. Return null to accept it, or the reason
     * it cannot be done right now (shown on the computer). Called off the main
     * thread. When unset, a request is accepted if an [UplinkProvider] exists.
     */
    var onModeRequested: ((StreamMode) -> String?)? = null

    @kotlin.concurrent.Volatile
    private var current: Session? = null

    private val _state = MutableStateFlow<BridgeState>(BridgeState.Idle)
    val state: StateFlow<BridgeState> = _state.asStateFlow()

    private val _stats = MutableStateFlow(BridgeStats())
    val stats: StateFlow<BridgeStats> = _stats.asStateFlow()

    /** Peak level, 0..1, updated fast enough to drive a meter. */
    private val _level = MutableStateFlow(0f)
    val level: StateFlow<Float> = _level.asStateFlow()

    private var config = config
    private var job: Job? = null
    private var activeSink: AudioSink? = null

    fun connect(info: PairingInfo) {
        disconnect()
        job = scope.launch { supervise(info) }
    }

    fun disconnect() {
        job?.cancel()
        job = null
        activeSink = null
        _state.value = BridgeState.Idle
        _stats.value = BridgeStats()
        _level.value = 0f
        _upLevel.value = 0f
        _uplink.value = UplinkStatus()
        _serverDuplex.value = null
    }

    /**
     * Change direction. Applies live to a connected duplex server and is kept
     * for the next connection otherwise. An invalid mode (nothing either way)
     * is ignored.
     */
    fun setMode(mode: StreamMode) {
        if (!mode.isValid) return
        _mode.value = mode
        if (mode.up == UpSource.OFF) _uplink.value = UplinkStatus()
        current?.let { s -> s.scope.launch { applyMode(s) } }
    }

    /** Takes effect immediately; buffer and latency settings apply on the next connect. */
    fun updateConfig(update: BridgeConfig) {
        val volumeChanged = update.volume != config.volume
        config = update
        if (volumeChanged) activeSink?.setVolume(update.volume)
    }

    val isRunning: Boolean get() = job?.isActive == true

    /**
     * Reconnect loop. Backoff resets once a session has lasted long enough to
     * count as real, so an unplugged cable retries gently while a Mac that went
     * to sleep for an hour is still picked up the moment it comes back.
     */
    private suspend fun supervise(info: PairingInfo) {
        var attempt = 0
        while (currentCoroutineContext().isActive) {
            val startedAt = clock()
            val reason = runSession(info)
            val lasted = clock() - startedAt

            if (!config.autoReconnect) {
                _state.value = BridgeState.Failed(reason)
                return
            }
            if (lasted >= STABLE_SESSION_MS) attempt = 0
            attempt++

            val waitMs = min(RECONNECT_BASE_MS shl min(attempt - 1, 5), RECONNECT_MAX_MS)
            var remaining = waitMs
            while (remaining > 0 && currentCoroutineContext().isActive) {
                _state.value = BridgeState.Reconnecting(attempt, reason, ((remaining + 999) / 1000).toInt())
                val slice = min(remaining, 1000L)
                delay(slice)
                remaining -= slice
            }
        }
    }

    /** Runs one connection to completion. Returns why it ended. */
    private suspend fun runSession(info: PairingInfo): String {
        var stream: ByteStream? = null
        var sink: AudioSink? = null
        try {
            _state.value = BridgeState.Connecting(info.hosts.joinToString(", "))
            val (connected, welcome, host) = race(info)
            stream = connected

            if (welcome.fmt != "pcm_s16le") {
                return "server offered ${welcome.fmt}, only pcm_s16le is supported"
            }
            _state.value = BridgeState.Connected(
                serverName = welcome.name.ifEmpty { host },
                host = host, rate = welcome.rate, channels = welcome.ch,
            )
            val duplex = Proto.CAP_DUPLEX in welcome.caps
            _serverDuplex.value = duplex

            val jitter = JitterBuffer(
                welcome.rate, welcome.ch, config.targetBufferMs, config.targetBufferMs * 4
            )
            sink = createAudioSink(welcome.rate, welcome.ch, config.targetBufferMs, config.preferLowLatency)
            sink.start()
            sink.setVolume(config.volume)
            activeSink = sink

            coroutineScope {
                val session = Session(stream, jitter, sink, welcome, clock(), this, duplex)
                current = session
                launch { readLoop(session) }
                launch { playbackLoop(session) }
                launch { statsLoop(session) }
                launch { levelLoop(session) }
                if (duplex) {
                    launch { applyMode(session) }
                } else if (_mode.value.up != UpSource.OFF) {
                    _uplink.value = UplinkStatus(
                        source = _mode.value.up,
                        problem = "Tethertone on this computer can't receive audio yet. Update it to use two-way audio.",
                    )
                }
            }
            return "connection closed"
        } catch (e: CancellationException) {
            throw e
        } catch (e: Throwable) {
            return e.message ?: e::class.simpleName ?: "connection failed"
        } finally {
            current?.uplinkSource?.runCatching { stop() }
            current = null
            activeSink = null
            sink?.runCatching { stop(); release() }
            stream?.close()
            _level.value = 0f
            _upLevel.value = 0f
            _serverDuplex.value = null
            val up = _uplink.value
            if (up.active) _uplink.value = up.copy(active = false, kbps = 0)
        }
    }

    private class Session(
        val stream: ByteStream,
        val jitter: JitterBuffer,
        val sink: AudioSink,
        val welcome: Welcome,
        val startedAt: Long,
        val scope: CoroutineScope,
        val duplex: Boolean,
    ) {
        var expectedSeq = -1L
        var lost = 0L
        var bytesIn = 0L
        var speed = 1.0f
        var speedSupported = true

        /** Off when the mode stops the server sending to us; playback then idles on silence. */
        @kotlin.concurrent.Volatile var downEnabled = true
        @kotlin.concurrent.Volatile var upBytes = 0L
        var uplinkJob: Job? = null
        @kotlin.concurrent.Volatile var uplinkSource: UplinkSource? = null

        /** Frames are written from several coroutines; each must reach the socket whole. */
        private val writeLock = Mutex()
        /** Mode changes are applied one at a time, in the order they were asked for. */
        val modeLock = Mutex()

        suspend fun send(type: Int, payload: ByteArray, seq: Long = 0) = writeLock.withLock {
            stream.write(encodeHeader(type, 0, payload.size, seq) + payload)
        }

        private val peakLock = PlatformLock()
        private var peak = 0f
        private var upPeak = 0f

        fun reportPeak(value: Float) = peakLock.withLock { if (value > peak) peak = value }
        fun reportUpPeak(value: Float) = peakLock.withLock { if (value > upPeak) upPeak = value }

        fun takePeak(): Float = peakLock.withLock { val p = peak; peak = 0f; p }
        fun takeUpPeak(): Float = peakLock.withLock { val p = upPeak; upPeak = 0f; p }
    }

    // MARK: duplex

    private suspend fun sendControl(s: Session, message: ControlMessage) {
        s.send(Proto.CONTROL, controlJson.encodeToString(message).encodeToByteArray())
    }

    /**
     * Brings the session in line with [_mode]: stops or starts the up stream,
     * gates the down stream, and tells the server what is really running.
     */
    private suspend fun applyMode(s: Session) = s.modeLock.withLock {
        val wanted = _mode.value
        s.downEnabled = wanted.down || !s.duplex

        s.uplinkJob?.let { it.cancel(); it.join() }
        s.uplinkJob = null
        s.uplinkSource = null

        var running = UpSource.OFF
        var rate: Int? = null
        var channels: Int? = null

        if (wanted.up != UpSource.OFF) {
            val result = uplink?.open(wanted.up, wanted.down)
                ?: UplinkResult.Unavailable("This device can't send audio.")
            when (result) {
                is UplinkResult.Opened -> {
                    val source = result.source
                    running = wanted.up
                    rate = source.sampleRate
                    channels = source.channels
                    s.uplinkSource = source
                    s.upBytes = 0
                    _uplink.value = UplinkStatus(active = true, source = wanted.up)
                    s.uplinkJob = s.scope.launch { uplinkLoop(s, source) }
                }
                is UplinkResult.Unavailable ->
                    _uplink.value = UplinkStatus(source = wanted.up, problem = result.reason)
            }
        } else {
            _uplink.value = UplinkStatus()
        }

        val effective = StreamMode(down = s.downEnabled, up = running)
        sendControl(s, ControlMessage.mode(effective, rate, channels))
    }

    /**
     * Reads the capture source and sends it up in 10 ms blocks. A capture
     * failure ends the up stream, not the connection: the phone keeps playing
     * and the server is told the up stream stopped.
     */
    private suspend fun uplinkLoop(s: Session, source: UplinkSource) = withContext(Dispatchers.IO) {
        val block = source.sampleRate / 100 * source.channels * 2
        val buffer = ByteArray(block)
        var seq = 0L
        try {
            source.start()
            while (isActive) {
                var filled = 0
                while (filled < block) {
                    val n = source.read(buffer, filled, block - filled)
                    if (n < 0) throw IllegalStateException("audio capture failed ($n)")
                    filled += n
                }
                s.send(Proto.AUDIO, buffer, seq)
                seq = (seq + 1) and 0xFFFFFFFFL
                s.upBytes += block
                s.reportUpPeak(pcmPeakLe16(buffer, block))
            }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Throwable) {
            _uplink.value = UplinkStatus(source = _mode.value.up, problem = e.message ?: "audio capture failed")
            runCatching { sendControl(s, ControlMessage.mode(StreamMode(s.downEnabled, UpSource.OFF), null, null)) }
        } finally {
            source.runCatching { stop() }
            _upLevel.value = 0f
        }
    }

    private suspend fun handleControl(s: Session, payload: ByteArray) {
        val message = runCatching { controlJson.decodeFromString<ControlMessage>(payload.decodeToString()) }
            .getOrNull() ?: return
        when (message.t) {
            ControlMessage.SET_MODE -> {
                val requested = message.mode
                val refusal = when {
                    !requested.isValid -> "A connection needs audio going at least one way."
                    else -> onModeRequested?.invoke(requested)
                        ?: if (requested.up != UpSource.OFF && uplink == null) "This device can't send audio." else null
                }
                if (refusal == null) {
                    _mode.value = requested
                    s.scope.launch { applyMode(s) }
                } else {
                    sendControl(s, ControlMessage(t = ControlMessage.REFUSED, reason = refusal))
                }
            }
            ControlMessage.UP_DENIED -> {
                val reason = message.reason ?: "The computer isn't taking audio right now."
                _mode.value = StreamMode(down = true, up = UpSource.OFF)
                s.scope.launch {
                    applyMode(s)
                    _uplink.value = UplinkStatus(problem = reason)
                }
            }
        }
    }

    /**
     * Connect to every candidate at once, first complete handshake wins.
     *
     * Hosts are staggered rather than fired simultaneously: the server puts the
     * USB loopback first when a cable is plugged in, and a small head start is
     * enough for it to win without the Wi-Fi attempt having to lose a race it
     * would often win on raw latency.
     */
    private suspend fun race(info: PairingInfo): Triple<ByteStream, Welcome, String> = coroutineScope {
        val winner = CompletableDeferred<Triple<ByteStream, Welcome, String>>()
        val errorLock = PlatformLock()
        val errors = mutableListOf<String>()

        val attempts = info.hosts.mapIndexed { index, host ->
            launch {
                delay(index * HOST_STAGGER_MS)
                if (winner.isCompleted) return@launch
                var opened: ByteStream? = null
                try {
                    opened = openStream(host, info.port, CONNECT_TIMEOUT_MS)
                    val welcome = handshake(opened, info.token)
                    if (!welcome.ok) {
                        errorLock.withLock { errors += "$host: ${welcome.err ?: "rejected"}" }
                        opened.close()
                        return@launch
                    }
                    if (!winner.complete(Triple(opened, welcome, host))) opened.close()
                } catch (e: CancellationException) {
                    opened?.close()
                    throw e
                } catch (e: Throwable) {
                    opened?.close()
                    errorLock.withLock { errors += "$host: ${e.message ?: e::class.simpleName}" }
                }
            }
        }

        val watchdog = launch {
            attempts.joinAll()
            if (!winner.isCompleted) {
                val detail = errorLock.withLock { errors.joinToString("; ") }
                winner.completeExceptionally(
                    IllegalStateException(if (detail.isEmpty()) "no route to server" else detail)
                )
            }
        }

        try {
            winner.await()
        } finally {
            attempts.forEach { it.cancel() }
            watchdog.cancel()
        }
    }

    private suspend fun handshake(stream: ByteStream, token: String): Welcome {
        val hello = json.encodeToString(Hello(token = token, name = deviceName))
        stream.write((hello + "\n").encodeToByteArray())
        return json.decodeFromString<Welcome>(stream.readLine())
    }

    private suspend fun readLoop(s: Session) {
        val header = ByteArray(Proto.HEADER_SIZE)
        while (true) {
            s.stream.readFully(header, 0, Proto.HEADER_SIZE)
            val h = decodeHeader(header)
            val payload = if (h.length > 0) ByteArray(h.length).also { s.stream.readFully(it, 0, h.length) }
                          else EMPTY_PAYLOAD

            when (h.type) {
                Proto.AUDIO -> {
                    if (!s.downEnabled) continue
                    if (s.expectedSeq >= 0 && h.seq != s.expectedSeq) s.lost += seqGap(s.expectedSeq, h.seq)
                    s.expectedSeq = (h.seq + 1) and 0xFFFFFFFFL
                    s.bytesIn += h.length
                    s.reportPeak(pcmPeakLe16(payload, h.length))
                    s.jitter.push(payload)
                }
                Proto.PING -> {
                    s.send(Proto.PONG, payload, h.seq)
                    val telemetry = json.encodeToString(
                        ClientStats(s.jitter.bufferedMs, s.jitter.underruns, s.lost, s.speed)
                    ).encodeToByteArray()
                    s.send(Proto.STATS, telemetry)
                }
                Proto.CONTROL -> if (s.duplex) handleControl(s, payload)
                Proto.BYE -> throw IllegalStateException(
                    "server closed: " + (if (payload.isEmpty()) "no reason given" else payload.decodeToString())
                )
            }
        }
    }

    /**
     * The blocking [AudioSink.write] is the clock: this loop runs at exactly the
     * speed the phone's DAC consumes samples, and the speed correction nudges
     * that to match the Mac.
     */
    private suspend fun playbackLoop(s: Session) = withContext(Dispatchers.IO) {
        // write() blocks for as long as the device is full, which is the whole
        // design — but it makes this thread useless to anyone else, so it
        // belongs on IO rather than holding a Default worker hostage.
        s.sink.onPlaybackThread()

        // Prebuffer, or the first second is nothing but underruns.
        val deadline = clock() + PREBUFFER_TIMEOUT_MS
        while (s.jitter.bufferedMs < s.jitter.targetMs && clock() < deadline) delay(4)

        // Opening the audio device takes long enough that the server's queue and
        // the socket have already pushed a burst at us. Keeping it would spend
        // the whole session carrying that burst as latency, so it goes now,
        // while nothing is playing and dropping it cannot be heard.
        s.jitter.trimTo(s.jitter.targetMs, countAsOverrun = false)

        val buffer = ByteArray(s.sink.bytesPerWrite)
        var sinceCorrection = 0
        // The device buffer takes a moment to fill and the output to really
        // start, during which the sink consumes less than the server sends.
        // Speed correction would need a minute and a half to give that back at
        // 0.2%, so it is trimmed away instead while playback is still ramping.
        val settleUntil = clock() + SETTLE_MS

        var idle = false
        while (isActive) {
            // Down stream switched off: keep the device clocking on silence
            // rather than stopping it, so switching back on is instant. When it
            // returns, wait for a fresh buffer instead of starting on underruns.
            if (!s.downEnabled || (idle && s.jitter.bufferedMs < s.jitter.targetMs)) {
                if (!s.downEnabled && !idle) { idle = true; s.jitter.reset(); s.expectedSeq = -1 }
                buffer.fill(0)
                if (s.sink.write(buffer, 0, buffer.size) < 0) throw IllegalStateException("audio sink failed")
                continue
            }
            if (idle) { idle = false; s.jitter.trimTo(s.jitter.targetMs, countAsOverrun = false) }

            s.jitter.pull(buffer, buffer.size)
            val written = s.sink.write(buffer, 0, buffer.size)
            if (written < 0) throw IllegalStateException("audio sink failed ($written)")

            if (++sinceCorrection >= CORRECTION_INTERVAL_WRITES) {
                sinceCorrection = 0
                val settling = clock() < settleUntil
                val bufferedMs = s.jitter.bufferedMs
                val target = s.jitter.targetMs

                val mustTrim = when {
                    settling -> bufferedMs > target
                    // Beyond this, speed correction would take minutes. One
                    // click now beats carrying the latency for the session.
                    bufferedMs > target * HARD_TRIM_FACTOR -> true
                    // No rate control on this sink at all: trimming is the only
                    // mechanism left for giving latency back.
                    !s.speedSupported && bufferedMs > target * FALLBACK_TRIM_FACTOR -> true
                    else -> false
                }

                if (mustTrim) {
                    s.jitter.trimTo(target, countAsOverrun = !settling)
                } else if (s.speedSupported) {
                    val wanted = s.jitter.speedCorrection()
                    if (abs(wanted - s.speed) > 0.0001f) {
                        s.speedSupported = s.sink.setSpeed(wanted)
                        if (s.speedSupported) s.speed = wanted
                    }
                }
            }
        }
    }

    /** Fast enough for a meter to look alive, with a decay so peaks stay readable. */
    private suspend fun levelLoop(s: Session) {
        while (true) {
            delay(LEVEL_INTERVAL_MS)
            val peak = s.takePeak()
            _level.value = if (peak >= _level.value) peak else _level.value * LEVEL_DECAY
            val up = s.takeUpPeak()
            _upLevel.value = if (up >= _upLevel.value) up else _upLevel.value * LEVEL_DECAY
        }
    }

    private suspend fun statsLoop(s: Session) {
        var lastBytes = 0L
        var lastUpBytes = 0L
        var lastAt = clock()
        // Publish once up front. Waiting a full second would show a freshly
        // connected stream as zero everything, and the drift card would claim
        // correction was unavailable before anything had been measured.
        publishStats(s, s.startedAt, 0)
        while (true) {
            delay(1000)
            val now = clock()
            val elapsed = (now - lastAt).coerceAtLeast(1)
            val kbps = ((s.bytesIn - lastBytes) * 8 / elapsed).toInt()
            lastBytes = s.bytesIn
            val upKbps = ((s.upBytes - lastUpBytes) * 8 / elapsed).toInt()
            lastUpBytes = s.upBytes
            lastAt = now
            publishStats(s, now, kbps)
            val up = _uplink.value
            if (up.active) _uplink.value = up.copy(kbps = upKbps)
        }
    }

    private fun publishStats(s: Session, now: Long, kbps: Int) {
        _stats.value = BridgeStats(
            bufferMs = s.jitter.bufferedMs,
            underruns = s.jitter.underruns,
            overruns = s.jitter.overruns,
            lostFrames = s.lost,
            speed = s.speed,
            kbps = kbps,
            secondsConnected = (now - s.startedAt) / 1000,
            driftCorrectionActive = s.speedSupported,
        )
    }

    companion object {
        const val CONNECT_TIMEOUT_MS = 4000
        const val HOST_STAGGER_MS = 150L
        const val PREBUFFER_TIMEOUT_MS = 2000L
        const val CORRECTION_INTERVAL_WRITES = 20
        const val FALLBACK_TRIM_FACTOR = 2
        const val HARD_TRIM_FACTOR = 3
        const val SETTLE_MS = 2500L

        const val RECONNECT_BASE_MS = 1000L
        const val RECONNECT_MAX_MS = 15000L
        const val STABLE_SESSION_MS = 10_000L

        const val LEVEL_INTERVAL_MS = 50L
        const val LEVEL_DECAY = 0.72f

        private val EMPTY_PAYLOAD = ByteArray(0)
    }
}

expect fun monotonicMillis(): Long
