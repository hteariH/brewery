package dev.brewery.daemon

import org.eclipse.lsp4j.jsonrpc.json.MessageJsonHandler
import org.junit.jupiter.api.Test
import java.io.File
import kotlin.test.assertEquals

class FixturesTest {
    private val gson = MessageJsonHandler(emptyMap()).gson

    @Test
    fun `initialize fixtures deserialise`() {
        val params = gson.fromJson(File("protocol/fixtures/initialize.params.json").readText(), InitializeParams::class.java)
        assertEquals(InitializeParams(1), params)
        val result = gson.fromJson(File("protocol/fixtures/initialize.result.json").readText(), InitializeResult::class.java)
        assertEquals(InitializeResult(1, "0.1.0"), result)
    }
}
