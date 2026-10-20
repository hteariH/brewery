package dev.brewery.daemon.mcp

import com.google.gson.JsonArray
import com.google.gson.JsonObject

/** One MCP tool. [call] gets the `arguments` object (empty when absent) and returns any Gson-serialisable value. */
class McpTool(val name: String, val description: String, val inputSchema: JsonObject, val call: (JsonObject) -> Any?)

/** A failure the agent should read (missing datasource, rejected SQL); reported as a tool result with `isError`. */
class McpToolError(message: String) : RuntimeException(message)

fun objectSchema(vararg props: Pair<String, JsonObject>, required: List<String> = emptyList()): JsonObject = JsonObject().apply {
    addProperty("type", "object")
    add("properties", JsonObject().apply { props.forEach { (name, schema) -> add(name, schema) } })
    if (required.isNotEmpty()) add("required", JsonArray().apply { required.forEach(::add) })
}

fun stringProp(description: String): JsonObject = prop("string", description)

fun boolProp(description: String): JsonObject = prop("boolean", description)

private fun prop(type: String, description: String) = JsonObject().apply {
    addProperty("type", type)
    addProperty("description", description)
}
