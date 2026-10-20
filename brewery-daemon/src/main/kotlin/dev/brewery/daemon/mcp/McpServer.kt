package dev.brewery.daemon.mcp

import com.google.gson.Gson
import com.google.gson.GsonBuilder
import com.google.gson.JsonArray
import com.google.gson.JsonElement
import com.google.gson.JsonNull
import com.google.gson.JsonObject
import com.google.gson.JsonParser
import java.io.InputStream
import java.io.OutputStream

/**
 * A Model Context Protocol server over stdio: one JSON-RPC message per line. Only the tools feature is
 * offered; anything else is "method not found". Tool failures are tool results with `isError`, so the agent
 * reads them, while protocol mistakes (unknown tool, bad JSON) are JSON-RPC errors.
 */
class McpServer(private val tools: List<McpTool>, private val version: String) {
    private val gson: Gson = GsonBuilder().serializeNulls().disableHtmlEscaping().create()
    private val byName = tools.associateBy { it.name }

    fun serve(input: InputStream, output: OutputStream) {
        val writer = output.bufferedWriter(Charsets.UTF_8)
        input.bufferedReader(Charsets.UTF_8).lineSequence().forEach { line ->
            if (line.isBlank()) return@forEach
            val response = handle(line) ?: return@forEach
            writer.write(response)
            writer.write("\n")
            writer.flush()
        }
    }

    /** The response line for [line], or null for a notification. */
    fun handle(line: String): String? {
        val message = try {
            JsonParser.parseString(line).asJsonObject
        } catch (e: Exception) {
            return error(JsonNull.INSTANCE, PARSE_ERROR, "Parse error")
        }
        val id = message.get("id") ?: return null
        val method = message.get("method")?.takeIf { it.isJsonPrimitive }?.asString
            ?: return error(id, INVALID_REQUEST, "Missing method")
        val params = message.get("params")?.takeIf { it.isJsonObject }?.asJsonObject ?: JsonObject()
        return when (method) {
            "initialize" -> result(id, initialize(params))
            "ping" -> result(id, JsonObject())
            "tools/list" -> result(id, JsonObject().apply { add("tools", toolList()) })
            "tools/call" -> {
                val name = params.get("name")?.takeIf { it.isJsonPrimitive }?.asString
                val tool = name?.let(byName::get) ?: return error(id, INVALID_PARAMS, "Unknown tool: $name")
                val args = params.get("arguments")?.takeIf { it.isJsonObject }?.asJsonObject ?: JsonObject()
                result(id, callTool(tool, args))
            }
            else -> error(id, METHOD_NOT_FOUND, "Method not found: $method")
        }
    }

    private fun initialize(params: JsonObject): JsonObject {
        val requested = params.get("protocolVersion")?.takeIf { it.isJsonPrimitive }?.asString
        return JsonObject().apply {
            addProperty("protocolVersion", requested?.takeIf { it in SUPPORTED_VERSIONS } ?: SUPPORTED_VERSIONS.last())
            add("capabilities", JsonObject().apply { add("tools", JsonObject()) })
            add("serverInfo", JsonObject().apply {
                addProperty("name", "brewery")
                addProperty("version", version)
            })
        }
    }

    private fun toolList() = JsonArray().apply {
        tools.forEach { tool ->
            add(JsonObject().apply {
                addProperty("name", tool.name)
                addProperty("description", tool.description)
                add("inputSchema", tool.inputSchema)
            })
        }
    }

    private fun callTool(tool: McpTool, args: JsonObject): JsonObject {
        val (text, isError) = try {
            gson.toJson(tool.call(args)) to false
        } catch (e: McpToolError) {
            (e.message ?: "Failed") to true
        } catch (e: Exception) {
            "${e.javaClass.simpleName}: ${e.message ?: ""}".trim() to true
        }
        return JsonObject().apply {
            add("content", JsonArray().apply {
                add(JsonObject().apply {
                    addProperty("type", "text")
                    addProperty("text", text)
                })
            })
            if (isError) addProperty("isError", true)
        }
    }

    private fun result(id: JsonElement, result: JsonElement): String = gson.toJson(JsonObject().apply {
        addProperty("jsonrpc", "2.0")
        add("id", id)
        add("result", result)
    })

    private fun error(id: JsonElement, code: Int, message: String): String = gson.toJson(JsonObject().apply {
        addProperty("jsonrpc", "2.0")
        add("id", id)
        add("error", JsonObject().apply {
            addProperty("code", code)
            addProperty("message", message)
        })
    })

    companion object {
        /** Oldest first; the last one is offered when the client asks for an unknown version. */
        val SUPPORTED_VERSIONS = listOf("2024-11-05", "2025-03-26", "2025-06-18")
        const val PARSE_ERROR = -32700
        const val INVALID_REQUEST = -32600
        const val METHOD_NOT_FOUND = -32601
        const val INVALID_PARAMS = -32602
    }
}
