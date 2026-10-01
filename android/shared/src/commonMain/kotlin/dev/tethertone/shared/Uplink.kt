package dev.tethertone.shared

/**
 * Somewhere to get PCM to send up to the computer: the phone's microphone,
 * what its other apps are playing, or a test tone on the desktop sink.
 *
 * [read] blocks until a block of audio is available, which makes the capture
 * device the clock for the up stream the same way the sink is for the down one.
 */
interface UplinkSource {
    val sampleRate: Int
    val channels: Int

    fun start()

    /** Blocking. Fills up to [length] bytes of s16le PCM; returns bytes read, or negative on error. */
    fun read(dest: ByteArray, offset: Int, length: Int): Int

    fun stop()
}

/**
 * Opens an [UplinkSource] for a mode, or explains why it cannot.
 *
 * [alsoPlaying] is true when the down stream is on at the same time — a
 * microphone should then use echo cancellation, and playback capture must
 * leave out the stream the phone is itself playing.
 */
fun interface UplinkProvider {
    fun open(source: UpSource, alsoPlaying: Boolean): UplinkResult
}

sealed interface UplinkResult {
    data class Opened(val source: UplinkSource) : UplinkResult
    data class Unavailable(val reason: String) : UplinkResult
}

/** What the phone's UI shows about the up stream. */
data class UplinkStatus(
    val active: Boolean = false,
    val source: UpSource = UpSource.OFF,
    val kbps: Int = 0,
    /** Why the up stream is not running although it was asked for, if it is not. */
    val problem: String? = null,
)
