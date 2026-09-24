package dev.tethertone.app

import android.Manifest
import android.annotation.SuppressLint
import android.content.Context
import android.content.pm.PackageManager
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioPlaybackCaptureConfiguration
import android.media.AudioRecord
import android.media.MediaRecorder
import android.media.audiofx.AcousticEchoCanceler
import android.media.projection.MediaProjection
import android.os.Build
import android.util.Log
import androidx.core.content.ContextCompat
import dev.tethertone.shared.UpSource
import dev.tethertone.shared.UplinkProvider
import dev.tethertone.shared.UplinkResult
import dev.tethertone.shared.UplinkSource

/**
 * What the phone is currently allowed to capture. Android ties both kinds of
 * capture to the foreground service having the matching type, which can only
 * be added while the app is on screen — so this is written by
 * [TethertoneService] and read when a mode is applied, possibly remotely.
 */
object Capture {
    @Volatile var micServiceType = false
    @Volatile var projection: MediaProjection? = null

    fun micPermission(context: Context): Boolean =
        ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) ==
            PackageManager.PERMISSION_GRANTED

    val playbackSupported: Boolean get() = Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q
}

/**
 * Opens the phone's microphone or playback capture for [BridgeClient], or says
 * — in words the Mac shows its user — why it can't right now.
 */
class AndroidUplinkProvider(private val context: Context) : UplinkProvider {

    /** Null if [source] can be opened now; otherwise the reason. Cheap: nothing is opened. */
    fun preflight(source: UpSource): String? = when (source) {
        UpSource.OFF -> null
        UpSource.MIC -> when {
            !Capture.micPermission(context) -> "Allow microphone access in Tethertone on the phone first."
            !Capture.micServiceType -> "Open Tethertone on the phone to turn its microphone on."
            else -> null
        }
        UpSource.PLAYBACK -> when {
            !Capture.playbackSupported -> "Capturing the phone's audio needs Android 10 or later."
            !Capture.micPermission(context) -> "Allow microphone access in Tethertone on the phone first."
            Capture.projection == null -> "Open Tethertone on the phone and allow capturing its audio."
            else -> null
        }
    }

    override fun open(source: UpSource, alsoPlaying: Boolean): UplinkResult {
        preflight(source)?.let { return UplinkResult.Unavailable(it) }
        return try {
            when (source) {
                UpSource.MIC -> UplinkResult.Opened(MicUplink(echoCancel = alsoPlaying))
                UpSource.PLAYBACK -> {
                    val projection = Capture.projection
                        ?: return UplinkResult.Unavailable("Allow capturing the phone's audio first.")
                    UplinkResult.Opened(PlaybackUplink(projection))
                }
                UpSource.OFF -> UplinkResult.Unavailable("Nothing to send.")
            }
        } catch (e: Exception) {
            Log.w(TAG, "could not open $source", e)
            UplinkResult.Unavailable(e.message ?: "The phone refused to start capturing audio.")
        }
    }

    private companion object { const val TAG = "TethertoneUplink" }
}

private const val RATE = 48000

/** Enough for 40 ms, so a scheduling hiccup does not overrun the recorder. */
private fun bufferBytes(channelMask: Int, channels: Int) = maxOf(
    AudioRecord.getMinBufferSize(RATE, channelMask, AudioFormat.ENCODING_PCM_16BIT),
    RATE / 25 * channels * 2,
)

/**
 * The phone's microphone. With the Mac's audio also playing on the phone, it
 * uses VOICE_COMMUNICATION — the source Android applies echo cancellation to —
 * plus the explicit canceller where one exists; otherwise the plain MIC source,
 * which keeps more of the room's sound.
 */
@SuppressLint("MissingPermission") // checked by AndroidUplinkProvider.preflight
private class MicUplink(echoCancel: Boolean) : UplinkSource {
    override val sampleRate = RATE
    override val channels = 1

    private val record = AudioRecord(
        if (echoCancel) MediaRecorder.AudioSource.VOICE_COMMUNICATION else MediaRecorder.AudioSource.MIC,
        RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT,
        bufferBytes(AudioFormat.CHANNEL_IN_MONO, 1),
    ).also { check(it.state == AudioRecord.STATE_INITIALIZED) { "The microphone is in use by another app." } }

    private val canceller: AcousticEchoCanceler? =
        if (echoCancel && AcousticEchoCanceler.isAvailable()) {
            AcousticEchoCanceler.create(record.audioSessionId)?.apply { enabled = true }
        } else null

    override fun start() = record.startRecording()
    override fun read(dest: ByteArray, offset: Int, length: Int) = record.read(dest, offset, length)
    override fun stop() {
        runCatching { record.stop() }
        canceller?.release()
        record.release()
    }
}

/**
 * Audio other apps are playing, via MediaProjection (Android 10+). Tethertone's
 * own stream is marked uncapturable in the sink, so the Mac's audio can never
 * be sent straight back to it. Apps may opt out of capture, and calls never
 * can be captured — both are the platform's rules.
 */
@SuppressLint("MissingPermission", "NewApi")
private class PlaybackUplink(projection: MediaProjection) : UplinkSource {
    override val sampleRate = RATE
    override val channels = 2

    private val record: AudioRecord = AudioRecord.Builder()
        .setAudioFormat(
            AudioFormat.Builder()
                .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                .setSampleRate(RATE)
                .setChannelMask(AudioFormat.CHANNEL_IN_STEREO)
                .build()
        )
        .setBufferSizeInBytes(bufferBytes(AudioFormat.CHANNEL_IN_STEREO, 2))
        .setAudioPlaybackCaptureConfig(
            AudioPlaybackCaptureConfiguration.Builder(projection)
                .addMatchingUsage(AudioAttributes.USAGE_MEDIA)
                .addMatchingUsage(AudioAttributes.USAGE_GAME)
                .addMatchingUsage(AudioAttributes.USAGE_UNKNOWN)
                .build()
        )
        .build()
        .also { check(it.state == AudioRecord.STATE_INITIALIZED) { "The phone refused to capture its audio." } }

    override fun start() = record.startRecording()
    override fun read(dest: ByteArray, offset: Int, length: Int) = record.read(dest, offset, length)
    override fun stop() {
        runCatching { record.stop() }
        record.release()
    }
}
