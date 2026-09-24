package dev.tethertone.app

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.media.projection.MediaProjectionManager
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.tethertone.shared.BridgeState
import dev.tethertone.shared.Pairing
import dev.tethertone.shared.PairingInfo
import dev.tethertone.shared.StreamMode
import dev.tethertone.shared.UpSource

class MainActivity : ComponentActivity() {

    private var pendingDeepLink by mutableStateOf<PairingInfo?>(null)
    private var cameraGranted by mutableStateOf(false)

    private val requestCamera = registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        cameraGranted = granted
    }
    private val requestNotifications =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        BridgeController.init(this)

        cameraGranted = ContextCompat.checkSelfPermission(this, Manifest.permission.CAMERA) ==
            PackageManager.PERMISSION_GRANTED

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            // Without this the foreground service still runs, but silently and
            // with no way to stop it from outside the app.
            requestNotifications.launch(Manifest.permission.POST_NOTIFICATIONS)
        }

        handleDeepLink(intent)

        setContent {
            TethertoneTheme {
                Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
                    AppRoot(
                        cameraGranted = cameraGranted,
                        onRequestCamera = { requestCamera.launch(Manifest.permission.CAMERA) },
                        deepLink = pendingDeepLink,
                        onDeepLinkConsumed = { pendingDeepLink = null },
                    )
                }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleDeepLink(intent)
    }

    private fun handleDeepLink(intent: Intent?) {
        val data = intent?.data ?: return
        if (!data.scheme.equals(Pairing.SCHEME, ignoreCase = true)) return
        pendingDeepLink = Pairing.parse(data.toString())

        // A consumed pairing link must not survive in the Activity's intent.
        // This is a singleTask activity, so the launch intent is handed back on
        // every recreation — leaving it in place makes the link fire again and
        // silently reconnect a session the user had just stopped.
        setIntent(Intent(Intent.ACTION_MAIN))
    }
}

private enum class Screen { Home, Scan, Settings }

@Composable
private fun AppRoot(
    cameraGranted: Boolean,
    onRequestCamera: () -> Unit,
    deepLink: PairingInfo?,
    onDeepLinkConsumed: () -> Unit,
) {
    val context = LocalContext.current
    val prefs = remember { Prefs(context) }

    var screen by remember { mutableStateOf(Screen.Home) }
    var showManual by remember { mutableStateOf(false) }
    var pairing by remember { mutableStateOf(prefs.pairing) }
    var bufferMs by remember { mutableIntStateOf(prefs.bufferMs) }
    var volume by remember { mutableStateOf(prefs.volume) }
    var autoReconnect by remember { mutableStateOf(prefs.autoReconnect) }
    var preferLowLatency by remember { mutableStateOf(prefs.preferLowLatency) }

    val state by BridgeController.state.collectAsStateWithLifecycle()
    val stats by BridgeController.stats.collectAsStateWithLifecycle()
    val level by BridgeController.level.collectAsStateWithLifecycle()
    val mode by BridgeController.mode.collectAsStateWithLifecycle()
    val uplink by BridgeController.uplinkStatus.collectAsStateWithLifecycle()
    val upLevel by BridgeController.upLevel.collectAsStateWithLifecycle()
    val live = state is BridgeState.Connected

    // Two-way audio needs up to two grants, both only askable from the screen:
    // the microphone permission, and a capture consent for the phone's audio
    // (a fresh one every session — Android does not let it be kept).
    var pendingMode by remember { mutableStateOf<StreamMode?>(null) }
    var directionNote by remember { mutableStateOf<String?>(null) }

    val captureLauncher = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        val data = result.data
        if (result.resultCode == Activity.RESULT_OK && data != null) {
            TethertoneService.acceptProjection(context, result.resultCode, data)
            directionNote = null
        } else {
            directionNote = "Capture wasn't allowed, so the phone's audio can't be sent."
        }
    }
    fun askForCapture() {
        val manager = context.getSystemService(MediaProjectionManager::class.java)
        captureLauncher.launch(manager.createScreenCaptureIntent())
    }

    fun applyMode(next: StreamMode) {
        directionNote = null
        BridgeController.setMode(next)
        if (live) {
            // Claim the microphone service type now the permission exists.
            TethertoneService.refresh(context)
            if (next.up == UpSource.PLAYBACK && Capture.projection == null) askForCapture()
        }
    }

    val micLauncher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        val next = pendingMode
        pendingMode = null
        when {
            next == null -> Unit
            granted -> applyMode(next)
            else -> directionNote = "Tethertone needs microphone access to send audio to the Mac."
        }
    }

    fun onModeChange(next: StreamMode) {
        if (next.up != UpSource.OFF && !Capture.micPermission(context)) {
            // Playback capture needs RECORD_AUDIO too.
            pendingMode = next
            micLauncher.launch(Manifest.permission.RECORD_AUDIO)
        } else {
            applyMode(next)
        }
    }

    fun connect(info: PairingInfo) {
        pairing = info
        prefs.pairing = info
        TethertoneService.start(context, info.toUri())
    }

    LaunchedEffect(deepLink) {
        val info = deepLink ?: return@LaunchedEffect
        onDeepLinkConsumed()
        connect(info)
        screen = Screen.Home
    }

    BackHandler(enabled = screen != Screen.Home) { screen = Screen.Home }

    if (showManual) {
        ManualConnectDialog(
            initial = pairing,
            onDismiss = { showManual = false },
            onConnect = { info ->
                showManual = false
                screen = Screen.Home
                connect(info)
            },
        )
    }

    when (screen) {
        Screen.Home -> HomeScreen(
            state = state,
            stats = stats,
            level = level,
            pairing = pairing,
            bufferMs = bufferMs,
            onBufferMsChange = { bufferMs = it; prefs.bufferMs = it },
            volume = volume,
            onVolumeChange = {
                volume = it
                prefs.volume = it
                // Live: the sink's gain is changed under the running stream.
                BridgeController.updateConfig(prefs.toConfig())
            },
            onScan = { screen = Screen.Scan },
            onManual = { showManual = true },
            onSettings = { screen = Screen.Settings },
            onConnect = { pairing?.let(::connect) },
            onDisconnect = { TethertoneService.stop(context) },
            onForget = {
                prefs.pairing = null
                pairing = null
            },
            modifier = Modifier.safeDrawingPadding(),
            direction = {
                DirectionCard(
                    mode = mode,
                    onModeChange = ::onModeChange,
                    uplink = uplink,
                    upLevel = upLevel,
                    connected = live,
                    playbackSupported = Capture.playbackSupported,
                    needsCaptureConsent = live && mode.up == UpSource.PLAYBACK && Capture.projection == null,
                    onAllowCapture = ::askForCapture,
                    note = directionNote,
                )
            },
        )

        Screen.Scan -> ScanScreen(
            cameraGranted = cameraGranted,
            onRequestCamera = onRequestCamera,
            onCancel = { screen = Screen.Home },
            onManual = { showManual = true },
            onPaired = { info ->
                screen = Screen.Home
                connect(info)
            },
        )

        Screen.Settings -> SettingsScreen(
            autoReconnect = autoReconnect,
            onAutoReconnectChange = {
                autoReconnect = it
                prefs.autoReconnect = it
                BridgeController.updateConfig(prefs.toConfig())
            },
            preferLowLatency = preferLowLatency,
            onPreferLowLatencyChange = {
                preferLowLatency = it
                prefs.preferLowLatency = it
            },
            driftCorrectionActive = (state is BridgeState.Connected) && stats.driftCorrectionActive,
            onBack = { screen = Screen.Home },
            modifier = Modifier.safeDrawingPadding(),
        )
    }
}
