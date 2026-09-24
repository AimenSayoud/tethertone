package dev.tethertone.shared

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.long
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals

/**
 * Runs docs/test-vectors.json — the same file the Swift self-test and
 * tools/probe.py check — so the implementations cannot drift apart silently.
 * JVM-only because it reads a file; the logic under test is common code.
 */
class SharedVectorsTest {

    private val vectors: JsonObject = run {
        // Gradle runs tests from the module directory (android/shared).
        val file = listOf("../../docs/test-vectors.json", "../docs/test-vectors.json", "docs/test-vectors.json")
            .map(::File).first { it.exists() }
        Json.parseToJsonElement(file.readText()).jsonObject
    }

    private val controlJson = Json { ignoreUnknownKeys = true; explicitNulls = false }

    @OptIn(ExperimentalStdlibApi::class)
    @Test
    fun headersMatchTheSharedBytes() {
        for (v in vectors.getValue("headers").jsonArray.map { it.jsonObject }) {
            val name = v.getValue("name").jsonPrimitive.content
            val encoded = encodeHeader(
                v.getValue("type").jsonPrimitive.int,
                v.getValue("flags").jsonPrimitive.int,
                v.getValue("length").jsonPrimitive.int,
                v.getValue("seq").jsonPrimitive.long,
            )
            assertEquals(v.getValue("hex").jsonPrimitive.content, encoded.toHexString(), name)
            val decoded = decodeHeader(encoded)
            assertEquals(v.getValue("seq").jsonPrimitive.long, decoded.seq, name)
        }
    }

    @Test
    fun controlMessagesDecodeAsSpecified() {
        for (v in vectors.getValue("control").jsonArray.map { it.jsonObject }) {
            val name = v.getValue("name").jsonPrimitive.content
            val message = controlJson.decodeFromString<ControlMessage>(v.getValue("json").jsonPrimitive.content)
            val expect = v.getValue("expect").jsonObject
            assertEquals(expect.getValue("t").jsonPrimitive.content, message.t, name)
            expect["down"]?.let { assertEquals(it.jsonPrimitive.boolean, message.mode.down, name) }
            expect["up"]?.let { assertEquals(it.jsonPrimitive.content, message.mode.up.wire, name) }
            expect["upRate"]?.let { assertEquals(it.jsonPrimitive.int, message.upRate, name) }
            expect["upCh"]?.let { assertEquals(it.jsonPrimitive.int, message.upCh, name) }
            expect["reason"]?.let { assertEquals(it.jsonPrimitive.content, message.reason, name) }
        }
    }

    @Test
    fun modeMessageOmitsFormatWhenNotSending() {
        val off = controlJson.encodeToString(ControlMessage.mode(StreamMode(down = true), 48000, 1))
        assertEquals("""{"t":"mode","down":true,"up":"off"}""", off)
        val mic = controlJson.encodeToString(ControlMessage.mode(StreamMode(true, UpSource.MIC), 48000, 1))
        assertEquals("""{"t":"mode","down":true,"up":"mic","upRate":48000,"upCh":1}""", mic)
    }

    @Test
    fun aModeMustCarryAudioSomewhere() {
        assertEquals(false, StreamMode(down = false, up = UpSource.OFF).isValid)
        assertEquals(true, StreamMode(down = false, up = UpSource.MIC).isValid)
    }
}
