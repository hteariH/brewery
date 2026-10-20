package dev.brewery.daemon.mcp

import com.google.gson.JsonObject
import com.google.gson.JsonParser
import org.junit.jupiter.api.Test
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

class McpServerTest {
    private val echo = McpTool("echo", "Echoes text", objectSchema("text" to stringProp("Text"), required = listOf("text"))) { args ->
        mapOf("text" to args.get("text").asString)
    }
    private val failing = McpTool("fail", "Always fails", objectSchema()) { throw McpToolError("no datasource") }
    private val crashing = McpTool("crash", "Throws", objectSchema()) { error("boom") }
    private val server = McpServer(listOf(echo, failing, crashing), "1.2.3")

    private fun call(line: String): JsonObject = JsonParser.parseString(server.handle(line)!!).asJsonObject

    @Test
    fun `initialize echoes a supported protocol version and declares tools`() {
        val r = call("""{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"zed","version":"1"}}}""")
        val result = r.getAsJsonObject("result")
        assertEquals(1, r.get("id").asInt)
        assertEquals("2025-03-26", result.get("protocolVersion").asString)
        assertTrue(result.getAsJsonObject("capabilities").has("tools"))
        assertEquals("brewery", result.getAsJsonObject("serverInfo").get("name").asString)
        assertEquals("1.2.3", result.getAsJsonObject("serverInfo").get("version").asString)
    }

    @Test
    fun `initialize answers an unknown protocol version with the newest one`() {
        val r = call("""{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}""")
        assertEquals("2025-06-18", r.getAsJsonObject("result").get("protocolVersion").asString)
    }

    @Test
    fun `notifications get no response`() {
        assertNull(server.handle("""{"jsonrpc":"2.0","method":"notifications/initialized"}"""))
    }

    @Test
    fun `ping returns an empty result`() {
        assertEquals(JsonObject(), call("""{"jsonrpc":"2.0","id":"a","method":"ping"}""").getAsJsonObject("result"))
    }

    @Test
    fun `tools list carries name description and schema`() {
        val tools = call("""{"jsonrpc":"2.0","id":2,"method":"tools/list"}""").getAsJsonObject("result").getAsJsonArray("tools")
        assertEquals(listOf("echo", "fail", "crash"), tools.map { it.asJsonObject.get("name").asString })
        val schema = tools[0].asJsonObject.getAsJsonObject("inputSchema")
        assertEquals("object", schema.get("type").asString)
        assertEquals("text", schema.getAsJsonArray("required")[0].asString)
    }

    @Test
    fun `tools call returns json text content`() {
        val r = call("""{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"echo","arguments":{"text":"hi"}}}""")
        val content = r.getAsJsonObject("result").getAsJsonArray("content")[0].asJsonObject
        assertEquals("text", content.get("type").asString)
        assertEquals("hi", JsonParser.parseString(content.get("text").asString).asJsonObject.get("text").asString)
        assertTrue(!r.getAsJsonObject("result").has("isError") || !r.getAsJsonObject("result").get("isError").asBoolean)
    }

    @Test
    fun `tool errors are results with isError`() {
        for ((name, text) in listOf("fail" to "no datasource", "crash" to "boom")) {
            val result = call("""{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"$name"}}""").getAsJsonObject("result")
            assertTrue(result.get("isError").asBoolean)
            assertTrue(result.getAsJsonArray("content")[0].asJsonObject.get("text").asString.contains(text))
        }
    }

    @Test
    fun `unknown tool and unknown method are json-rpc errors`() {
        assertEquals(-32602, call("""{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"nope"}}""").getAsJsonObject("error").get("code").asInt)
        assertEquals(-32601, call("""{"jsonrpc":"2.0","id":6,"method":"resources/list"}""").getAsJsonObject("error").get("code").asInt)
    }

    @Test
    fun `malformed line is a parse error with null id`() {
        val r = call("{not json")
        assertEquals(-32700, r.getAsJsonObject("error").get("code").asInt)
        assertTrue(r.get("id").isJsonNull)
    }

    @Test
    fun `serve answers line by line and skips blank lines`() {
        val input = ByteArrayInputStream(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n".toByteArray(),
        )
        val out = ByteArrayOutputStream()
        server.serve(input, out)
        val lines = out.toString(Charsets.UTF_8).split("\n").filter { it.isNotBlank() }
        assertEquals(2, lines.size)
        assertEquals(listOf(1, 2), lines.map { JsonParser.parseString(it).asJsonObject.get("id").asInt })
    }
}
