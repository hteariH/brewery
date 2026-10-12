package dev.brewery.daemon.springconfig

import org.eclipse.lsp4j.ClientCapabilities
import org.eclipse.lsp4j.CompletionItem
import org.eclipse.lsp4j.CompletionParams
import org.eclipse.lsp4j.Diagnostic
import org.eclipse.lsp4j.DiagnosticSeverity
import com.google.gson.JsonObject
import org.eclipse.lsp4j.DidChangeTextDocumentParams
import org.eclipse.lsp4j.DidChangeWatchedFilesCapabilities
import org.eclipse.lsp4j.DidChangeWatchedFilesParams
import org.eclipse.lsp4j.DidOpenTextDocumentParams
import org.eclipse.lsp4j.DidSaveTextDocumentParams
import org.eclipse.lsp4j.FileChangeType
import org.eclipse.lsp4j.FileEvent
import org.eclipse.lsp4j.MessageType
import org.eclipse.lsp4j.RegistrationParams
import org.eclipse.lsp4j.WorkspaceClientCapabilities
import org.eclipse.lsp4j.HoverParams
import org.eclipse.lsp4j.InitializeParams
import org.eclipse.lsp4j.InitializedParams
import org.eclipse.lsp4j.MessageActionItem
import org.eclipse.lsp4j.MessageParams
import org.eclipse.lsp4j.Position
import org.eclipse.lsp4j.PublishDiagnosticsParams
import org.eclipse.lsp4j.ShowMessageRequestParams
import org.eclipse.lsp4j.TextDocumentContentChangeEvent
import org.eclipse.lsp4j.TextDocumentIdentifier
import org.eclipse.lsp4j.TextDocumentItem
import org.eclipse.lsp4j.TextDocumentSyncKind
import org.eclipse.lsp4j.VersionedTextDocumentIdentifier
import org.eclipse.lsp4j.launch.LSPLauncher
import org.eclipse.lsp4j.services.LanguageClient
import org.eclipse.lsp4j.services.LanguageServer
import java.io.File
import java.io.PipedInputStream
import java.io.PipedOutputStream
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.CompletableFuture
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

private class RecordingClient : LanguageClient {
    val diagnostics = LinkedBlockingQueue<PublishDiagnosticsParams>()
    val logs = LinkedBlockingQueue<MessageParams>()
    val registrations = LinkedBlockingQueue<RegistrationParams>()
    override fun publishDiagnostics(d: PublishDiagnosticsParams) { diagnostics.add(d) }
    override fun telemetryEvent(o: Any?) {}
    override fun showMessage(m: MessageParams?) {}
    override fun showMessageRequest(r: ShowMessageRequestParams?): CompletableFuture<MessageActionItem> =
        CompletableFuture.completedFuture(null)
    override fun logMessage(m: MessageParams) { logs.add(m) }
    override fun registerCapability(params: RegistrationParams): CompletableFuture<Void> {
        registrations.add(params)
        return CompletableFuture.completedFuture(null)
    }
}

class SpringConfigServerTest {
    private val root: File = Files.createTempDirectory("spring-lsp").toFile()
    private val jar = makeJar(root, "a.jar", mapOf("META-INF/spring-configuration-metadata.json" to MAIN_METADATA))
    private val client = RecordingClient()
    private val closers = ArrayList<() -> Unit>()

    private val loads = AtomicInteger()

    private fun start(
        metadata: SpringMetadata = SpringMetadata.fromClasspath(listOf(jar)),
        gate: CompletableFuture<Unit>? = null,
        capabilities: ClientCapabilities = ClientCapabilities(),
        load: (Path, Resolution) -> SpringMetadata = { _, _ -> gate?.get(); metadata },
    ): LanguageServer {
        val server = SpringConfigServer(
            loadMetadata = { root, resolution -> loads.incrementAndGet(); load(root, resolution) },
            debounceMs = 50,
            reloadDebounceMs = 150,
        )
        val c2s = PipedOutputStream()
        val s2c = PipedOutputStream()
        val serverIn = PipedInputStream(c2s)
        val clientIn = PipedInputStream(s2c)
        val serverLauncher = LSPLauncher.createServerLauncher(server, serverIn, s2c)
        server.connect(serverLauncher.remoteProxy)
        serverLauncher.startListening()
        val clientLauncher = LSPLauncher.createClientLauncher(client, clientIn, c2s)
        clientLauncher.startListening()
        closers += { c2s.close(); s2c.close() }
        val proxy = clientLauncher.remoteProxy
        val init = InitializeParams().apply {
            rootUri = root.toURI().toString()
            this.capabilities = capabilities
        }
        proxy.initialize(init).get(10, TimeUnit.SECONDS)
        proxy.initialized(InitializedParams())
        return proxy
    }

    @AfterTest
    fun tearDown() { closers.forEach { runCatching(it) } }

    private fun uri(name: String) = File(root, name).toURI().toString()

    private fun open(server: LanguageServer, name: String, text: String): String {
        val u = uri(name)
        server.textDocumentService.didOpen(DidOpenTextDocumentParams(TextDocumentItem(u, "properties", 1, text)))
        return u
    }

    private fun complete(server: LanguageServer, uri: String, line: Int, col: Int): List<CompletionItem> =
        server.textDocumentService.completion(CompletionParams(TextDocumentIdentifier(uri), Position(line, col)))
            .get(10, TimeUnit.SECONDS).left.orEmpty()

    /** Metadata loads in the background, so poll a request until it stops being empty. */
    private fun completeEventually(server: LanguageServer, uri: String, line: Int, col: Int): List<CompletionItem> {
        val deadline = System.currentTimeMillis() + 10_000
        while (true) {
            val items = complete(server, uri, line, col)
            if (items.isNotEmpty() || System.currentTimeMillis() > deadline) return items
            Thread.sleep(50)
        }
    }

    private fun nextDiagnostics(uri: String): List<Diagnostic> {
        val deadline = System.currentTimeMillis() + 10_000
        while (System.currentTimeMillis() < deadline) {
            val p = client.diagnostics.poll(200, TimeUnit.MILLISECONDS) ?: continue
            if (p.uri == uri) return p.diagnostics
        }
        error("no diagnostics for $uri")
    }

    @Test
    fun `initialize advertises the capabilities and server name`() {
        val server = SpringConfigServer(loadMetadata = { _, _ -> SpringMetadata.EMPTY })
        val result = server.initialize(InitializeParams()).get()
        assertEquals("brewery-spring-config", result.serverInfo.name)
        assertEquals(setOf(".", ":", "="), result.capabilities.completionProvider.triggerCharacters.toSet())
        assertTrue(result.capabilities.hoverProvider.left)
        val sync = result.capabilities.textDocumentSync.right
        assertEquals(TextDocumentSyncKind.Full, sync.change)
        assertTrue(sync.openClose)
        assertNotNull(sync.save)
    }

    @Test
    fun `completes keys at an empty line`() {
        val server = start()
        val u = open(server, "application.properties", "\n")
        val items = completeEventually(server, u, 0, 0)
        val port = assertNotNull(items.firstOrNull { it.label == "server.port" })
        assertEquals("Integer", port.detail)
        assertTrue(port.documentation.right.value.contains("Server HTTP port."))
        assertTrue(port.documentation.right.value.contains("Default: `8080`"))
        assertTrue(items.first { it.label == "old.setting" }.tags.isNotEmpty())
    }

    @Test
    fun `completes boolean and hint values`() {
        val server = start()
        val u = open(server, "application.properties", "spring.jpa.show-sql=\nspring.main.banner-mode=\n")
        assertEquals(listOf("true", "false"), completeEventually(server, u, 0, 21).map { it.label })
        val hints = complete(server, u, 1, 25)
        assertEquals(listOf("console", "off"), hints.map { it.label })
        assertEquals("Print to stdout.", hints[0].documentation.left)
    }

    @Test
    fun `completes yaml keys under the parent`() {
        val server = start()
        val u = open(server, "application.yml", "spring:\n  jpa:\n    \n")
        val labels = completeEventually(server, u, 2, 4).map { it.label }.sorted()
        assertEquals(listOf("properties", "show-sql"), labels)
        val top = complete(server, u, 0, 0).map { it.label }
        assertTrue("server" in top && "spring" in top)
    }

    @Test
    fun `hover returns the description`() {
        val server = start()
        val u = open(server, "application.properties", "server.port=1\n")
        completeEventually(server, u, 0, 0) // waits for the metadata
        val hover = server.textDocumentService.hover(HoverParams(TextDocumentIdentifier(u), Position(0, 3)))
            .get(10, TimeUnit.SECONDS)
        assertTrue(hover.contents.right.value.contains("Server HTTP port."))
        assertNull(
            server.textDocumentService.hover(HoverParams(TextDocumentIdentifier(u), Position(0, 12)))
                .get(10, TimeUnit.SECONDS),
        )
    }

    @Test
    fun `reports unknown and deprecated keys`() {
        val server = start()
        val text = listOf(
            "server.port=1", "bogus.key=1", "old.setting=x", "spring.jpa.properties.a.b=1", "logging.level.com.x=DEBUG", "",
        ).joinToString("\n")
        val u = open(server, "application.properties", text)
        val diags = nextDiagnostics(u)
        assertEquals(2, diags.size, diags.toString())
        val unknown = diags.first { it.range.start.line == 1 }
        assertEquals(DiagnosticSeverity.Warning, unknown.severity)
        assertEquals("Unknown Spring Boot property 'bogus.key'", unknown.message.left)
        assertEquals(0, unknown.range.start.character)
        assertEquals(9, unknown.range.end.character)
        val deprecated = diags.first { it.range.start.line == 2 }
        assertEquals(DiagnosticSeverity.Warning, deprecated.severity)
        assertTrue(deprecated.message.left.contains("new.setting"), deprecated.message.left)
    }

    @Test
    fun `reports yaml keys and republishes on change`() {
        val server = start()
        val u = open(server, "application.yml", "server:\n  port: 1\n  nope: 2\n")
        assertEquals(listOf("Unknown Spring Boot property 'server.nope'"), nextDiagnostics(u).map { it.message.left })
        server.textDocumentService.didChange(
            DidChangeTextDocumentParams(
                VersionedTextDocumentIdentifier(u, 2),
                listOf(TextDocumentContentChangeEvent("server:\n  port: 1\n")),
            ),
        )
        assertEquals(emptyList<Diagnostic>(), nextDiagnostics(u))
    }

    @Test
    fun `other files get nothing`() {
        val server = start()
        val u = open(server, "config.properties", "bogus.key=1\n")
        val o = open(server, "application.properties", "bogus.key=1\n")
        assertEquals(1, nextDiagnostics(o).size)
        assertTrue(client.diagnostics.none { it.uri == u })
        assertEquals(emptyList<CompletionItem>(), complete(server, u, 0, 0))
    }

    @Test
    fun `stays silent until the metadata is loaded and when there is none`() {
        val gate = CompletableFuture<Unit>()
        val server = start(gate = gate)
        val u = open(server, "application.properties", "bogus.key=1\n")
        assertEquals(emptyList<CompletionItem>(), complete(server, u, 0, 0))
        assertNull(client.diagnostics.poll(400, TimeUnit.MILLISECONDS))
        gate.complete(Unit)
        assertEquals(1, nextDiagnostics(u).size)

        val bare = start(metadata = SpringMetadata.EMPTY)
        val v = open(bare, "application-dev.properties", "bogus.key=1\n")
        assertNull(client.diagnostics.poll(600, TimeUnit.MILLISECONDS)?.takeIf { it.uri == v && it.diagnostics.isNotEmpty() })
        assertEquals(emptyList<CompletionItem>(), complete(bare, v, 0, 0))
    }

    @Test
    fun `no false unknown warnings for project metadata, object lists, Properties maps and groups`() {
        val project = writeMetadataDir(
            File(root, "src/main/resources"),
            """{"properties":[{"name":"app.custom","type":"java.lang.String"}]}""",
        )
        val shapes = makeJar(root, "shapes.jar", mapOf("META-INF/spring-configuration-metadata.json" to SHAPES_METADATA))
        val server = start(metadata = SpringMetadata.fromClasspath(listOf(project, jar, shapes)))
        val yaml = open(
            server, "application.yml",
            listOf(
                "app:", "  custom: x", "server:", "spring:", "  cloud:", "    gateway:", "      routes:",
                "        - id: one", "          uri: http://a", "  datasource:", "    hikari:",
                "      data-source-properties:", "        cachePrepStmts: true", "      maximum-pool-sise: 3", "",
            ).joinToString("\n"),
        )
        assertEquals(
            listOf("Unknown Spring Boot property 'spring.datasource.hikari.maximum-pool-sise'"),
            nextDiagnostics(yaml).map { it.message.left },
        )
        val props = open(
            server, "application.properties",
            listOf(
                "app.custom=x", "spring.cloud.gateway.routes[0].id=one",
                "spring.datasource.hikari.data-source-properties.cachePrepStmts=true", "server=", "server.prot=1", "",
            ).joinToString("\n"),
        )
        assertEquals(listOf("Unknown Spring Boot property 'server.prot'"), nextDiagnostics(props).map { it.message.left })
    }

    private fun List<CompletionItem>.edit(label: String) = first { it.label == label }.textEdit.left

    @Test
    fun `yaml key completion inserts a colon only where none is there yet`() {
        val server = start()
        val u = open(server, "application.yml", "spring:\n  jpa:\n    \n    sh: x\n")
        val fresh = completeEventually(server, u, 2, 4)
        assertEquals("show-sql: ", fresh.edit("show-sql").newText)
        // A map leaf and a group take the colon alone: what follows is a nested block.
        assertEquals("properties:", fresh.edit("properties").newText)
        assertEquals("server:", complete(server, u, 2, 0).edit("server").newText)

        val existing = complete(server, u, 3, 6).edit("show-sql")
        assertEquals("show-sql", existing.newText)
        assertEquals(4, existing.range.start.character)
        assertEquals(6, existing.range.end.character)
    }

    @Test
    fun `yaml value completion right after the colon adds the space`() {
        val server = start()
        val u = open(server, "application.yml", "spring:\n  jpa:\n    show-sql:\n  main:\n    banner-mode: \n")
        val tight = completeEventually(server, u, 2, 13).first { it.label == "true" }
        assertEquals(" true", tight.textEdit.left.newText)
        assertEquals(13, tight.textEdit.left.range.start.character)
        assertEquals("true", tight.filterText ?: tight.label)
        assertEquals("console", complete(server, u, 4, 17).edit("console").newText)

        val props = open(server, "application.properties", "spring.jpa.show-sql=\n")
        assertEquals("true", complete(server, props, 0, 20).edit("true").newText)
    }

    private fun awaitLoads(n: Int) {
        val deadline = System.currentTimeMillis() + 10_000
        while (loads.get() < n && System.currentTimeMillis() < deadline) Thread.sleep(20)
        assertEquals(n, loads.get())
    }

    private fun changed(server: LanguageServer, vararg names: String) =
        server.workspaceService.didChangeWatchedFiles(
            DidChangeWatchedFilesParams(names.map { FileEvent(uri(it), FileChangeType.Changed) }),
        )

    @Test
    fun `the classpath is resolved on the first Spring config file, not on initialized`() {
        val server = start()
        Thread.sleep(300)
        assertEquals(0, loads.get())
        // Build changes before any Spring file is open do not start anything either.
        changed(server, "build.gradle.kts")
        open(server, "config.properties", "a=1\n")
        Thread.sleep(400)
        assertEquals(0, loads.get())
        open(server, "application.yml", "server:\n  port: 1\n")
        awaitLoads(1)
        open(server, "application-dev.properties", "server.port=1\n")
        Thread.sleep(300)
        assertEquals(1, loads.get())
    }

    @Test
    fun `build file changes re-resolve the classpath once per burst`() {
        val bogus = makeJar(
            root, "bogus.jar",
            mapOf("META-INF/spring-configuration-metadata.json" to """{"properties":[{"name":"bogus.key","type":"java.lang.String"}]}"""),
        )
        val before = SpringMetadata.fromClasspath(listOf(jar))
        val after = SpringMetadata.fromClasspath(listOf(jar, bogus))
        val server = start(load = { _, _ -> if (loads.get() == 1) before else after })
        val u = open(server, "application.properties", "bogus.key=1\n")
        assertEquals(1, nextDiagnostics(u).size)
        changed(server, "src/main/java/A.java")
        changed(server, "build.gradle.kts", "settings.gradle")
        changed(server, "gradle/libs.versions.toml")
        // The first load may publish twice (open and load completion); wait for the reloaded result.
        while (nextDiagnostics(u).isNotEmpty()) continue
        awaitLoads(2)
        Thread.sleep(400)
        assertEquals(2, loads.get())

        // A save of a build file the client routed to this server is the fallback when nothing is watched.
        server.textDocumentService.didSave(DidSaveTextDocumentParams(TextDocumentIdentifier(uri("pom.xml"))))
        awaitLoads(3)
        server.textDocumentService.didSave(DidSaveTextDocumentParams(TextDocumentIdentifier(uri("application.yml"))))
        Thread.sleep(400)
        assertEquals(3, loads.get())
    }

    @Test
    fun `build files are watched when the client supports dynamic registration`() {
        val dynamic = ClientCapabilities().apply {
            workspace = WorkspaceClientCapabilities().apply {
                didChangeWatchedFiles = DidChangeWatchedFilesCapabilities(true)
            }
        }
        start(capabilities = dynamic)
        val registration = assertNotNull(client.registrations.poll(10, TimeUnit.SECONDS)).registrations.single()
        assertEquals("workspace/didChangeWatchedFiles", registration.method)
        val globs = (registration.registerOptions as JsonObject).getAsJsonArray("watchers")
            .map { it.asJsonObject.get("globPattern").asString }
        assertTrue("**/pom.xml" in globs && "**/build.gradle.kts" in globs && "**/gradle/libs.versions.toml" in globs, globs.toString())

        start()
        assertNull(client.registrations.poll(500, TimeUnit.MILLISECONDS))
    }

    @Test
    fun `resolution problems are logged as warnings`() {
        val server = start(load = { _, resolution -> resolution.warn("Maven exited with code 1"); error("boom") })
        open(server, "application.properties", "a=1\n")
        val messages = listOf(client.logs.poll(10, TimeUnit.SECONDS), client.logs.poll(10, TimeUnit.SECONDS))
        assertTrue(messages.all { it?.type == MessageType.Warning }, messages.toString())
        assertTrue(messages.any { it?.message == "Maven exited with code 1" }, messages.toString())
        assertTrue(messages.any { it?.message?.contains("boom") == true }, messages.toString())
    }
}
