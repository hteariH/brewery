# Brewery Lite Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship Brewery's non-UI features (Spring config LSP, an MCP server for Zed's agent, theme + icons) as the Zed extension "Brewery Lite".

**Architecture:** The Kotlin daemon gains a `--mcp` mode (newline-delimited JSON-RPC over stdio, hand-written on Gson) exposing project, Spring and read-only database tools. A Rust/WASM extension in `extensions/brewery-lite` downloads the version-matched daemon jar from the GitHub release, finds or downloads Java 21, and starts the daemon as a language server (`--spring-config-lsp`) and as a context server (`--mcp`). The release workflow attaches the daemon jar.

**Tech Stack:** Kotlin 2.4 / JVM 21, Gson (via lsp4j), JUnit 5, Testcontainers; Rust `zed_extension_api` 0.7.0 → `wasm32-wasip2`, `sha2`; Python release scripts; GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-10-20-brewery-lite-design.md`

## Global Constraints

- Java 21+ for the daemon (`jvmToolchain(21)`).
- Extension id `brewery-lite`, name "Brewery Lite", version == `crates/zed/BREWERY_VERSION`, licence Apache-2.0.
- `zed_extension_api = "0.7.0"` (newest on crates.io). The extension crate is NOT a member of the root workspace.
- Daemon jar asset name: `brewery-daemon-<version>.jar`, downloaded from tag `v<version>` of `hteariH/brewery`, verified against the release's `SHA256SUMS`.
- Only `process:exec` capability: command `java`, args `["-version"]`.
- `db_query`: only `SqlText.classify(sql) == READ`, read-only connection, transaction always rolled back, max 200 rows, 30 s timeout.
- MCP stdio framing: one JSON object per line (UTF-8, `\n`). Logs only to stderr.
- All user-facing text in English. LF line endings. No attribution lines in commits.
- Commit after every task. Every edit to a Zed-owned file is marked `// brewery:` (none expected in this plan).

## File Structure

Daemon (`brewery-daemon/src/main/kotlin/dev/brewery/daemon/`):
- `mcp/McpServer.kt` — JSON-RPC/MCP framing and dispatch; knows nothing about tools' internals.
- `mcp/McpTool.kt` — `McpTool` + `McpToolError` + schema helper.
- `mcp/ProjectTools.kt` — `project_model`, `spring_beans`, `spring_endpoints`, `run_configurations`.
- `mcp/SpringDatasource.kt` — find `spring.datasource.*` in the project.
- `mcp/DbTools.kt` — `db_schema`, `db_query`.
- `project/ProjectModelReader.kt` — gains `readProjectModel(dir)` (moved out of `BreweryDaemon.readModel`).
- `Main.kt` — `--mcp` switch + `runMcp`.

Tests (`brewery-daemon/src/test/kotlin/dev/brewery/daemon/mcp/`): `McpServerTest.kt`, `ProjectToolsTest.kt`, `SpringDatasourceTest.kt`, `DbToolsTest.kt`, `ContainerDbToolsTest.kt`, `RunMcpTest.kt`.

Release: `script/brewery/release_assets.py`, `script/brewery/test_release_assets.py`, `.github/workflows/brewery_ci.yml` (daemon job uploads `package-daemon`).

Extension (`extensions/brewery-lite/`): `extension.toml`, `Cargo.toml`, `src/lib.rs` (Zed glue), `src/java.rs` (pure Java discovery helpers), `src/daemon_jar.rs` (pure jar/asset helpers), `themes/brewery.json`, `icon_themes/brewery-icons.json`, `icons/**`, `LICENSE-APACHE`, `README.md`.
Sync check: `script/brewery/sync_brewery_lite.py` + `script/brewery/test_sync_brewery_lite.py`. CI job `brewery-lite` in `brewery_ci.yml`.

Docs: `docs/brewery/FORK.md` (Brewery Lite section), `CHANGELOG.md` (Unreleased).

---

### Task 1: MCP server core and `--mcp` mode

**Files:**
- Create: `brewery-daemon/src/main/kotlin/dev/brewery/daemon/mcp/McpTool.kt`
- Create: `brewery-daemon/src/main/kotlin/dev/brewery/daemon/mcp/McpServer.kt`
- Modify: `brewery-daemon/src/main/kotlin/dev/brewery/daemon/Main.kt`
- Test: `brewery-daemon/src/test/kotlin/dev/brewery/daemon/mcp/McpServerTest.kt`

**Interfaces:**
- Produces:
  - `class McpTool(val name: String, val description: String, val inputSchema: JsonObject, val call: (JsonObject) -> Any?)` — `call` returns any Gson-serialisable value; throws `McpToolError` for user-facing failures.
  - `class McpToolError(message: String) : RuntimeException(message)`
  - `fun objectSchema(vararg props: Pair<String, JsonObject>, required: List<String> = emptyList()): JsonObject`, `fun stringProp(description: String): JsonObject`, `fun boolProp(description: String): JsonObject`
  - `class McpServer(tools: List<McpTool>, version: String)` with `fun handle(line: String): String?` and `fun serve(input: InputStream, output: OutputStream)`
  - `fun runMcp(root: Path, input: InputStream, rpcOut: OutputStream, tools: List<McpTool>, exit: (Int) -> Unit)` in `Main.kt`

- [ ] **Step 1: Write the failing tests**

```kotlin
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
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd brewery-daemon && ./gradlew test --tests 'dev.brewery.daemon.mcp.McpServerTest' --no-daemon`
Expected: compilation FAIL (`McpTool`, `McpServer` unresolved).

- [ ] **Step 3: Implement `McpTool.kt`**

```kotlin
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
```

- [ ] **Step 4: Implement `McpServer.kt`**

```kotlin
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
```

- [ ] **Step 5: Run the tests**

Run: `cd brewery-daemon && ./gradlew test --tests 'dev.brewery.daemon.mcp.McpServerTest' --no-daemon`
Expected: PASS (10 tests).

- [ ] **Step 6: Add `--mcp` to `Main.kt`**

In `main`, before the `--spring-config-lsp` branch:

```kotlin
    if ("--mcp" in args) {
        val root = java.nio.file.Paths.get("").toAbsolutePath()
        runMcp(root, System.`in`, rpcOut, mcpTools(root)) { exitProcess(it) }
    } else if ("--spring-config-lsp" in args) {
```

and below `runSpringConfigLsp`:

```kotlin
/**
 * Serves MCP over [input]/[rpcOut] for the project in [root] until EOF. Zed starts context servers in the
 * project's first worktree and sends no MCP roots, so the working directory is the project.
 */
fun runMcp(root: Path, input: InputStream, rpcOut: OutputStream, tools: List<McpTool>, exit: (Int) -> Unit) {
    System.err.println("brewery mcp: serving $root")
    McpServer(tools, daemonVersion()).serve(input, rpcOut)
    exit(0)
}

/** Every MCP tool for [root]; the tool sets are added by later tasks. */
fun mcpTools(root: Path): List<McpTool> = emptyList()

private fun daemonVersion(): String = McpServer::class.java.`package`?.implementationVersion ?: "dev"
```

(imports: `dev.brewery.daemon.mcp.McpServer`, `dev.brewery.daemon.mcp.McpTool`, `java.nio.file.Path`.)

- [ ] **Step 7: Test `runMcp` end to end over pipes**

Create `brewery-daemon/src/test/kotlin/dev/brewery/daemon/mcp/RunMcpTest.kt`:

```kotlin
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
```

Run: `cd brewery-daemon && ./gradlew test --tests 'dev.brewery.daemon.mcp.*' --no-daemon` → PASS.

- [ ] **Step 8: Commit**

```bash
git add brewery-daemon/src/main/kotlin/dev/brewery/daemon/mcp brewery-daemon/src/main/kotlin/dev/brewery/daemon/Main.kt brewery-daemon/src/test/kotlin/dev/brewery/daemon/mcp
git commit -m "daemon: MCP server over stdio (--mcp)"
```

---

### Task 2: Project and Spring tools

**Files:**
- Modify: `brewery-daemon/src/main/kotlin/dev/brewery/daemon/project/ProjectModelReader.kt` (add `readProjectModel`)
- Modify: `brewery-daemon/src/main/kotlin/dev/brewery/daemon/BreweryDaemon.kt:317-330` (use `readProjectModel`)
- Create: `brewery-daemon/src/main/kotlin/dev/brewery/daemon/mcp/ProjectTools.kt`
- Modify: `brewery-daemon/src/main/kotlin/dev/brewery/daemon/Main.kt` (`mcpTools`)
- Test: `brewery-daemon/src/test/kotlin/dev/brewery/daemon/mcp/ProjectToolsTest.kt`

**Interfaces:**
- Consumes: `McpTool`, `McpToolError`, `objectSchema`, `boolProp` (Task 1); `SpringScanner().scan(model): SpringModel`; `RunConfigurations.build(project, spring, windows): List<RunConfiguration>`; `ProjectModel(buildTool, rootDir, modules)`.
- Produces: `fun readProjectModel(dir: Path): ProjectModel` (top level, `project` package); `class ProjectTools(root: Path, readModel: (Path) -> ProjectModel = ::readProjectModel, windows: Boolean = isWindows())` with `fun tools(): List<McpTool>` naming `project_model`, `spring_beans`, `spring_endpoints`, `run_configurations`.

- [ ] **Step 1: Write the failing tests**

```kotlin
package dev.brewery.daemon.mcp

import com.google.gson.JsonObject
import dev.brewery.daemon.BuildModule
import dev.brewery.daemon.BuildTask
import dev.brewery.daemon.BuildTool
import dev.brewery.daemon.ProjectModel
import dev.brewery.daemon.spring.SpringModel
import dev.brewery.daemon.run.RunConfiguration
import org.junit.jupiter.api.Test
import java.io.File
import java.nio.file.Path
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class ProjectToolsTest {
    private val petshop: Path = File("src/test/resources/projects/spring-petshop").absoluteFile.toPath().normalize()
    private var reads = 0
    private val model = ProjectModel(BuildTool.gradle, petshop.toString(), listOf(BuildModule("spring-petshop", ":", listOf(BuildTask("bootRun", "application", null)))))
    private val tools = ProjectTools(petshop, readModel = { reads++; model }, windows = false).tools().associateBy { it.name }

    private fun args(vararg pairs: Pair<String, Boolean>) = JsonObject().apply { pairs.forEach { (k, v) -> addProperty(k, v) } }

    @Test
    fun `offers the four project tools`() {
        assertEquals(setOf("project_model", "spring_beans", "spring_endpoints", "run_configurations"), tools.keys)
    }

    @Test
    fun `project model is cached until refresh`() {
        assertEquals(model, tools.getValue("project_model").call(args()))
        tools.getValue("project_model").call(args())
        assertEquals(1, reads)
        tools.getValue("project_model").call(args("refresh" to true))
        assertEquals(2, reads)
    }

    @Test
    fun `spring tools scan the project sources`() {
        @Suppress("UNCHECKED_CAST")
        val beans = tools.getValue("spring_beans").call(args()) as List<Any>
        assertTrue(beans.isNotEmpty())
        @Suppress("UNCHECKED_CAST")
        val endpoints = tools.getValue("spring_endpoints").call(args()) as List<Any>
        assertTrue(endpoints.isNotEmpty())
    }

    @Test
    fun `run configurations use the bootRun task`() {
        @Suppress("UNCHECKED_CAST")
        val configs = tools.getValue("run_configurations").call(args()) as List<RunConfiguration>
        assertTrue(configs.isNotEmpty())
        assertTrue(configs.all { "bootRun" in it.args.joinToString(" ") })
    }
}
```

(Check the fixture: `src/test/resources/projects/spring-petshop` must contain at least one bean and one endpoint; `SpringScannerTest` already asserts both, so reuse its expectations if you want exact counts. If the fixture has no Gradle wrapper, `RunConfigurations` falls back to `gradle` — the assertion only checks the task.)

- [ ] **Step 2: Run to verify it fails**

Run: `cd brewery-daemon && ./gradlew test --tests 'dev.brewery.daemon.mcp.ProjectToolsTest' --no-daemon`
Expected: compilation FAIL (`ProjectTools` unresolved).

- [ ] **Step 3: Move the reader choice to `readProjectModel`**

Append to `project/ProjectModelReader.kt`:

```kotlin
/** The model of the project in [dir], read by the reader its build files call for. Throws whatever the reader throws. */
fun readProjectModel(dir: Path): ProjectModel {
    val reader = when (detectBuildTool(dir)) {
        BuildTool.gradle -> GradleModelReader()
        BuildTool.maven -> MavenModelReader()
        BuildTool.none -> return ProjectModel(BuildTool.none, dir.toString(), emptyList())
    }
    return reader.read(dir)
}
```

In `BreweryDaemon.readModel`, replace the `val reader = when ... return reader.read(dir)` body inside `try` with `return readProjectModel(dir)` (keep the catch blocks unchanged). Run `./gradlew test --tests 'dev.brewery.daemon.ProjectModelTest' --no-daemon` → PASS.

- [ ] **Step 4: Implement `ProjectTools.kt`**

```kotlin
package dev.brewery.daemon.mcp

import com.google.gson.JsonObject
import dev.brewery.daemon.ProjectModel
import dev.brewery.daemon.project.readProjectModel
import dev.brewery.daemon.run.RunConfigurations
import dev.brewery.daemon.spring.SpringModel
import dev.brewery.daemon.spring.SpringScanner
import java.nio.file.Path

/**
 * Project tools for the agent. The build model is imported once (a Gradle import takes seconds) and kept
 * until the agent asks for `refresh`; the Spring scan is cheap and incremental, so it runs on every call.
 */
class ProjectTools(
    private val root: Path,
    private val readModel: (Path) -> ProjectModel = ::readProjectModel,
    private val windows: Boolean = System.getProperty("os.name").startsWith("Windows"),
) {
    private val scanner = SpringScanner()
    private var model: ProjectModel? = null

    @Synchronized
    private fun model(refresh: Boolean): ProjectModel {
        if (refresh || model == null) {
            model = try {
                readModel(root)
            } catch (e: Exception) {
                throw McpToolError("Could not import the ${root.fileName} build: ${e.message ?: e.javaClass.simpleName}")
            }
        }
        return model!!
    }

    private fun spring(): SpringModel = scanner.scan(model(false))

    private fun JsonObject.flag(name: String) = get(name)?.takeIf { it.isJsonPrimitive }?.asBoolean ?: false

    fun tools(): List<McpTool> = listOf(
        McpTool(
            "project_model",
            "Build tool (Gradle or Maven), modules and their tasks of the open project.",
            objectSchema("refresh" to boolProp("Re-import the build instead of using the cached model.")),
        ) { args -> model(args.flag("refresh")) },
        McpTool(
            "spring_beans",
            "Spring beans declared in the project's sources: name, type, stereotype, file and line.",
            objectSchema(),
        ) { spring().beans },
        McpTool(
            "spring_endpoints",
            "HTTP endpoints of the project's Spring controllers: method, path, handler, file and line.",
            objectSchema(),
        ) { spring().endpoints },
        McpTool(
            "run_configurations",
            "Commands that start each Spring Boot application of the project, with working directory and main class.",
            objectSchema(),
        ) { RunConfigurations.build(model(false), spring(), windows) },
    )
}
```

- [ ] **Step 5: Wire into `mcpTools`**

```kotlin
fun mcpTools(root: Path): List<McpTool> = ProjectTools(root).tools()
```

- [ ] **Step 6: Run the tests**

Run: `cd brewery-daemon && ./gradlew test --tests 'dev.brewery.daemon.mcp.*' --tests 'dev.brewery.daemon.ProjectModelTest' --no-daemon`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add brewery-daemon/src
git commit -m "daemon: MCP project, Spring and run configuration tools"
```

---

### Task 3: Spring datasource discovery

**Files:**
- Create: `brewery-daemon/src/main/kotlin/dev/brewery/daemon/mcp/SpringDatasource.kt`
- Test: `brewery-daemon/src/test/kotlin/dev/brewery/daemon/mcp/SpringDatasourceTest.kt`

**Interfaces:**
- Consumes: `YamlDocument(text).entries` (`YamlEntry.key`, `.value`, `.document`, `.hasChildren`), `PropertiesDocument(text).entries` (`PropertyEntry.key`, `.value`).
- Produces: `data class Datasource(val url: String, val username: String?, val password: String?, val source: Path)`; `object SpringDatasource { fun find(root: Path, env: (String) -> String? = System::getenv): Datasource? }` — throws `McpToolError` when a placeholder can't be resolved.

- [ ] **Step 1: Write the failing tests**

```kotlin
package dev.brewery.daemon.mcp

import org.junit.jupiter.api.Test
import org.junit.jupiter.api.assertThrows
import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.assertEquals
import kotlin.test.assertNull

class SpringDatasourceTest {
    private fun project(vararg files: Pair<String, String>): Path {
        val root = Files.createTempDirectory("ds")
        for ((path, text) in files) {
            val file = root.resolve(path)
            Files.createDirectories(file.parent)
            Files.writeString(file, text)
        }
        return root
    }

    private val res = "src/main/resources"

    @Test
    fun `reads application yml of the default profile`() {
        val root = project("$res/application.yml" to """
            spring:
              datasource:
                url: "jdbc:postgresql://localhost/shop"   # local db
                username: shop
                password: 'secret'
            ---
            spring:
              config.activate.on-profile: prod
              datasource:
                url: jdbc:postgresql://prod/shop
        """.trimIndent())
        val ds = SpringDatasource.find(root) { null }!!
        assertEquals("jdbc:postgresql://localhost/shop", ds.url)
        assertEquals("shop", ds.username)
        assertEquals("secret", ds.password)
    }

    @Test
    fun `reads application properties and resolves placeholders`() {
        val root = project("$res/application.properties" to """
            spring.datasource.url=jdbc:postgresql://${'$'}{DB_HOST:localhost}:5432/shop
            spring.datasource.username=${'$'}{DB_USER}
        """.trimIndent())
        val ds = SpringDatasource.find(root) { if (it == "DB_USER") "alice" else null }!!
        assertEquals("jdbc:postgresql://localhost:5432/shop", ds.url)
        assertEquals("alice", ds.username)
        assertNull(ds.password)
    }

    @Test
    fun `unresolvable placeholder is a tool error`() {
        val root = project("$res/application.properties" to "spring.datasource.url=jdbc:postgresql://\${DB_HOST}/shop")
        val e = assertThrows<McpToolError> { SpringDatasource.find(root) { null } }
        assertEquals(true, e.message!!.contains("DB_HOST"))
    }

    @Test
    fun `root module wins over submodules and build dirs are skipped`() {
        val root = project(
            "api/$res/application.properties" to "spring.datasource.url=jdbc:sqlite:api.db",
            "$res/application.yml" to "spring.datasource.url: jdbc:sqlite:root.db",
            "build/resources/main/application.properties" to "spring.datasource.url=jdbc:sqlite:stale.db",
        )
        assertEquals("jdbc:sqlite:root.db", SpringDatasource.find(root) { null }!!.url)
        Files.delete(root.resolve("$res/application.yml"))
        assertEquals("jdbc:sqlite:api.db", SpringDatasource.find(root) { null }!!.url)
    }

    @Test
    fun `no datasource gives null`() {
        assertNull(SpringDatasource.find(project("$res/application.yml" to "server.port: 8080")) { null })
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd brewery-daemon && ./gradlew test --tests 'dev.brewery.daemon.mcp.SpringDatasourceTest' --no-daemon`
Expected: compilation FAIL.

- [ ] **Step 3: Implement `SpringDatasource.kt`**

Before writing, open `springconfig/YamlDocument.kt` and check what `YamlEntry.value` holds for `url: "x"   # c` (raw text after the colon, or trimmed). `cleanYaml` below strips quotes and a trailing ` #` comment either way.

```kotlin
package dev.brewery.daemon.mcp

import dev.brewery.daemon.springconfig.PropertiesDocument
import dev.brewery.daemon.springconfig.YamlDocument
import java.nio.file.FileVisitResult
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.SimpleFileVisitor
import java.nio.file.attribute.BasicFileAttributes

/** [source] is the config file the values came from, so errors can point at it. */
data class Datasource(val url: String, val username: String?, val password: String?, val source: Path)

/**
 * Finds the project's `spring.datasource.*` in `src/main/resources/application.{yml,yaml,properties}`.
 * Only the default profile counts: the first YAML document and the plain properties file. The root
 * module's file wins, then modules in path order.
 */
object SpringDatasource {
    private val SKIP = setOf("build", "target", "out", "node_modules", ".gradle", ".git", ".idea")
    private val NAMES = listOf("application.yml", "application.yaml", "application.properties")
    private val PLACEHOLDER = Regex("""\$\{([^}:]+)(?::([^}]*))?}""")
    private const val MAX_DEPTH = 6

    fun find(root: Path, env: (String) -> String? = System::getenv): Datasource? {
        for (file in configFiles(root)) {
            val values = read(file)
            val url = values["spring.datasource.url"] ?: continue
            return Datasource(
                resolve(url, env, file),
                values["spring.datasource.username"]?.let { resolve(it, env, file) },
                values["spring.datasource.password"]?.let { resolve(it, env, file) },
                file,
            )
        }
        return null
    }

    private fun configFiles(root: Path): List<Path> {
        val found = ArrayList<Path>()
        Files.walkFileTree(root, emptySet(), MAX_DEPTH + 4, object : SimpleFileVisitor<Path>() {
            override fun preVisitDirectory(dir: Path, attrs: BasicFileAttributes): FileVisitResult =
                if (dir != root && dir.fileName.toString() in SKIP) FileVisitResult.SKIP_SUBTREE else FileVisitResult.CONTINUE

            override fun visitFile(file: Path, attrs: BasicFileAttributes): FileVisitResult {
                val parent = file.parent
                if (file.fileName.toString() in NAMES && parent.endsWith(Path.of("src", "main", "resources"))) found += file
                return FileVisitResult.CONTINUE
            }
        })
        // Root module first (fewest path segments), then path order; yml before properties within a module.
        return found.sortedWith(compareBy<Path>({ root.relativize(it).nameCount }, { it.parent.toString() }, { NAMES.indexOf(it.fileName.toString()) }))
    }

    private fun read(file: Path): Map<String, String> {
        val text = Files.readString(file)
        return if (file.fileName.toString().endsWith(".properties")) {
            PropertiesDocument(text).entries.associate { it.key to it.value.trim() }
        } else {
            YamlDocument(text).entries.filter { it.document == 0 && !it.hasChildren }.associate { it.key to cleanYaml(it.value) }
        }
    }

    private fun cleanYaml(raw: String): String {
        var value = raw.trim()
        if (value.startsWith("\"") || value.startsWith("'")) {
            val end = value.indexOf(value[0], 1)
            if (end > 0) return value.substring(1, end)
        }
        val comment = value.indexOf(" #")
        if (comment >= 0) value = value.substring(0, comment).trimEnd()
        return value
    }

    private fun resolve(value: String, env: (String) -> String?, file: Path): String = PLACEHOLDER.replace(value) { m ->
        val name = m.groupValues[1]
        env(name) ?: m.groups[2]?.value
            ?: throw McpToolError("$file: \${$name} is not set in the environment and has no default")
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cd brewery-daemon && ./gradlew test --tests 'dev.brewery.daemon.mcp.SpringDatasourceTest' --no-daemon`
Expected: PASS (5 tests). If the dotted YAML key `config.activate.on-profile` or `spring.datasource.url:` (dotted key at top level) is reported with a different `key` by `YamlDocument`, adjust only `read` (e.g. join path segments) — not the parser.

- [ ] **Step 5: Commit**

```bash
git add brewery-daemon/src
git commit -m "daemon: find the Spring datasource for MCP database tools"
```

---

### Task 4: Database tools (`db_schema`, `db_query`)

**Files:**
- Create: `brewery-daemon/src/main/kotlin/dev/brewery/daemon/mcp/DbTools.kt`
- Modify: `brewery-daemon/src/main/kotlin/dev/brewery/daemon/Main.kt` (`mcpTools`)
- Test: `brewery-daemon/src/test/kotlin/dev/brewery/daemon/mcp/DbToolsTest.kt`, `brewery-daemon/src/test/kotlin/dev/brewery/daemon/mcp/ContainerDbToolsTest.kt`

**Interfaces:**
- Consumes: `SpringDatasource.find(root, env)`, `Datasource` (Task 3); `Introspector(connection).snapshot(maxTables): DbSchema` (`DbSchema.schemas: List<SchemaInfo(name, tables: List<TableInfo(name, kind, columns: List<ColumnInfo(name, type)>)>)>`, `truncated`); `SqlText.classify(sql): SqlText.Kind`; `ValueRenderer.kinds(meta)`, `ValueRenderer.read(rs, index, kind): String?`.
- Produces: `class DbTools(root: Path, env: (String) -> String? = System::getenv, connect: (Datasource) -> Connection = ::openJdbc)` with `fun tools(): List<McpTool>` naming `db_schema`, `db_query`; `data class QueryResult(val columns: List<String>, val rows: List<List<String?>>, val truncated: Boolean)`; `const val MAX_ROWS = 200`.

- [ ] **Step 1: Write the failing tests (SQLite, no Docker)**

```kotlin
package dev.brewery.daemon.mcp

import com.google.gson.JsonObject
import org.junit.jupiter.api.Test
import java.nio.file.Files
import java.nio.file.Path
import java.sql.DriverManager
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

class DbToolsTest {
    private fun project(): Path {
        val root = Files.createTempDirectory("dbtools")
        val db = root.resolve("shop.db")
        DriverManager.getConnection("jdbc:sqlite:$db").use { c ->
            c.createStatement().use { s ->
                s.execute("create table pet(id integer primary key, name text not null)")
                for (i in 1..250) s.execute("insert into pet(name) values ('pet$i')")
            }
        }
        val res = root.resolve("src/main/resources")
        Files.createDirectories(res)
        Files.writeString(res.resolve("application.properties"), "spring.datasource.url=jdbc:sqlite:${db.toString().replace('\\', '/')}\n")
        return root
    }

    private val root = project()
    private val tools = DbTools(root, env = { null }).tools().associateBy { it.name }
    private fun args(vararg pairs: Pair<String, String>) = JsonObject().apply { pairs.forEach { (k, v) -> addProperty(k, v) } }

    @Test
    fun `schema lists tables and one table's columns`() {
        val all = tools.getValue("db_schema").call(args()).toString()
        assertTrue(all.contains("pet"))
        val pet = tools.getValue("db_schema").call(args("table" to "PET")).toString()
        assertTrue(pet.contains("name"), pet)
    }

    @Test
    fun `query returns at most 200 rows and says it truncated`() {
        val r = tools.getValue("db_query").call(args("sql" to "select id, name from pet order by id")) as QueryResult
        assertEquals(listOf("id", "name"), r.columns)
        assertEquals(MAX_ROWS, r.rows.size)
        assertEquals(listOf("1", "pet1"), r.rows[0])
        assertTrue(r.truncated)
    }

    @Test
    fun `writes and multi statements are rejected`() {
        for (sql in listOf("delete from pet", "select 1; delete from pet", "insert into pet(name) values ('x')", "select * into x from pet")) {
            assertFailsWith<McpToolError>(sql) { tools.getValue("db_query").call(args("sql" to sql)) }
        }
        val count = tools.getValue("db_query").call(args("sql" to "select count(*) from pet")) as QueryResult
        assertEquals("250", count.rows[0][0])
    }

    @Test
    fun `missing datasource is a tool error`() {
        val empty = DbTools(Files.createTempDirectory("nods"), env = { null }).tools().associateBy { it.name }
        val e = assertFailsWith<McpToolError> { empty.getValue("db_schema").call(args()) }
        assertTrue(e.message!!.contains("spring.datasource.url"))
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd brewery-daemon && ./gradlew test --tests 'dev.brewery.daemon.mcp.DbToolsTest' --no-daemon`
Expected: compilation FAIL.

- [ ] **Step 3: Implement `DbTools.kt`**

```kotlin
package dev.brewery.daemon.mcp

import dev.brewery.daemon.db.Introspector
import dev.brewery.daemon.db.SchemaSnapshot
import dev.brewery.daemon.db.SqlText
import dev.brewery.daemon.db.ValueRenderer
import java.nio.file.Path
import java.sql.Connection
import java.sql.DriverManager
import java.sql.SQLException

const val MAX_ROWS = 200
private const val QUERY_TIMEOUT_SECONDS = 30

data class QueryResult(val columns: List<String>, val rows: List<List<String?>>, val truncated: Boolean)

private fun openJdbc(ds: Datasource): Connection = DriverManager.getConnection(ds.url, ds.username, ds.password)

/**
 * Read-only database tools on the project's Spring datasource. Read-only is enforced twice: only SQL that
 * [SqlText.classify] calls a plain read is run, on a read-only connection whose transaction is rolled back.
 */
class DbTools(
    private val root: Path,
    private val env: (String) -> String? = System::getenv,
    private val connect: (Datasource) -> Connection = ::openJdbc,
) {
    private fun <T> withConnection(block: (Connection) -> T): T {
        val ds = SpringDatasource.find(root, env)
            ?: throw McpToolError("No spring.datasource.url in src/main/resources/application.{yml,properties} of $root")
        val connection = try {
            connect(ds)
        } catch (e: SQLException) {
            throw McpToolError("Could not connect to ${ds.url} (from ${ds.source}): ${e.message}")
        }
        return connection.use {
            it.isReadOnly = true
            it.autoCommit = false
            try {
                block(it)
            } finally {
                runCatching { it.rollback() }
            }
        }
    }

    private fun schema(table: String?): Any = withConnection { c ->
        val snapshot = Introspector(c).snapshot(SchemaSnapshot.MAX_TABLES)
        if (table == null) {
            mapOf(
                "schemas" to snapshot.schemas.map { s ->
                    mapOf("schema" to s.name, "tables" to s.tables.map { mapOf("name" to it.name, "kind" to it.kind, "columns" to it.columns.size) })
                },
                "truncated" to snapshot.truncated,
            )
        } else {
            val matches = snapshot.schemas.flatMap { s -> s.tables.filter { it.name.equals(table, ignoreCase = true) }.map { s.name to it } }
            if (matches.isEmpty()) throw McpToolError("No table or view named $table")
            matches.map { (schema, t) -> mapOf("schema" to schema, "name" to t.name, "kind" to t.kind, "columns" to t.columns) }
        }
    }

    private fun query(sql: String): QueryResult {
        if (SqlText.classify(sql) != SqlText.Kind.READ) {
            throw McpToolError("Only a single read-only statement (SELECT, WITH, VALUES, TABLE, SHOW, EXPLAIN) is allowed")
        }
        return withConnection { c ->
            c.createStatement().use { st ->
                st.maxRows = MAX_ROWS + 1
                st.queryTimeout = QUERY_TIMEOUT_SECONDS
                st.executeQuery(sql).use { rs ->
                    val meta = rs.metaData
                    val kinds = ValueRenderer.kinds(meta)
                    val columns = (1..meta.columnCount).map { meta.getColumnLabel(it) }
                    val rows = ArrayList<List<String?>>()
                    var truncated = false
                    while (rs.next()) {
                        if (rows.size == MAX_ROWS) { truncated = true; break }
                        rows += (1..meta.columnCount).map { ValueRenderer.read(rs, it, kinds[it - 1]) }
                    }
                    QueryResult(columns, rows, truncated)
                }
            }
        }
    }

    fun tools(): List<McpTool> = listOf(
        McpTool(
            "db_schema",
            "Tables and views of the project's Spring datasource (spring.datasource.url), or the columns of one table.",
            objectSchema("table" to stringProp("Table or view name (case-insensitive). Omit to list all tables.")),
        ) { args -> schema(args.get("table")?.takeIf { it.isJsonPrimitive }?.asString) },
        McpTool(
            "db_query",
            "Runs one read-only SQL statement on the project's Spring datasource and returns up to $MAX_ROWS rows.",
            objectSchema("sql" to stringProp("A single SELECT, WITH, VALUES, TABLE, SHOW or EXPLAIN statement."), required = listOf("sql")),
        ) { args ->
            val sql = args.get("sql")?.takeIf { it.isJsonPrimitive }?.asString ?: throw McpToolError("sql is required")
            query(sql)
        },
    )
}
```

Note: SQLite's JDBC driver rejects `setReadOnly(true)` after the connection is open on some versions ("Cannot change read-only flag after establishing a connection"). If the test hits that, wrap `it.isReadOnly = true` in `runCatching` with a comment explaining that the `SqlText` guard and the rollback still hold for SQLite.

- [ ] **Step 4: Wire into `mcpTools`**

```kotlin
fun mcpTools(root: Path): List<McpTool> = ProjectTools(root).tools() + DbTools(root).tools()
```

- [ ] **Step 5: Run the tests**

Run: `cd brewery-daemon && ./gradlew test --tests 'dev.brewery.daemon.mcp.*' --no-daemon`
Expected: PASS.

- [ ] **Step 6: Postgres container test**

Follow the pattern of `src/test/kotlin/dev/brewery/daemon/db/ContainerSchemaTest.kt` (Docker check and skipping). Create `ContainerDbToolsTest.kt` that starts `PostgreSQLContainer`, creates `create table pet(id serial primary key, name text)` with 3 rows, writes `application.yml` with `${PG_URL}`/`${PG_USER}`/`${PG_PASSWORD}` placeholders, builds `DbTools(root, env = mapOf("PG_URL" to c.jdbcUrl, "PG_USER" to c.username, "PG_PASSWORD" to c.password)::get)` and asserts:
- `db_query` `select name from pet order by id` → 3 rows;
- `db_query` `delete from pet` → `McpToolError`;
- `db_query` `select pg_catalog.set_config('x.y','1',false)` succeeds but `with d as (delete from pet returning 1) select * from d` → `McpToolError` (writing CTE caught by `SqlText`);
- after all calls `select count(*) from pet` → `3`.

Run: `cd brewery-daemon && ./gradlew test --tests 'dev.brewery.daemon.mcp.ContainerDbToolsTest' --no-daemon` (skips without Docker locally; runs on CI ubuntu).

- [ ] **Step 7: Commit**

```bash
git add brewery-daemon/src
git commit -m "daemon: read-only MCP database tools on the Spring datasource"
```

---

### Task 5: Attach the daemon jar to releases

**Files:**
- Modify: `script/brewery/release_assets.py:60-68` (`package_names`)
- Modify: `script/brewery/test_release_assets.py:237-247` (`make_artifacts`)
- Modify: `.github/workflows/brewery_ci.yml` (daemon job: extra upload on ubuntu)

**Interfaces:**
- Produces: release asset `brewery-daemon-<version>.jar` listed in `SHA256SUMS` (consumed by Task 8's extension).

- [ ] **Step 1: Failing test** — in `make_artifacts` add `"package-daemon": ["daemon"]` to `layout`. Run `python -m unittest discover -v -s script/brewery -p 'test_re*.py'` → FAIL (`KeyError: 'daemon'`).

- [ ] **Step 2: Implement** — add to the dict returned by `package_names`:

```python
        "daemon": f"brewery-daemon-{version}.jar",
```

Run the same command → PASS (`test_assemble_everything` now also expects the jar because it iterates `names.values()`).

- [ ] **Step 3: CI artifact** — in `brewery_ci.yml`'s `daemon` job, after the existing `upload-artifact` step, add:

```yaml
      # The release attaches the daemon jar for Brewery Lite (extensions/brewery-lite downloads it).
      - name: Name the daemon jar for the release
        if: matrix.os == 'ubuntu-24.04'
        shell: bash
        run: |
          mkdir -p target/package-daemon
          cp brewery-daemon/build/libs/brewery-daemon-all.jar "target/package-daemon/brewery-daemon-$(tr -d '[:space:]' < crates/zed/BREWERY_VERSION).jar"
      - if: matrix.os == 'ubuntu-24.04'
        uses: actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02 # v4
        with:
          name: package-daemon
          path: target/package-daemon
          retention-days: 14
```

Check that the release's `ci` call always runs the daemon job (look at how `changes` outputs are forced on `workflow_call`; the bundle jobs already depend on the daemon jar, so it should). Run `python -m unittest -v script/brewery/test_ci_workflow.py` → PASS (fix the test only if it pins the daemon job's step list).

- [ ] **Step 4: Commit**

```bash
git add script/brewery/release_assets.py script/brewery/test_release_assets.py .github/workflows/brewery_ci.yml
git commit -m "release: attach the daemon jar for Brewery Lite"
```

---

### Task 6: Extension scaffold, theme and icons, sync check, CI job

**Files:**
- Create: `extensions/brewery-lite/extension.toml`, `Cargo.toml`, `src/lib.rs` (stub), `LICENSE-APACHE`, `README.md`, `.gitignore`
- Create: `script/brewery/sync_brewery_lite.py`, `script/brewery/test_sync_brewery_lite.py`
- Generated by the script: `extensions/brewery-lite/themes/brewery.json`, `extensions/brewery-lite/icon_themes/brewery-icons.json`, `extensions/brewery-lite/icons/**`, `extensions/brewery-lite/icons/NOTICE`
- Modify: `.github/workflows/brewery_ci.yml` (path filter output `lite` + job `brewery-lite`)
- Modify: root `Cargo.toml` `[workspace] exclude` (add `extensions/brewery-lite` if other `extensions/*` are excluded there; check how `extensions/html` is handled)

**Interfaces:**
- Produces: `python script/brewery/sync_brewery_lite.py [--check]` — copies or verifies theme/icon files; exit 1 with the drifted paths under `--check`.

- [ ] **Step 1: Read the sources** — `crates/brewery_theme/icons/icon_theme.json` (paths it references, e.g. `icons/dark/...`), `crates/brewery_theme/icons/NOTICE`, `assets/themes/brewery/brewery.json` (top-level `"$schema"`, `"name"`, `"author"`, `"themes"`), and `extensions/html/extension.toml` + `extensions/html/Cargo.toml` for the expected format. Zed loads icon themes from `icon_themes/*.json`, with icon paths relative to the extension root.

- [ ] **Step 2: Failing test for the sync script**

```python
import importlib.util
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("sync_brewery_lite", HERE / "sync_brewery_lite.py")
sync = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sync)


class SyncTest(unittest.TestCase):
    def make_repo(self, root: Path):
        (root / "assets/themes/brewery").mkdir(parents=True)
        (root / "assets/themes/brewery/brewery.json").write_text('{"name":"Brewery"}\n')
        icons = root / "crates/brewery_theme/icons"
        (icons / "dark").mkdir(parents=True)
        (icons / "dark/java.svg").write_text("<svg/>")
        (icons / "NOTICE").write_text("Apache-2.0 icons\n")
        (icons / "icon_theme.json").write_text('{"themes":[{"file_icons":{"java":{"path":"./icons/dark/java.svg"}}}]}\n')

    def test_copy_then_check_passes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.make_repo(root)
            self.assertEqual(sync.sync(root, check=False), [])
            ext = root / "extensions/brewery-lite"
            self.assertTrue((ext / "themes/brewery.json").is_file())
            self.assertTrue((ext / "icon_themes/brewery-icons.json").is_file())
            self.assertTrue((ext / "icons/dark/java.svg").is_file())
            self.assertTrue((ext / "icons/NOTICE").is_file())
            self.assertEqual(sync.sync(root, check=True), [])

    def test_check_reports_drift(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.make_repo(root)
            sync.sync(root, check=False)
            (root / "crates/brewery_theme/icons/dark/java.svg").write_text("<svg changed/>")
            (root / "extensions/brewery-lite/icons/dark/stale.svg").write_text("<svg/>")
            drift = sync.sync(root, check=True)
            self.assertIn("icons/dark/java.svg", drift)
            self.assertIn("icons/dark/stale.svg", drift)


if __name__ == "__main__":
    unittest.main()
```

Run `python -m unittest -v script/brewery/test_sync_brewery_lite.py` → FAIL (no module).

- [ ] **Step 3: Implement `sync_brewery_lite.py`**

```python
"""Keeps extensions/brewery-lite's theme and icons identical to the IDE's.

    python script/brewery/sync_brewery_lite.py          # copy
    python script/brewery/sync_brewery_lite.py --check  # exit 1 and list drifted files
"""
import argparse
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
EXT = Path("extensions/brewery-lite")


def _wanted(repo: Path) -> dict:
    """{path relative to the extension: source bytes}."""
    files = {"themes/brewery.json": (repo / "assets/themes/brewery/brewery.json").read_bytes()}
    icons = repo / "crates/brewery_theme/icons"
    for path in sorted(p for p in icons.rglob("*") if p.is_file()):
        rel = path.relative_to(icons).as_posix()
        if rel == "icon_theme.json":
            files["icon_themes/brewery-icons.json"] = path.read_bytes()
        else:
            files[f"icons/{rel}"] = path.read_bytes()
    return files


def _managed(ext: Path) -> set:
    out = set()
    for top in ("themes", "icon_themes", "icons"):
        base = ext / top
        if base.exists():
            out |= {p.relative_to(ext).as_posix() for p in base.rglob("*") if p.is_file()}
    return out


def sync(repo: Path, check: bool) -> list:
    """Copies (or with check=True only compares) and returns the drifted relative paths."""
    ext = repo / EXT
    wanted = _wanted(repo)
    drift = sorted(
        [rel for rel, data in wanted.items() if not (ext / rel).is_file() or (ext / rel).read_bytes() != data]
        + [rel for rel in _managed(ext) if rel not in wanted]
    )
    if not check:
        for rel in drift:
            target = ext / rel
            if rel in wanted:
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(wanted[rel])
            else:
                target.unlink()
        return []
    return drift


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args(argv)
    drift = sync(REPO, args.check)
    for rel in drift:
        print(f"out of date: {EXT.as_posix()}/{rel}", file=sys.stderr)
    if drift:
        print("run: python script/brewery/sync_brewery_lite.py", file=sys.stderr)
    return 1 if drift else 0


if __name__ == "__main__":
    sys.exit(main())
```

Run the test → PASS. Then extend the script and its test: the `version = "..."` lines of `extensions/brewery-lite/extension.toml` and `Cargo.toml` must equal `crates/zed/BREWERY_VERSION` (sync rewrites them, `--check` reports `extension.toml`/`Cargo.toml` as drifted), so bumping BREWERY_VERSION for a release can't leave the extension pointing at the wrong daemon jar. Then run `python script/brewery/sync_brewery_lite.py` for real. If `icon_theme.json` references icons with a path prefix other than `./icons/...` relative to its own directory, rewrite the prefix in `_wanted` and add a test for it.

- [ ] **Step 4: Manifest and crate**

`extensions/brewery-lite/extension.toml` (read the real version from `crates/zed/BREWERY_VERSION`):

```toml
id = "brewery-lite"
name = "Brewery Lite"
description = "Spring Boot config completion, an MCP server for Java/Spring projects and databases, and Brewery's IntelliJ-style theme and icons."
version = "0.1.5"
schema_version = 1
authors = ["hteariH <https://github.com/hteariH>"]
repository = "https://github.com/hteariH/brewery"

[language_servers.brewery-spring-config]
name = "Brewery Spring Config"
languages = ["YAML", "Properties"]

[context_servers.brewery]

[[capabilities]]
kind = "process:exec"
command = "java"
args = ["-version"]
```

`extensions/brewery-lite/Cargo.toml`:

```toml
[package]
name = "brewery_lite"
version = "0.1.5"
edition = "2024"
publish = false
license = "Apache-2.0"

[lib]
path = "src/lib.rs"
crate-type = ["cdylib", "rlib"]

[dependencies]
zed_extension_api = "0.7.0"
sha2 = "0.10"
```

(`rlib` lets `cargo test` run the pure helpers on the host.) Stub `src/lib.rs`:

```rust
use zed_extension_api as zed;

struct BreweryLite;

impl zed::Extension for BreweryLite {
    fn new() -> Self {
        BreweryLite
    }
}

zed::register_extension!(BreweryLite);
```

`.gitignore`: `target/`, `*.wasm`, `Cargo.lock` is committed (reproducible extension builds). `LICENSE-APACHE`: copy `brewery-daemon/LICENSE` if it is the Apache text, otherwise the standard Apache-2.0 text. `README.md` (English): what it does, requirements (Java 21 found or downloaded), settings (`java_path` for both servers, shown below), the MCP tools list, a link to the full Brewery IDE, and the read-only guarantee of `db_query`.

```json
{
  "lsp": { "brewery-spring-config": { "settings": { "java_path": "/path/to/jdk-21/bin/java" } } },
  "context_servers": { "brewery": { "settings": { "java_path": "/path/to/jdk-21/bin/java" } } }
}
```

Make sure the root workspace ignores the crate: if `extensions/html` is a workspace member, add `"extensions/brewery-lite"` to `[workspace] exclude` in the root `Cargo.toml` instead (it must build for `wasm32-wasip2` with its own lockfile).

- [ ] **Step 5: Build**

```bash
rustup target add wasm32-wasip2
cd extensions/brewery-lite && cargo build --release --target wasm32-wasip2
```

Expected: `target/wasm32-wasip2/release/brewery_lite.wasm` exists.

- [ ] **Step 6: CI job** — in `brewery_ci.yml`'s `changes` job add an output `lite` with a filter `extensions/brewery-lite/**`, `crates/brewery_theme/icons/**`, `assets/themes/brewery/**`, `script/brewery/sync_brewery_lite.py` (follow how the existing outputs are computed). Add:

```yaml
  brewery-lite:
    name: Brewery Lite extension
    needs: changes
    if: needs.changes.outputs.lite == 'true'
    runs-on: ubuntu-24.04
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4
      - name: Theme and icons match the IDE
        run: python3 script/brewery/sync_brewery_lite.py --check
      - name: Test the sync script
        run: python3 -m unittest -v script/brewery/test_sync_brewery_lite.py
      - name: Add the wasm target
        run: rustup target add wasm32-wasip2
      - name: Test
        working-directory: extensions/brewery-lite
        run: cargo test --locked
      - name: Build
        working-directory: extensions/brewery-lite
        run: cargo build --locked --release --target wasm32-wasip2
```

Run `python -m unittest -v script/brewery/test_ci_workflow.py` and update it if it enumerates jobs.

- [ ] **Step 7: Commit**

```bash
git add extensions/brewery-lite script/brewery/sync_brewery_lite.py script/brewery/test_sync_brewery_lite.py .github/workflows/brewery_ci.yml Cargo.toml
git commit -m "Brewery Lite: extension scaffold with theme and icons"
```

---

### Task 7: Pure provisioning helpers

**Files:**
- Create: `extensions/brewery-lite/src/java.rs`, `extensions/brewery-lite/src/daemon_jar.rs`
- Modify: `extensions/brewery-lite/src/lib.rs` (`mod java; mod daemon_jar;`)

**Interfaces:**
- Produces (all pure, no `zed` calls):
  - `java::MIN_MAJOR: u32 = 21`
  - `java::parse_major(version_output: &str) -> Option<u32>` — parses `java -version` stderr (`openjdk version "21.0.5" ...`, `java version "1.8.0_412"` → 8, `"25-ea"` → 25)
  - `java::adoptium_url(os: zed::Os, arch: zed::Architecture) -> Result<String, String>` — `https://api.adoptium.net/v3/binary/latest/21/ga/{linux|mac|windows}/{x64|aarch64}/jre/hotspot/normal/eclipse`; x86 → `Err`
  - `java::java_in_jre(jre_dir_entries: &[String], os: zed::Os) -> Option<String>` — given the top-level entries of the extraction dir, the relative path of `java`: `<dir>/bin/java[.exe]`, on macOS `<dir>/Contents/Home/bin/java`
  - `daemon_jar::asset_name(version: &str) -> String` → `brewery-daemon-{version}.jar`
  - `daemon_jar::expected_sha256(sums: &str, name: &str) -> Option<String>` — parses `SHA256SUMS` (`<hex>  <name>` or `<hex> *<name>`)
  - `daemon_jar::sha256_hex(bytes: &[u8]) -> String`
  - `daemon_jar::stale_jars(entries: &[String], current: &str) -> Vec<String>` — other `brewery-daemon-*.jar` names

- [ ] **Step 1: Write the failing tests** (in each module's `#[cfg(test)] mod tests`)

```rust
// java.rs tests
#[test]
fn parses_java_version_output() {
    assert_eq!(parse_major("openjdk version \"21.0.5\" 2024-10-15 LTS\nOpenJDK Runtime Environment"), Some(21));
    assert_eq!(parse_major("java version \"1.8.0_412\"\nJava(TM) SE Runtime"), Some(8));
    assert_eq!(parse_major("openjdk version \"25-ea\" 2025-09-16"), Some(25));
    assert_eq!(parse_major("openjdk version \"17\" 2021-09-14"), Some(17));
    assert_eq!(parse_major("'java' is not recognized"), None);
}

#[test]
fn adoptium_urls_per_platform() {
    assert_eq!(
        adoptium_url(zed::Os::Mac, zed::Architecture::Aarch64).unwrap(),
        "https://api.adoptium.net/v3/binary/latest/21/ga/mac/aarch64/jre/hotspot/normal/eclipse"
    );
    assert_eq!(
        adoptium_url(zed::Os::Windows, zed::Architecture::X8664).unwrap(),
        "https://api.adoptium.net/v3/binary/latest/21/ga/windows/x64/jre/hotspot/normal/eclipse"
    );
    assert!(adoptium_url(zed::Os::Linux, zed::Architecture::X86).is_err());
}

#[test]
fn finds_java_in_extracted_jre() {
    let entries = vec!["jdk-21.0.5+11-jre".to_string()];
    assert_eq!(java_in_jre(&entries, zed::Os::Linux).as_deref(), Some("jdk-21.0.5+11-jre/bin/java"));
    assert_eq!(java_in_jre(&entries, zed::Os::Windows).as_deref(), Some("jdk-21.0.5+11-jre/bin/java.exe"));
    assert_eq!(java_in_jre(&entries, zed::Os::Mac).as_deref(), Some("jdk-21.0.5+11-jre/Contents/Home/bin/java"));
    assert_eq!(java_in_jre(&[], zed::Os::Linux), None);
}

// daemon_jar.rs tests
#[test]
fn asset_and_checksums() {
    assert_eq!(asset_name("0.1.6"), "brewery-daemon-0.1.6.jar");
    let sums = "aaa  Brewery-0.1.6-x86_64-setup.exe\nBBB *brewery-daemon-0.1.6.jar\n";
    assert_eq!(expected_sha256(sums, "brewery-daemon-0.1.6.jar").as_deref(), Some("bbb"));
    assert_eq!(expected_sha256(sums, "missing.jar"), None);
    assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}

#[test]
fn stale_jars_are_other_daemon_versions() {
    let entries = vec!["brewery-daemon-0.1.5.jar".into(), "brewery-daemon-0.1.6.jar".into(), "jre".into()];
    assert_eq!(stale_jars(&entries, "brewery-daemon-0.1.6.jar"), vec!["brewery-daemon-0.1.5.jar".to_string()]);
}
```

Run: `cd extensions/brewery-lite && cargo test` → FAIL (functions missing).

- [ ] **Step 2: Implement `java.rs`**

```rust
//! Java discovery helpers. Pure functions so they run under `cargo test` on the host.

use zed_extension_api as zed;

/// The daemon is compiled for Java 21 (`jvmToolchain(21)` in brewery-daemon/build.gradle.kts).
pub const MIN_MAJOR: u32 = 21;

/// The major version from `java -version` output (printed on stderr). `1.8` style versions report 8.
pub fn parse_major(version_output: &str) -> Option<u32> {
    let start = version_output.find("version \"")? + "version \"".len();
    let rest = &version_output[start..];
    let version = &rest[..rest.find('"')?];
    let mut parts = version.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty());
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 { parts.next()?.parse().ok() } else { Some(first) }
}

/// Download URL of the newest Temurin 21 JRE for this platform.
pub fn adoptium_url(os: zed::Os, arch: zed::Architecture) -> Result<String, String> {
    let os = match os {
        zed::Os::Mac => "mac",
        zed::Os::Linux => "linux",
        zed::Os::Windows => "windows",
    };
    let arch = match arch {
        zed::Architecture::Aarch64 => "aarch64",
        zed::Architecture::X8664 => "x64",
        zed::Architecture::X86 => return Err("Brewery Lite needs a 64-bit system for Java 21".into()),
    };
    Ok(format!("https://api.adoptium.net/v3/binary/latest/{MIN_MAJOR}/ga/{os}/{arch}/jre/hotspot/normal/eclipse"))
}

/// The `java` executable inside an extracted Temurin archive, given the extraction dir's entries.
pub fn java_in_jre(entries: &[String], os: zed::Os) -> Option<String> {
    let top = entries.iter().find(|e| e.starts_with("jdk"))?;
    Some(match os {
        zed::Os::Mac => format!("{top}/Contents/Home/bin/java"),
        zed::Os::Windows => format!("{top}/bin/java.exe"),
        zed::Os::Linux => format!("{top}/bin/java"),
    })
}
```

- [ ] **Step 3: Implement `daemon_jar.rs`**

```rust
//! Daemon jar helpers: release asset names and checksum checks.

use sha2::{Digest, Sha256};

pub fn asset_name(version: &str) -> String {
    format!("brewery-daemon-{version}.jar")
}

/// The digest `SHA256SUMS` lists for `name` (lowercase), in `sha256sum` format (`<hex>  <name>` or `<hex> *<name>`).
pub fn expected_sha256(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hex, file) = line.split_once(char::is_whitespace)?;
        let file = file.trim_start().trim_start_matches('*');
        (file == name).then(|| hex.to_ascii_lowercase())
    })
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Daemon jars of other versions, left behind by earlier extension versions.
pub fn stale_jars(entries: &[String], current: &str) -> Vec<String> {
    entries
        .iter()
        .filter(|e| e.starts_with("brewery-daemon-") && e.ends_with(".jar") && e.as_str() != current)
        .cloned()
        .collect()
}
```

If `zed::Os`/`zed::Architecture` cannot be constructed in host tests (WIT bindings types are plain enums, so they should be), introduce local `enum Os`/`enum Arch` in `java.rs` and convert in `lib.rs`.

- [ ] **Step 4: Run tests and wasm build**

```bash
cd extensions/brewery-lite && cargo test && cargo build --release --target wasm32-wasip2
```

Expected: tests PASS, wasm builds (unused-function warnings allowed until Task 8; do not add `#[allow]`).

- [ ] **Step 5: Commit**

```bash
git add extensions/brewery-lite
git commit -m "Brewery Lite: Java and daemon jar helpers"
```

---

### Task 8: Extension wiring (LSP + MCP provisioning)

**Files:**
- Modify: `extensions/brewery-lite/src/lib.rs`

**Interfaces:**
- Consumes: Task 7 helpers; `zed_extension_api` 0.7.0: `github_release_by_tag_name`, `download_file`, `DownloadedFileType`, `current_platform`, `make_file_executable`, `set_language_server_installation_status`, `LanguageServerInstallationStatus`, `settings::LspSettings::for_worktree`, `settings::ContextServerSettings::for_project`, `process::Command`, `Worktree::{shell_env, which}`, `Command { command, args, env }`.
- Produces: working `language_server_command` (id `brewery-spring-config`) and `context_server_command` (id `brewery`).

Before writing, read `~/.cargo/registry/src/*/zed_extension_api-0.7.0/src/extension_api.rs` to confirm exact names and signatures (`github_release_by_tag_name(repo, tag) -> Result<GithubRelease>`, `GithubReleaseAsset { name, download_url }`, `download_file(url, path, file_type)`, `Command { command: String, args: Vec<String>, env: Vec<(String,String)> }`, `ContextServerSettings::for_project(id, project)`). Adjust the code below to the real signatures; the logic stays.

- [ ] **Step 1: Implement `lib.rs`**

```rust
//! Brewery Lite: runs the Brewery daemon as Zed's Spring config language server and as an MCP server.
//! The daemon jar comes from the GitHub release matching this extension's version; Java 21 comes from
//! settings, the environment or a downloaded Temurin JRE, in that order.

mod daemon_jar;
mod java;

use std::fs;
use zed_extension_api::{self as zed, settings::ContextServerSettings, settings::LspSettings};

const REPO: &str = "hteariH/brewery";
const LSP_ID: &str = "brewery-spring-config";
const MCP_ID: &str = "brewery";
const JRE_DIR: &str = "jre";
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Default)]
struct BreweryLite {
    jar: Option<String>,
    java: Option<String>,
}

fn java_path_setting(settings: Option<serde_json::Value>) -> Option<String> {
    settings?.get("java_path")?.as_str().map(str::to_owned)
}

impl BreweryLite {
    /// Path of the verified daemon jar in the extension's work dir, downloading it on first use.
    fn daemon_jar(&mut self, status: &dyn Fn(zed::LanguageServerInstallationStatus)) -> Result<String, String> {
        if let Some(jar) = &self.jar {
            if fs::metadata(jar).is_ok() {
                return Ok(jar.clone());
            }
        }
        let name = daemon_jar::asset_name(VERSION);
        if fs::metadata(&name).is_err() {
            status(zed::LanguageServerInstallationStatus::Downloading);
            let release = zed::github_release_by_tag_name(REPO, &format!("v{VERSION}"))
                .map_err(|e| format!("Brewery {VERSION} release not found on GitHub ({e})"))?;
            let asset = |n: &str| release.assets.iter().find(|a| a.name == n).map(|a| a.download_url.clone());
            let jar_url = asset(&name).ok_or_else(|| format!("{name} is missing from the v{VERSION} release"))?;
            let sums_url = asset("SHA256SUMS").ok_or("SHA256SUMS is missing from the release")?;
            let partial = format!("{name}.partial");
            zed::download_file(&jar_url, &partial, zed::DownloadedFileType::Uncompressed)?;
            zed::download_file(&sums_url, "SHA256SUMS", zed::DownloadedFileType::Uncompressed)?;
            let sums = fs::read_to_string("SHA256SUMS").map_err(|e| e.to_string())?;
            let expected = daemon_jar::expected_sha256(&sums, &name).ok_or("the release's SHA256SUMS does not list the daemon jar")?;
            let actual = daemon_jar::sha256_hex(&fs::read(&partial).map_err(|e| e.to_string())?);
            if actual != expected {
                let _ = fs::remove_file(&partial);
                return Err(format!("{name}: checksum mismatch (expected {expected}, got {actual})"));
            }
            fs::rename(&partial, &name).map_err(|e| e.to_string())?;
            let entries: Vec<String> = fs::read_dir(".").map_err(|e| e.to_string())?
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .collect();
            for stale in daemon_jar::stale_jars(&entries, &name) {
                let _ = fs::remove_file(stale);
            }
        }
        let jar = std::env::current_dir().map_err(|e| e.to_string())?.join(&name).to_string_lossy().into_owned();
        self.jar = Some(jar.clone());
        Ok(jar)
    }

    /// `java -version` through process:exec; Some(path) when it reports 21+.
    fn java_on_path(program: &str) -> Option<String> {
        let output = zed::process::Command::new(program).arg("-version").output().ok()?;
        let text = String::from_utf8_lossy(&output.stderr).into_owned() + &String::from_utf8_lossy(&output.stdout);
        (java::parse_major(&text)? >= java::MIN_MAJOR).then(|| program.to_owned())
    }

    /// The downloaded Temurin JRE, fetching it on first use.
    fn downloaded_java(&mut self, status: &dyn Fn(zed::LanguageServerInstallationStatus)) -> Result<String, String> {
        let (os, arch) = zed::current_platform();
        let list = |dir: &str| -> Vec<String> {
            fs::read_dir(dir).map(|d| d.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect()).unwrap_or_default()
        };
        let mut found = java::java_in_jre(&list(JRE_DIR), os);
        if found.is_none() {
            status(zed::LanguageServerInstallationStatus::Downloading);
            let url = java::adoptium_url(os, arch)?;
            let kind = if os == zed::Os::Windows { zed::DownloadedFileType::Zip } else { zed::DownloadedFileType::GzipTar };
            zed::download_file(&url, JRE_DIR, kind).map_err(|e| format!("downloading Java {}: {e}", java::MIN_MAJOR))?;
            found = java::java_in_jre(&list(JRE_DIR), os);
        }
        let rel = found.ok_or("the downloaded JRE has no bin/java")?;
        let path = std::env::current_dir().map_err(|e| e.to_string())?.join(JRE_DIR).join(rel).to_string_lossy().into_owned();
        if os != zed::Os::Windows {
            zed::make_file_executable(&path)?;
        }
        Ok(path)
    }

    fn java(
        &mut self,
        setting: Option<String>,
        from_worktree: Option<String>,
        status: &dyn Fn(zed::LanguageServerInstallationStatus),
    ) -> Result<String, String> {
        if let Some(path) = setting {
            return Ok(path);
        }
        if let Some(java) = &self.java {
            return Ok(java.clone());
        }
        let java = match from_worktree.or_else(|| Self::java_on_path("java")) {
            Some(java) => java,
            None => self.downloaded_java(status)?,
        };
        self.java = Some(java.clone());
        Ok(java)
    }
}

impl zed::Extension for BreweryLite {
    fn new() -> Self {
        Self::default()
    }

    fn language_server_command(&mut self, id: &zed::LanguageServerId, worktree: &zed::Worktree) -> zed::Result<zed::Command> {
        let status = |s| zed::set_language_server_installation_status(id, &s);
        let setting = LspSettings::for_worktree(LSP_ID, worktree).ok().and_then(|s| java_path_setting(s.settings));
        // JAVA_HOME and PATH of the worktree's shell; accepted without a version probe, the daemon's
        // UnsupportedClassVersionError in the LSP log says what is wrong if it is older than 21.
        let env = worktree.shell_env();
        let separator = if zed::current_platform().0 == zed::Os::Windows { "\\" } else { "/" };
        let exe = if zed::current_platform().0 == zed::Os::Windows { "java.exe" } else { "java" };
        let from_worktree = env.iter().find(|(k, _)| k == "JAVA_HOME")
            .map(|(_, home)| format!("{home}{separator}bin{separator}{exe}"))
            .or_else(|| worktree.which("java"));
        let java = self.java(setting, from_worktree, &status)?;
        let jar = self.daemon_jar(&status)?;
        status(zed::LanguageServerInstallationStatus::None);
        Ok(zed::Command { command: java, args: vec!["-jar".into(), jar, "--spring-config-lsp".into()], env: Vec::new() })
    }

    fn context_server_command(&mut self, _id: &zed::ContextServerId, project: &zed::Project) -> zed::Result<zed::Command> {
        let setting = ContextServerSettings::for_project(MCP_ID, project).ok().and_then(|s| java_path_setting(s.settings));
        let quiet = |_| {};
        let java = self.java(setting, None, &quiet)?;
        let jar = self.daemon_jar(&quiet)?;
        Ok(zed::Command { command: java, args: vec!["-jar".into(), jar, "--mcp".into()], env: Vec::new() })
    }
}

zed::register_extension!(BreweryLite);
```

Notes for the implementer:
- `serde_json` is re-exported as `zed::serde_json`; use that (no extra dependency).
- A JAVA_HOME from the worktree that is older than 21 is a known limitation (documented in the README): the extension cannot probe arbitrary paths with `process:exec` limited to `java`.
- If `context_server_configuration` is required for the server to appear in Zed's agent settings, implement it returning `ContextServerConfiguration { installation_instructions: <README excerpt>, settings_schema: <json schema with optional java_path>, default_settings: "{}" }`.

- [ ] **Step 2: Build and unit tests**

```bash
cd extensions/brewery-lite && cargo test && cargo build --release --target wasm32-wasip2 && cargo clippy --target wasm32-wasip2 -- -D warnings
```

Expected: PASS, no warnings.

- [ ] **Step 3: Commit**

```bash
git add extensions/brewery-lite
git commit -m "Brewery Lite: start the daemon as Spring config LSP and MCP server"
```

---

### Task 9: End-to-end check, docs, changelog

**Files:**
- Modify: `docs/brewery/FORK.md` (new "Brewery Lite" section)
- Modify: `CHANGELOG.md` (Unreleased: "Brewery Lite extension for Zed")
- Modify: `extensions/brewery-lite/README.md` (anything learned in the e2e check)

- [ ] **Step 1: MCP smoke against a real project** — build the jar and talk to it:

```bash
cd brewery-daemon && ./gradlew shadowJar --no-daemon
cd src/test/resources/projects/spring-petshop
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}' '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"spring_endpoints"}}' | java -jar ../../../../../build/libs/brewery-daemon-all.jar --mcp
```

Expected: three response lines; tool list has 6 tools; endpoints non-empty.

- [ ] **Step 2: Dev-extension check in a Brewery/Zed build** — the jar is not on a published release yet, so for local testing temporarily copy `brewery-daemon-all.jar` as `brewery-daemon-<version>.jar` into the extension's work dir (`%LOCALAPPDATA%\Zed\extensions\work\brewery-lite\` on Windows, or Brewery's equivalent under its data dir) — `daemon_jar` skips the download when the file exists. Install with "zed: install dev extension" pointing at `extensions/brewery-lite`. Verify:
  - the Spring LSP starts on `brewery-daemon/src/test/resources/projects/spring-petshop` (completion of `server.` in `application.yml`; check the LSP log);
  - the `brewery` context server shows as running in the agent settings, and the agent can call `spring_endpoints`.
  Take a window screenshot as proof (no synthetic mouse input; ask the user to click if something needs clicking).

- [ ] **Step 3: Docs** — FORK.md section covering: what Lite contains and why the rest can't be an extension (no UI extension points), the version coupling (extension version == BREWERY_VERSION, jar from release `v<version>`), how to test locally (Step 2), and publishing: after the release `v<version>` is published, the owner opens a PR to `zed-industries/extensions` adding the submodule with `path = "extensions/brewery-lite"` and the version in `extensions.toml`; future releases bump that entry. CHANGELOG Unreleased entry.

- [ ] **Step 4: Run every affected suite**

```bash
cd brewery-daemon && ./gradlew test --no-daemon
python -m unittest discover -v -s script/brewery
cd extensions/brewery-lite && cargo test && cargo build --release --target wasm32-wasip2
python script/brewery/sync_brewery_lite.py --check
```

Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add docs/brewery/FORK.md CHANGELOG.md extensions/brewery-lite/README.md
git commit -m "docs: Brewery Lite"
```
