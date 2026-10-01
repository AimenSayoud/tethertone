package dev.tethertone.shared

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioTrack
import android.os.Build
import android.util.Log
import kotlin.math.roundToInt

private const val TAG = "TethertoneSink"

/**
 * AudioTrack in streaming mode. This is the single biggest win over the browser
 * receiver: a real low-latency output path, no Web Audio scheduling, and it
 * keeps running with the screen off as long as a foreground service holds it.
 */
internal class AudioTrackSink(
    private val sampleRate: Int,
    private val channels: Int,
    targetMs: Int,
    private val preferLowLatency: Boolean,
) : AudioSink {

    private val channelMask = when (channels) {
        1 -> AudioFormat.CHANNEL_OUT_MONO
        2 -> AudioFormat.CHANNEL_OUT_STEREO
        else -> AudioFormat.CHANNEL_OUT_STEREO
    }

    private val format = AudioFormat.Builder()
        .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
        .setSampleRate(sampleRate)
        .setChannelMask(channelMask)
        .build()

    private val minBuffer = AudioTrack.getMinBufferSize(sampleRate, channelMask, AudioFormat.ENCODING_PCM_16BIT)
        .coerceAtLeast(1024)

    /**
     * The device buffer only has to cover the gap between playback-loop wakeups;
     * the jitter buffer upstream is what absorbs the network. Making this large
     * would silently add latency nothing can claw back, so it stays near the
     * hardware minimum.
     */
    private val deviceBuffer = maxOf(minBuffer, bytesForMs(targetMs / 2))

    private var track: AudioTrack? = null
    private var speedFailed = false

    /** Documented ceiling for setPlaybackRate: twice the device's native rate. */
    private val maxPlaybackRate =
        2 * AudioTrack.getNativeOutputSampleRate(AudioManager.STREAM_MUSIC).coerceAtLeast(sampleRate)

    override val bytesPerWrite: Int = bytesForMs(10).coerceAtMost(deviceBuffer)

    private fun bytesForMs(ms: Int): Int {
        val frames = sampleRate * ms / 1000
        return frames * channels * 2
    }

    /**
     * Builds the track twice when it has to.
     *
     * PERFORMANCE_MODE_LOW_LATENCY is worth a few milliseconds, but on some
     * devices (a Galaxy S22 among them) a low-latency track refuses both
     * setPlaybackParams and setPlaybackRate, which leaves drift correction with
     * nothing to drive. Losing smooth correction means the buffer can only be
     * brought back by discarding audio — an audible click every few minutes —
     * and that is a worse trade than a few ms of latency. So: probe rate
     * control on the low-latency track, and if it is refused, rebuild without
     * low-latency mode and keep the correction.
     */
    override fun start() {
        var created = build(lowLatency = preferLowLatency)
        var rateControl = probeRateControl(created)
        var mode = "low-latency"

        if (!rateControl && preferLowLatency) {
            runCatching { created.release() }
            created = build(lowLatency = false)
            rateControl = probeRateControl(created)
            mode = if (rateControl) "standard (traded for drift correction)" else "standard"
        } else if (!preferLowLatency) {
            mode = "standard"
        }

        speedFailed = !rateControl
        created.play()
        track = created
        Log.i(TAG, "AudioTrack up: ${sampleRate}Hz ${channels}ch deviceBuffer=$deviceBuffer " +
                "(min=$minBuffer) write=$bytesPerWrite mode=$mode driftCorrection=$rateControl")
    }

    private fun build(lowLatency: Boolean): AudioTrack {
        val attributes = AudioAttributes.Builder()
            .setUsage(AudioAttributes.USAGE_MEDIA)
            .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
            .apply {
                // The Mac's stream must never be captured back: with "Both"
                // and playback capture on, it would loop straight back to it.
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    setAllowedCapturePolicy(AudioAttributes.ALLOW_CAPTURE_BY_NONE)
                }
            }
            .build()

        val builder = AudioTrack.Builder()
            .setAudioAttributes(attributes)
            .setAudioFormat(format)
            .setBufferSizeInBytes(deviceBuffer)
            .setTransferMode(AudioTrack.MODE_STREAM)

        if (lowLatency && Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            builder.setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY)
        }

        val created = builder.build()
        if (created.state != AudioTrack.STATE_INITIALIZED) {
            created.release()
            error("AudioTrack failed to initialise at $sampleRate Hz / ${channels}ch")
        }
        return created
    }

    /** Ask for a rate one hertz off, then put it back. Cheap, and only the answer matters. */
    private fun probeRateControl(candidate: AudioTrack): Boolean = try {
        val ok = candidate.setPlaybackRate(sampleRate + 1) == AudioTrack.SUCCESS
        if (ok) candidate.setPlaybackRate(sampleRate)
        ok
    } catch (e: IllegalStateException) {
        false
    }

    override fun onPlaybackThread() {
        runCatching {
            android.os.Process.setThreadPriority(android.os.Process.THREAD_PRIORITY_URGENT_AUDIO)
        }.onFailure { Log.w(TAG, "could not raise playback thread priority", it) }
    }

    override fun setVolume(volume: Float) {
        runCatching { track?.setVolume(volume.coerceIn(0f, 1f)) }
    }

    override fun write(data: ByteArray, offset: Int, length: Int): Int =
        track?.write(data, offset, length, AudioTrack.WRITE_BLOCKING) ?: -1

    /**
     * Drift correction via [AudioTrack.setPlaybackRate], not [AudioTrack.setPlaybackParams].
     *
     * setPlaybackParams is the modern API and it time-stretches rather than
     * repitching — but a track built with PERFORMANCE_MODE_LOW_LATENCY rejects
     * it outright ("arguments out of range"), and low latency is the entire
     * point of this app. setPlaybackRate changes the resampling ratio instead,
     * works in low-latency mode, and is exactly the right shape for the job:
     * correcting clock drift *is* asking for a slightly different sample rate.
     * The pitch shift it costs at the +/-0.2% this is ever asked for is about
     * three cents, which is inaudible.
     */
    override fun setSpeed(speed: Float): Boolean {
        if (speedFailed) return false
        val t = track ?: return false
        val requested = (sampleRate * speed).roundToInt().coerceIn(1, maxPlaybackRate)
        return try {
            if (t.setPlaybackRate(requested) == AudioTrack.SUCCESS) {
                true
            } else {
                Log.w(TAG, "setPlaybackRate($requested) refused; drift correction disabled")
                speedFailed = true
                false
            }
        } catch (e: IllegalStateException) {
            Log.w(TAG, "playback rate unsupported here; drift correction disabled", e)
            speedFailed = true
            false
        }
    }

    override fun stop() {
        runCatching { track?.pause(); track?.flush(); track?.stop() }
    }

    override fun release() {
        runCatching { track?.release() }
        track = null
    }
}
