package dev.brewery.daemon.mcp

import com.google.gson.JsonParser
import dev.brewery.daemon.runMcp
import org.junit.jupiter.api.Test
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.nio.file.Files
import kotlin.test.assertEquals

class RunMcpTest {
    @Test
    fun `runMcp serves until EOF and exits 0`() {
        val input = ByteArrayInputStream("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n".toByteArray())
        val out = ByteArrayOutputStream()
        var code = -1
        runMcp(Files.createTempDirectory("mcp"), input, out, emptyList()) { code = it }
        assertEquals(0, code)
        val tools = JsonParser.parseString(out.toString(Charsets.UTF_8).trim()).asJsonObject.getAsJsonObject("result").getAsJsonArray("tools")
        assertEquals(0, tools.size())
    }
}
