package dev.tethertone.app

import android.content.Context
import android.os.Build
import dev.tethertone.shared.BridgeClient
import dev.tethertone.shared.BridgeConfig
import dev.tethertone.shared.BridgeState
import dev.tethertone.shared.BridgeStats
import dev.tethertone.shared.PairingInfo
import dev.tethertone.shared.StreamMode
import dev.tethertone.shared.UplinkStatus
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.plus

/**
 * One connection per process, owned outside the Activity and outside the
 * Service. The Service exists to keep the process alive and put up a
 * notification; the UI only observes. Neither owns the socket, so a rotation or
 * a notification tap cannot interrupt playback.
 */
object BridgeController {

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    private val _state = MutableStateFlow<BridgeState>(BridgeState.Idle)
    val state: StateFlow<BridgeState> = _state.asStateFlow()

    private val _stats = MutableStateFlow(BridgeStats())
    val stats: StateFlow<BridgeStats> = _stats.asStateFlow()

    private val _level = MutableStateFlow(0f)
    val level: StateFlow<Float> = _level.asStateFlow()

    /** Which way audio flows. Mirrors the saved choice while disconnected. */
    private val _mode = MutableStateFlow(StreamMode())
    val mode: StateFlow<StreamMode> = _mode.asStateFlow()

    private val _uplink = MutableStateFlow(UplinkStatus())
    val uplinkStatus: StateFlow<UplinkStatus> = _uplink.asStateFlow()

    private val _upLevel = MutableStateFlow(0f)
    val upLevel: StateFlow<Float> = _upLevel.asStateFlow()

    private val _serverDuplex = MutableStateFlow<Boolean?>(null)
    /** Whether the connected Mac can take audio back; null while not connected. */
    val serverSupportsDuplex: StateFlow<Boolean?> = _serverDuplex.asStateFlow()

    private var client: BridgeClient? = null
    private var mirror: Job? = null
    private var prefs: Prefs? = null

    val deviceName: String = "${Build.MANUFACTURER} ${Build.MODEL}".trim()

    /** Loads the saved mode. Idempotent; call from anything that starts first. */
    fun init(context: Context) {
        if (prefs != null) return
        prefs = Prefs(context.applicationContext).also { _mode.value = it.mode }
    }

    fun connect(context: Context, info: PairingInfo, config: BridgeConfig) {
        disconnect()
        init(context)
        val provider = AndroidUplinkProvider(context.applicationContext)
        val fresh = BridgeClient(scope, deviceName, config, provider)
        // The Mac can ask for another mode; the phone accepts only what it can
        // do without its screen, and says why otherwise.
        fresh.onModeRequested = { requested -> provider.preflight(requested.up) }
        fresh.setMode(_mode.value)
        client = fresh
        mirror = scope.launch {
            launch { fresh.state.collect { _state.value = it } }
            launch { fresh.stats.collect { _stats.value = it } }
            launch { fresh.level.collect { _level.value = it } }
            launch { fresh.uplinkStatus.collect { _uplink.value = it } }
            launch { fresh.upLevel.collect { _upLevel.value = it } }
            launch { fresh.serverSupportsDuplex.collect { _serverDuplex.value = it } }
            // Also changed by the Mac: remember whatever was applied last.
            launch { fresh.mode.collect { _mode.value = it; prefs?.mode = it } }
        }
        fresh.connect(info)
    }

    /** Change direction: saved, and applied live when connected. */
    fun setMode(mode: StreamMode) {
        if (!mode.isValid) return
        _mode.value = mode
        prefs?.mode = mode
        client?.setMode(mode)
    }

    /** Re-apply the current mode after a permission or capture grant changed what is possible. */
    fun reapply() {
        client?.setMode(_mode.value)
    }

    /** Volume applies at once; buffer and latency settings take effect on the next connect. */
    fun updateConfig(config: BridgeConfig) {
        client?.updateConfig(config)
    }

    fun disconnect() {
        client?.disconnect()
        client = null
        mirror?.cancel()
        mirror = null
        _state.value = BridgeState.Idle
        _stats.value = BridgeStats()
        _level.value = 0f
        _uplink.value = UplinkStatus()
        _upLevel.value = 0f
        _serverDuplex.value = null
    }
}
