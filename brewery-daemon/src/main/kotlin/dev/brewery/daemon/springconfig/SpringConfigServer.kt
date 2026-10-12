package dev.brewery.daemon.springconfig

import org.eclipse.lsp4j.CompletionItem
import org.eclipse.lsp4j.CompletionItemKind
import org.eclipse.lsp4j.CompletionItemTag
import org.eclipse.lsp4j.CompletionList
import org.eclipse.lsp4j.CompletionOptions
import org.eclipse.lsp4j.CompletionParams
import org.eclipse.lsp4j.Diagnostic
import org.eclipse.lsp4j.DiagnosticSeverity
import org.eclipse.lsp4j.DiagnosticTag
import org.eclipse.lsp4j.DidChangeConfigurationParams
import org.eclipse.lsp4j.DidChangeTextDocumentParams
import org.eclipse.lsp4j.DidChangeWatchedFilesParams
import org.eclipse.lsp4j.DidCloseTextDocumentParams
import org.eclipse.lsp4j.DidOpenTextDocumentParams
import org.eclipse.lsp4j.DidSaveTextDocumentParams
import org.eclipse.lsp4j.Hover
import org.eclipse.lsp4j.HoverParams
import org.eclipse.lsp4j.InitializeParams
import org.eclipse.lsp4j.InitializeResult
import org.eclipse.lsp4j.InitializedParams
import org.eclipse.lsp4j.MarkupContent
import org.eclipse.lsp4j.MarkupKind
import org.eclipse.lsp4j.Position
import org.eclipse.lsp4j.PublishDiagnosticsParams
import org.eclipse.lsp4j.Range
import org.eclipse.lsp4j.ServerCapabilities
import org.eclipse.lsp4j.ServerInfo
import org.eclipse.lsp4j.TextDocumentSyncKind
import org.eclipse.lsp4j.TextEdit
import org.eclipse.lsp4j.jsonrpc.messages.Either
import org.eclipse.lsp4j.services.LanguageClient
import org.eclipse.lsp4j.services.LanguageClientAware
import org.eclipse.lsp4j.services.LanguageServer
import org.eclipse.lsp4j.services.TextDocumentService
import org.eclipse.lsp4j.services.WorkspaceService
import java.net.URI
import java.nio.file.Path
import java.nio.file.Paths
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit

const val SERVER_NAME = "brewery-spring-config"

private val CONFIG_FILE = Regex("""^(application|bootstrap)(-[^/\\]+)?\.(properties|yml|yaml)$""")

private enum class FileKind { PROPERTIES, YAML }

private fun fileKind(uri: String): FileKind? {
    val name = uri.substringAfterLast('/')
    val match = CONFIG_FILE.matchEntire(name) ?: return null
    return if (match.groupValues[3] == "properties") FileKind.PROPERTIES else FileKind.YAML
}

/**
 * LSP server for Spring Boot `application*.properties|yml|yaml` files. Metadata is read from the project's
 * dependency jars in the background after `initialized`; until it is there (or when the project has none)
 * the server answers with nothing, so it never produces misleading "unknown property" warnings.
 */
class SpringConfigServer(
    private val loadMetadata: (Path) -> SpringMetadata = { root -> SpringMetadata.fromClasspath(resolveClasspath(root)) },
    private val debounceMs: Long = 300,
    private val onExit: () -> Unit = {},
) : LanguageServer, LanguageClientAware {
    private var client: LanguageClient? = null
    private var root: Path? = null

    @Volatile
    private var metadata: SpringMetadata? = null
    private val documents = ConcurrentHashMap<String, String>()
    private val pending = ConcurrentHashMap<String, ScheduledFuture<*>>()
    private val scheduler = Executors.newScheduledThreadPool(2) { r -> Thread(r, "spring-config-lsp").apply { isDaemon = true } }

    override fun connect(client: LanguageClient) {
        this.client = client
    }

    override fun initialize(params: InitializeParams): CompletableFuture<InitializeResult> {
        root = rootOf(params)
        val capabilities = ServerCapabilities().apply {
            setTextDocumentSync(TextDocumentSyncKind.Full)
            completionProvider = CompletionOptions(false, listOf(".", ":", "="))
            setHoverProvider(true)
        }
        return CompletableFuture.completedFuture(InitializeResult(capabilities, ServerInfo(SERVER_NAME, "0.1.0")))
    }

    @Suppress("DEPRECATION")
    private fun rootOf(params: InitializeParams): Path? {
        val uri = params.workspaceFolders?.firstOrNull()?.uri ?: params.rootUri
        return try {
            if (uri != null) Paths.get(URI(uri)) else params.rootPath?.let { Paths.get(it) }
        } catch (_: Exception) {
            null
        }
    }

    override fun initialized(params: InitializedParams?) {
        val projectRoot = root
        scheduler.execute {
            metadata = if (projectRoot == null) SpringMetadata.EMPTY else try {
                loadMetadata(projectRoot)
            } catch (_: Throwable) {
                SpringMetadata.EMPTY
            }
            documents.keys.forEach(::publish)
        }
    }

    override fun shutdown(): CompletableFuture<Any> {
        scheduler.shutdownNow()
        return CompletableFuture.completedFuture(null)
    }

    override fun exit() = onExit()

    override fun getTextDocumentService(): TextDocumentService = documentsService

    override fun getWorkspaceService(): WorkspaceService = object : WorkspaceService {
        override fun didChangeConfiguration(params: DidChangeConfigurationParams?) {}
        override fun didChangeWatchedFiles(params: DidChangeWatchedFilesParams?) {}
    }

    private fun schedulePublish(uri: String) {
        pending.remove(uri)?.cancel(false)
        if (scheduler.isShutdown) return
        pending[uri] = scheduler.schedule({ publish(uri) }, debounceMs, TimeUnit.MILLISECONDS)
    }

    private fun publish(uri: String) {
        val meta = metadata?.takeUnless { it.isEmpty } ?: return
        val text = documents[uri] ?: return
        val kind = fileKind(uri) ?: return
        client?.publishDiagnostics(PublishDiagnosticsParams(uri, diagnose(text, kind, meta)))
    }

    private val documentsService = object : TextDocumentService {
        override fun didOpen(params: DidOpenTextDocumentParams) {
            val uri = params.textDocument.uri
            documents[uri] = params.textDocument.text
            schedulePublish(uri)
        }

        override fun didChange(params: DidChangeTextDocumentParams) {
            val uri = params.textDocument.uri
            // Full sync: the last change event carries the whole document.
            val text = params.contentChanges.lastOrNull()?.text ?: return
            documents[uri] = text
            schedulePublish(uri)
        }

        override fun didClose(params: DidCloseTextDocumentParams) {
            val uri = params.textDocument.uri
            pending.remove(uri)?.cancel(false)
            val wasReported = metadata?.isEmpty == false && fileKind(uri) != null
            documents.remove(uri)
            if (wasReported) client?.publishDiagnostics(PublishDiagnosticsParams(uri, emptyList()))
        }

        override fun didSave(params: DidSaveTextDocumentParams?) {}

        override fun completion(params: CompletionParams): CompletableFuture<Either<List<CompletionItem>, CompletionList>> {
            val items = run {
                val meta = metadata?.takeUnless { it.isEmpty } ?: return@run emptyList()
                val uri = params.textDocument.uri
                val kind = fileKind(uri) ?: return@run emptyList()
                val text = documents[uri] ?: return@run emptyList()
                complete(text, kind, params.position, meta)
            }
            return CompletableFuture.completedFuture(Either.forLeft(items))
        }

        override fun hover(params: HoverParams): CompletableFuture<Hover?> {
            val meta = metadata?.takeUnless { it.isEmpty }
            val uri = params.textDocument.uri
            val kind = fileKind(uri)
            val text = documents[uri]
            if (meta == null || kind == null || text == null) return CompletableFuture.completedFuture(null)
            return CompletableFuture.completedFuture(hover(text, kind, params.position, meta))
        }
    }
}

private fun range(line: Int, start: Int, end: Int) = Range(Position(line, start), Position(line, end))

private fun complete(text: String, kind: FileKind, pos: Position, meta: SpringMetadata): List<CompletionItem> {
    val context = when (kind) {
        FileKind.PROPERTIES -> PropertiesDocument(text).contextAt(pos.line, pos.character)
        FileKind.YAML -> YamlDocument(text).contextAt(pos.line, pos.character)
    } ?: return emptyList()
    return when (context) {
        is ConfigContext.Value -> valueItems(context, pos.line, meta)
        is ConfigContext.Key -> when (kind) {
            FileKind.PROPERTIES -> meta.properties.map { propertyItem(it.name, it, it.name, range(pos.line, context.startCol, context.endCol)) }
            FileKind.YAML -> yamlKeyItems(context, pos.line, meta)
        }
    }
}

private fun valueItems(context: ConfigContext.Value, line: Int, meta: SpringMetadata): List<CompletionItem> {
    val edit = range(line, context.startCol, context.endCol)
    fun item(value: String, doc: String?) = CompletionItem(value).apply {
        kind = CompletionItemKind.Value
        textEdit = Either.forLeft(TextEdit(edit, if (context.needsSpace) " $value" else value))
        filterText = value
        if (doc != null) documentation = Either.forLeft(doc)
    }
    val hint = meta.hint(context.key)
    if (hint != null && hint.values.isNotEmpty()) return hint.values.map { item(it.value, it.description) }
    val type = meta.find(context.key)?.type
    return if (type == "java.lang.Boolean" || type == "boolean") listOf(item("true", null), item("false", null)) else emptyList()
}

private fun propertyItem(label: String, property: ConfigProperty?, insert: String, edit: Range) =
    CompletionItem(label).apply {
        kind = if (property != null) CompletionItemKind.Property else CompletionItemKind.Module
        detail = property?.simpleType
        textEdit = Either.forLeft(TextEdit(edit, insert))
        filterText = label
        sortText = label
        if (property != null) {
            documentation = Either.forRight(MarkupContent(MarkupKind.MARKDOWN, describe(property, withName = false)))
            if (property.deprecation != null) {
                @Suppress("DEPRECATION")
                deprecated = true
                tags = listOf(CompletionItemTag.Deprecated)
            }
        }
    }

/** A single key segment for relaxed comparison (the same rules as [canonicalKey], minus the dot handling). */
private fun segment(s: String) = s.replace(Regex("""\[[^\]]*]"""), "").replace("-", "").replace("_", "").lowercase()

private fun yamlKeyItems(context: ConfigContext.Key, line: Int, meta: SpringMetadata): List<CompletionItem> {
    val parent = context.parentPath.map(::segment)
    val n = parent.size
    // Next segment name -> the property that ends exactly there (null while it is only a group of deeper keys).
    val next = LinkedHashMap<String, Pair<String, ConfigProperty?>>()
    for (p in meta.properties) {
        val parts = p.name.split('.')
        if (parts.size <= n || parts.take(n).map(::segment) != parent) continue
        val name = parts[n]
        val key = segment(name)
        val leaf = if (parts.size == n + 1) p else next[key]?.second
        next[key] = name to leaf
    }
    val edit = range(line, context.startCol, context.endCol)
    return next.values.map { (name, leaf) ->
        // A scalar is written on the same line; a map or a group continues in a nested block below.
        val insert = when {
            context.hasColon -> name
            leaf != null && !leaf.isMap -> "$name: "
            else -> "$name:"
        }
        propertyItem(name, leaf, insert, edit)
    }
}

private fun describe(p: ConfigProperty, withName: Boolean): String = buildString {
    if (withName) append("**").append(p.name).append("**\n\n")
    p.type?.let { append('`').append(it).append("`\n\n") }
    p.description?.let { append(it).append("\n\n") }
    p.defaultValue?.let { append("Default: `").append(it).append("`\n\n") }
    p.deprecation?.let { append(deprecationText(p.name, it)).append("\n\n") }
}.trimEnd()

private fun deprecationText(name: String, d: Deprecation): String = buildString {
    append("Deprecated Spring Boot property '").append(name).append('\'')
    d.reason?.let { append(": ").append(it) }
    d.replacement?.let { append(". Use '").append(it).append("' instead") }
}

private fun hover(text: String, kind: FileKind, pos: Position, meta: SpringMetadata): Hover? {
    val (key, start, end) = when (kind) {
        FileKind.PROPERTIES -> PropertiesDocument(text).keyAt(pos.line, pos.character)?.let { Triple(it.key, it.keyStart, it.keyEnd) }
        FileKind.YAML -> YamlDocument(text).keyAt(pos.line, pos.character)?.let { Triple(it.key, it.keyStart, it.keyEnd) }
    } ?: return null
    val property = meta.find(key) ?: return null
    return Hover(MarkupContent(MarkupKind.MARKDOWN, describe(property, withName = true)), range(pos.line, start, end))
}

// Prefixes whose children are free-form even though the metadata does not mark them as maps everywhere.
private val OPEN_PREFIXES = listOf("logging.level.", "spring.profiles.")

private fun diagnose(text: String, kind: FileKind, meta: SpringMetadata): List<Diagnostic> {
    val keys: List<Triple<String, Range, Boolean>> = when (kind) {
        FileKind.PROPERTIES -> PropertiesDocument(text).entries.map { Triple(it.key, range(it.line, it.keyStart, it.keyEnd), true) }
        // A YAML key that only groups other keys is not a property itself.
        FileKind.YAML -> YamlDocument(text).entries.map {
            Triple(it.key, range(it.line, it.keyStart, it.keyEnd), it.value.isNotEmpty() || !it.hasChildren)
        }
    }
    val result = ArrayList<Diagnostic>()
    for ((key, where, checked) in keys) {
        if (!checked) continue
        val property = meta.find(key)
        if (property != null) {
            property.deprecation?.let {
                result += Diagnostic(where, deprecationText(property.name, it), DiagnosticSeverity.Warning, SERVER_NAME).apply {
                    tags = listOf(DiagnosticTag.Deprecated)
                }
            }
            continue
        }
        val canonical = canonicalKey(key)
        if (!meta.isUnknown(key) || OPEN_PREFIXES.any { canonical.startsWith(it) }) continue
        result += Diagnostic(where, "Unknown Spring Boot property '$key'", DiagnosticSeverity.Warning, SERVER_NAME)
    }
    return result
}
