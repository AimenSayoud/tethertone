package dev.tethertone.shared

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/** Wire constants. See docs/PROTOCOL.md — this file and that document must agree. */
object Proto {
    const val VERSION = 1
    const val HEADER_SIZE = 8
    const val MAX_PAYLOAD = 65535

    const val AUDIO = 0
    const val PING = 1
    const val PONG = 2
    const val STATS = 3
    const val BYE = 4
    const val CONTROL = 5

    /** Capability: audio in both directions plus CONTROL frames. See PROTOCOL.md §6. */
    const val CAP_DUPLEX = "duplex"
}

/**
 * `seq` is a u32 on the wire and is carried as a Long so that comparisons and
 * gap arithmetic do not have to fight Kotlin's signed Int.
 */
data class FrameHeader(val type: Int, val flags: Int, val length: Int, val seq: Long)

/** Header fields are big-endian. AUDIO *payload* is little-endian PCM — see [pcmLe16At]. */
fun encodeHeader(dest: ByteArray, offset: Int, type: Int, flags: Int, length: Int, seq: Long) {
    require(dest.size - offset >= Proto.HEADER_SIZE) { "header does not fit" }
    require(length in 0..Proto.MAX_PAYLOAD) { "payload length $length out of range" }
    dest[offset] = type.toByte()
    dest[offset + 1] = flags.toByte()
    dest[offset + 2] = (length ushr 8).toByte()
    dest[offset + 3] = length.toByte()
    dest[offset + 4] = (seq ushr 24).toByte()
    dest[offset + 5] = (seq ushr 16).toByte()
    dest[offset + 6] = (seq ushr 8).toByte()
    dest[offset + 7] = seq.toByte()
}

fun decodeHeader(src: ByteArray, offset: Int = 0): FrameHeader {
    require(src.size - offset >= Proto.HEADER_SIZE) { "short header" }
    fun b(i: Int) = src[offset + i].toInt() and 0xFF
    return FrameHeader(
        type = b(0),
        flags = b(1),
        length = (b(2) shl 8) or b(3),
        seq = (b(4).toLong() shl 24) or (b(5).toLong() shl 16) or (b(6).toLong() shl 8) or b(7).toLong(),
    )
}

fun encodeHeader(type: Int, flags: Int, length: Int, seq: Long): ByteArray =
    ByteArray(Proto.HEADER_SIZE).also { encodeHeader(it, 0, type, flags, length, seq) }

/** u32 distance from [from] to [to], accounting for wrap at 2^32. */
fun seqGap(from: Long, to: Long): Long = (to - from) and 0xFFFFFFFFL

fun putU64Be(value: Long): ByteArray = ByteArray(8) { i -> (value ushr (56 - 8 * i)).toByte() }

fun getU64Be(src: ByteArray, offset: Int = 0): Long {
    var v = 0L
    for (i in 0 until 8) v = (v shl 8) or (src[offset + i].toLong() and 0xFF)
    return v
}

/** One signed 16-bit little-endian sample at byte [index]. Used by tests and level meters. */
fun pcmLe16At(src: ByteArray, index: Int): Int {
    val lo = src[index].toInt() and 0xFF
    val hi = src[index + 1].toInt()
    return (hi shl 8) or lo
}

/**
 * Peak absolute amplitude of an s16le buffer, normalised to 0.0..1.0.
 *
 * Sampled every [stride] frames rather than exhaustively: this runs on the
 * network thread for every packet, and a meter does not need the true peak,
 * only one close enough to move convincingly.
 */
fun pcmPeakLe16(src: ByteArray, length: Int = src.size, stride: Int = 4): Float {
    var peak = 0
    var i = 0
    val step = stride * 2
    val end = length - 1
    while (i < end) {
        val sample = pcmLe16At(src, i)
        val magnitude = if (sample < 0) -sample else sample
        if (magnitude > peak) peak = magnitude
        i += step
    }
    return (peak / 32768f).coerceIn(0f, 1f)
}

@Serializable
data class Hello(
    val v: Int = Proto.VERSION,
    val role: String = "sink",
    val token: String,
    val name: String,
    val caps: List<String> = listOf("pcm_s16le", Proto.CAP_DUPLEX),
)

@Serializable
data class Welcome(
    val v: Int = Proto.VERSION,
    val ok: Boolean = false,
    val rate: Int = 48000,
    val ch: Int = 2,
    val fmt: String = "pcm_s16le",
    val block: Int = 480,
    val name: String = "",
    val err: String? = null,
    val caps: List<String> = emptyList(),
)

@Serializable
data class ClientStats(
    @SerialName("bufMs") val bufMs: Int,
    @SerialName("under") val underruns: Int,
    @SerialName("lost") val lost: Long,
    @SerialName("speed") val speed: Float,
)

/** What the client sends up to the server, if anything. */
enum class UpSource(val wire: String) {
    OFF("off"),
    MIC("mic"),
    /** Audio other apps on the phone are playing. */
    PLAYBACK("playback");

    companion object {
        /** Anything unrecognised is OFF: a newer peer's source we cannot produce. */
        fun fromWire(value: String?): UpSource = entries.firstOrNull { it.wire == value } ?: OFF
    }
}

/** Which way audio flows on one connection. The client owns it (PROTOCOL.md §6). */
data class StreamMode(val down: Boolean = true, val up: UpSource = UpSource.OFF) {
    val isValid: Boolean get() = down || up != UpSource.OFF
}

/** Payload of a CONTROL frame. Only the fields a given `t` uses are present. */
@Serializable
data class ControlMessage(
    val t: String,
    val down: Boolean? = null,
    val up: String? = null,
    val upRate: Int? = null,
    val upCh: Int? = null,
    val reason: String? = null,
) {
    val mode: StreamMode get() = StreamMode(down = down ?: true, up = UpSource.fromWire(up))

    companion object {
        const val MODE = "mode"
        const val SET_MODE = "setMode"
        const val REFUSED = "refused"
        const val UP_DENIED = "upDenied"

        fun mode(mode: StreamMode, upRate: Int?, upCh: Int?) = ControlMessage(
            t = MODE, down = mode.down, up = mode.up.wire,
            upRate = upRate.takeIf { mode.up != UpSource.OFF },
            upCh = upCh.takeIf { mode.up != UpSource.OFF },
        )
    }
}
