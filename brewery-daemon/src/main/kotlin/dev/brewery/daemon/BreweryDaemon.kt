package dev.brewery.daemon

import dev.brewery.daemon.db.ConnectionInfo
import dev.brewery.daemon.db.DbCancelParams
import dev.brewery.daemon.db.DbCloseParams
import dev.brewery.daemon.db.DbCloseSessionParams
import dev.brewery.daemon.db.DbConnectionsDeleteParams
import dev.brewery.daemon.db.DbConnectionsListParams
import dev.brewery.daemon.db.DbConnectionsSaveParams
import dev.brewery.daemon.db.DbConnectionsTestParams
import dev.brewery.daemon.db.DbDriversAddJarParams
import dev.brewery.daemon.db.DbDriversParams
import dev.brewery.daemon.db.DbExecuteParams
import dev.brewery.daemon.db.DbExecuteResult
import dev.brewery.daemon.db.DbFetchParams
import dev.brewery.daemon.db.DbFetchResult
import dev.brewery.daemon.db.DbIntrospectParams
import dev.brewery.daemon.db.DbNode
import dev.brewery.daemon.db.DbException
import dev.brewery.daemon.db.DbSchema
import dev.brewery.daemon.db.DbSchemaParams
import dev.brewery.daemon.db.DbService
import dev.brewery.daemon.db.DbTestResult
import dev.brewery.daemon.db.DriverInfo
import dev.brewery.daemon.db.KeyringSecretStore
import dev.brewery.daemon.db.SecretStore
import dev.brewery.daemon.project.GradleModelReader
import dev.brewery.daemon.project.MavenModelReader
import dev.brewery.daemon.project.detectBuildTool
import dev.brewery.daemon.project.isBuildFileName
import dev.brewery.daemon.run.RunConfiguration
import dev.brewery.daemon.run.RunConfigurations
import dev.brewery.daemon.run.RunConfigurationsParams
import dev.brewery.daemon.spring.SpringModel
import dev.brewery.daemon.spring.SpringScanner
import org.eclipse.lsp4j.jsonrpc.ResponseErrorException
import org.eclipse.lsp4j.jsonrpc.messages.ResponseError
import org.eclipse.lsp4j.jsonrpc.messages.ResponseErrorCode
import java.io.IOException
import java.nio.file.ClosedWatchServiceException
import java.nio.file.FileVisitResult
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.Paths
import java.nio.file.SimpleFileVisitor
import java.nio.file.StandardWatchEventKinds
import java.nio.file.WatchService
import java.nio.file.attribute.BasicFileAttributes
import java.util.concurrent.CompletableFuture
import java.util.concurrent.Executors
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.ThreadFactory
import java.util.concurrent.TimeUnit
import kotlin.system.exitProcess

/**
 * [debounceMillis] coalesces bursts of build-file events (an IDE save touches several files) into one
 * notification. [modelReader] exists so tests can substitute a controllable import.
 */
class BreweryDaemon(
    private val debounceMillis: Long = DEFAULT_DEBOUNCE_MILLIS,
    // Injected so tests can pick the wrapper spelling without depending on the host OS.
    private val windows: Boolean = System.getProperty("os.name").orEmpty().startsWith("Windows", ignoreCase = true),
    // Opened lazily on first use, so a machine without a keyring costs nothing until a password is saved.
    private val secretStore: SecretStore = KeyringSecretStore(),
    // The database service of a project root; tests keep a handle on it.
    private val dbServiceFactory: (Path) -> DbService = { DbService(it, secretStore) },
    // Runs the debounced notifications; tests substitute a manual clock.
    private val notifier: ScheduledExecutorService = Executors.newSingleThreadScheduledExecutor(daemonThreads("brewery-notify")),
    // Last so call sites can pass the reader as a trailing lambda.
    private val modelReader: ((Path) -> ProjectModel)? = null,
) : BreweryServer, AutoCloseable {
    private var client: BreweryClient? = null
    private val lock = Any()
    private var root: Path? = null
    private var cached: CompletableFuture<ProjectModel>? = null
    private var watcher: BuildFileWatcher? = null
    private var pendingNotification: ScheduledFuture<*>? = null
    private var cachedSpring: CompletableFuture<SpringModel>? = null
    private var pendingSpringNotification: ScheduledFuture<*>? = null
    private val springScanner = SpringScanner()
    private var db: DbService? = null

    // One import at a time: an invalidation during a running import just queues the next one behind it
    // instead of racing a parallel Gradle/Maven run against the same project.
    private val importer = Executors.newSingleThreadExecutor(daemonThreads("brewery-import"))
    // Scanning parses every source file, so it gets its own thread rather than delaying the next import.
    private val scanner = Executors.newSingleThreadExecutor(daemonThreads("brewery-spring-scan"))
    // Database calls block on keyrings, drivers and the network, so they must not run on the JSON-RPC reader thread.
    private val dbExecutor = Executors.newCachedThreadPool(daemonThreads("brewery-db"))

    fun connect(client: BreweryClient) {
        this.client = client
    }

    override fun initialize(params: InitializeParams): CompletableFuture<InitializeResult> =
        CompletableFuture.completedFuture(InitializeResult(PROTOCOL_VERSION, DAEMON_VERSION))

    override fun shutdown(): CompletableFuture<Any?> = CompletableFuture.completedFuture(null)

    override fun exit() {
        close()
        exitProcess(0)
    }

    override fun close() {
        synchronized(lock) {
            watcher?.close()
            watcher = null
            pendingNotification?.cancel(false)
            pendingNotification = null
            pendingSpringNotification?.cancel(false)
            pendingSpringNotification = null
            db?.close()
            db = null
        }
        dbExecutor.shutdownNow()
        notifier.shutdownNow()
        importer.shutdownNow()
        scanner.shutdownNow()
    }

    override fun openProject(params: OpenProjectParams): CompletableFuture<Any?> {
        val dir = try {
            Paths.get(params.rootDir).toAbsolutePath().normalize()
        } catch (e: Exception) {
            return failed(ErrorCodes.BUILD_IMPORT_FAILED, "Invalid project path: ${params.rootDir}")
        }
        if (!Files.isDirectory(dir)) {
            return failed(ErrorCodes.BUILD_IMPORT_FAILED, "Project directory does not exist: $dir")
        }
        // Built before any state changes, so a failure here cannot leave a half-switched project.
        val newWatcher = try {
            BuildFileWatcher(dir, { onBuildFileChanged(dir) }, { onJavaSourceChanged(dir) })
        } catch (e: Exception) {
            System.err.println("brewery: build-file watching disabled for $dir: $e")
            null
        }
        val oldDb = synchronized(lock) {
            watcher?.close()
            watcher = newWatcher
            root = dir
            val oldDb = db
            db = null
            cached = null
            cachedSpring = null
            // A notification armed for the previous project would tell the client to refresh the wrong one.
            pendingNotification?.cancel(false)
            pendingNotification = null
            pendingSpringNotification?.cancel(false)
            pendingSpringNotification = null
            oldDb
        }
        // Closing waits up to 5 s per busy session; outside the lock it does not hold up every other request.
        if (oldDb != null) {
            try {
                dbExecutor.execute { oldDb.close() }
            } catch (_: RejectedExecutionException) {
                oldDb.close()
            }
        }
        return CompletableFuture.completedFuture(null)
    }

    override fun projectModel(params: ProjectModelParams): CompletableFuture<ProjectModel> {
        synchronized(lock) {
            val dir = root ?: return failed(ErrorCodes.NOT_OPENED, "No project is open")
            val shared = liveImport(dir)
            // Each caller gets its own dependent future: lsp4j cancels the future it was handed when the
            // client cancels a request, and that must not cancel the import other callers share.
            return shared.thenApply { it }
        }
    }

    override fun springModel(params: SpringModelParams): CompletableFuture<SpringModel> {
        synchronized(lock) {
            val dir = root ?: return failed(ErrorCodes.NOT_OPENED, "No project is open")
            val shared = cachedSpring?.takeUnless { it.isCompletedExceptionally }
                ?: startScan(dir).also { cachedSpring = it }
            return shared.thenApply { it }
        }
    }

    override fun runConfigurations(params: RunConfigurationsParams): CompletableFuture<List<RunConfiguration>> {
        val project = projectModel(ProjectModelParams())
        val spring = springModel(SpringModelParams())
        return project.thenCombine(spring) { p, s -> RunConfigurations.build(p, s, windows) }
    }

    override fun dbDrivers(params: DbDriversParams): CompletableFuture<List<DriverInfo>> = dbCall { it.drivers() }

    override fun dbDriversAddJar(params: DbDriversAddJarParams): CompletableFuture<DriverInfo> =
        dbCall { it.addJar(params.driverId, params.name, params.className, params.jarPaths) }

    override fun dbConnectionsList(params: DbConnectionsListParams): CompletableFuture<List<ConnectionInfo>> =
        dbCall { it.list() }

    override fun dbConnectionsSave(params: DbConnectionsSaveParams): CompletableFuture<ConnectionInfo> =
        dbCall { it.save(params.connection, params.password) }

    override fun dbConnectionsDelete(params: DbConnectionsDeleteParams): CompletableFuture<Any?> =
        dbCall { it.delete(params.id); null }

    override fun dbConnectionsTest(params: DbConnectionsTestParams): CompletableFuture<DbTestResult> =
        dbCall { it.test(params.connection, params.password) }

    override fun dbIntrospect(params: DbIntrospectParams): CompletableFuture<List<DbNode>> =
        dbCall { it.introspect(params.connectionId, params.path ?: emptyList(), params.includeSystem == true) }

    override fun dbSchema(params: DbSchemaParams): CompletableFuture<DbSchema> =
        dbCall { it.schema(params.connectionId, params.refresh) }

    override fun dbExecute(params: DbExecuteParams): CompletableFuture<DbExecuteResult> =
        dbCall { it.execute(params.connectionId, params.sql, params.sessionId) }

    override fun dbFetch(params: DbFetchParams): CompletableFuture<DbFetchResult> =
        dbCall { it.fetch(params.cursorId, params.offset, params.limit) }

    override fun dbCancel(params: DbCancelParams): CompletableFuture<Any?> =
        dbCall { it.cancel(params.connectionId, params.cursorId, params.sessionId); null }

    override fun dbClose(params: DbCloseParams): CompletableFuture<Any?> =
        dbCall { it.closeCursor(params.cursorId); null }

    override fun dbSessionsClose(params: DbCloseSessionParams): CompletableFuture<Any?> =
        dbCall { it.closeSession(params.connectionId, params.sessionId); null }

    private fun <T> dbCall(block: (DbService) -> T): CompletableFuture<T> {
        val service = synchronized(lock) {
            val dir = root ?: return failed(ErrorCodes.NOT_OPENED, "No project is open")
            db ?: dbServiceFactory(dir).also { db = it }
        }
        val future = CompletableFuture<T>()
        try {
            dbExecutor.execute {
                try {
                    future.complete(block(service))
                } catch (e: DbException) {
                    future.completeExceptionally(e.toResponseError())
                } catch (e: Throwable) {
                    // The message of an unexpected failure may carry connection details; keep only its type.
                    future.completeExceptionally(
                        ResponseErrorException(ResponseError(ResponseErrorCode.InternalError.value, "Database call failed: ${e.javaClass.simpleName}", null)),
                    )
                }
            }
        } catch (e: RejectedExecutionException) {
            future.completeExceptionally(e)
        }
        return future
    }

    // Caller holds [lock]. A future that already failed may still be cached when its completion handler ran
    // inline before the assignment, so a failed one is never reused.
    private fun liveImport(dir: Path): CompletableFuture<ProjectModel> =
        cached?.takeUnless { it.isCompletedExceptionally } ?: startImport(dir).also { cached = it }

    // Caller holds [lock].
    private fun startScan(dir: Path): CompletableFuture<SpringModel> {
        val model = liveImport(dir)
        val future = CompletableFuture<SpringModel>()
        model.whenComplete { project, error ->
            if (error != null) {
                future.completeExceptionally(error)
                return@whenComplete
            }
            try {
                scanner.execute {
                    // Same staleness check as imports: the project may have been replaced while this waited.
                    if (synchronized(lock) { root != dir }) {
                        future.cancel(false)
                        return@execute
                    }
                    try {
                        future.complete(springScanner.scan(project))
                    } catch (e: Throwable) {
                        future.completeExceptionally(e)
                    }
                }
            } catch (e: RejectedExecutionException) {
                future.completeExceptionally(e)
            }
        }
        future.whenComplete { _, error ->
            if (error != null) synchronized(lock) { if (cachedSpring === future) cachedSpring = null }
        }
        return future
    }

    private fun startImport(dir: Path): CompletableFuture<ProjectModel> {
        val future = CompletableFuture<ProjectModel>()
        try {
            importer.execute {
                // Checked at run time: imports queue behind a running one, and by now the project may be replaced.
                if (synchronized(lock) { root != dir }) {
                    future.cancel(false)
                    return@execute
                }
                try {
                    future.complete(modelReader?.invoke(dir) ?: readModel(dir))
                } catch (e: Throwable) {
                    future.completeExceptionally(e)
                }
            }
        } catch (e: RejectedExecutionException) {
            future.completeExceptionally(e)
        }
        // A failed import must not stick: the next request retries.
        future.whenComplete { _, error ->
            if (error != null) synchronized(lock) { if (cached === future) cached = null }
        }
        return future
    }

    private fun readModel(dir: Path): ProjectModel {
        try {
            val reader = when (detectBuildTool(dir)) {
                BuildTool.gradle -> GradleModelReader()
                BuildTool.maven -> MavenModelReader()
                BuildTool.none -> return ProjectModel(BuildTool.none, dir.toString(), emptyList())
            }
            return reader.read(dir)
        } catch (e: ResponseErrorException) {
            throw e
        } catch (e: Throwable) {
            throw ResponseErrorException(ResponseError(ErrorCodes.BUILD_IMPORT_FAILED, describe(e), null))
        }
    }

    // Tooling API wraps the real script error (file, line) in its cause chain, so the top message alone is useless.
    private fun describe(error: Throwable): String {
        val parts = mutableListOf<String>()
        var current: Throwable? = error
        var depth = 0
        while (current != null && depth++ < 20) {
            val text = current.message?.trim().takeUnless { it.isNullOrEmpty() } ?: current.javaClass.simpleName
            if (text !in parts) parts += text
            current = current.cause
        }
        return parts.joinToString("\n").take(MAX_MESSAGE)
    }

    internal fun onBuildFileChanged(dir: Path) {
        synchronized(lock) {
            if (root != dir) return
            // Dropped immediately so a request arriving inside the debounce window already re-imports.
            cached = null
            // The module list decides which source roots exist.
            cachedSpring = null
            // Trailing edge: every event restarts the timer, so the client hears about a burst once, after
            // the last write, and its re-import sees the finished files rather than a half-saved set.
            pendingNotification?.cancel(false)
            try {
                lateinit var task: ScheduledFuture<*>
                task = notifier.schedule({
                    // A newer event may have replaced this task just as it started running.
                    val current = synchronized(lock) {
                        (pendingNotification === task).also { if (it) pendingNotification = null }
                    }
                    if (current) client?.projectChanged(ProjectChanged())
                }, debounceMillis, TimeUnit.MILLISECONDS)
                pendingNotification = task
            } catch (_: RejectedExecutionException) {
                // Closed while an event was in flight.
            }
        }
    }

    internal fun onJavaSourceChanged(dir: Path) {
        synchronized(lock) {
            if (root != dir) return
            cachedSpring = null
            pendingSpringNotification?.cancel(false)
            try {
                lateinit var task: ScheduledFuture<*>
                task = notifier.schedule({
                    val current = synchronized(lock) {
                        (pendingSpringNotification === task).also { if (it) pendingSpringNotification = null }
                    }
                    if (current) client?.springChanged(SpringChanged())
                }, debounceMillis, TimeUnit.MILLISECONDS)
                pendingSpringNotification = task
            } catch (_: RejectedExecutionException) {
                // Closed while an event was in flight.
            }
        }
    }

    private companion object {
        const val MAX_MESSAGE = 2000
        const val DEFAULT_DEBOUNCE_MILLIS = 500L

        fun daemonThreads(name: String) = ThreadFactory { Thread(it, name).apply { isDaemon = true } }
    }

    private fun <T> failed(code: Int, message: String): CompletableFuture<T> =
        CompletableFuture.failedFuture(ResponseErrorException(ResponseError(code, message, null)))
}

/**
 * Watches the project root and its module directories for build-file edits, and every `src/main/java`
 * tree for Java edits. Runs on its own daemon thread so it never keeps the JVM alive.
 */
private class BuildFileWatcher(
    private val root: Path,
    private val onBuildChange: () -> Unit,
    private val onJavaChange: () -> Unit,
) : AutoCloseable {
    private val service: WatchService = root.fileSystem.newWatchService()
    private val skipped = setOf("build", "target", "node_modules", ".git", ".gradle", ".idea")
    private val thread = Thread(::run, "brewery-build-watcher").apply { isDaemon = true }

    init {
        try {
            register(root)
        } catch (e: Exception) {
            service.close()
            throw e
        }
        thread.start()
    }

    private fun isJavaRoot(dir: Path): Boolean {
        val rel = root.relativize(dir)
        val n = rel.nameCount
        return n >= 3 && rel.getName(n - 1).toString() == "java" && rel.getName(n - 2).toString() == "main" &&
            rel.getName(n - 3).toString() == "src"
    }

    private fun underJavaRoot(dir: Path): Boolean {
        var current: Path? = dir
        while (current != null && current != root) {
            if (isJavaRoot(current)) return true
            current = current.parent
        }
        return false
    }

    private fun register(start: Path) {
        // Module layouts are shallow; a bounded walk avoids registering whole dependency trees. The walk may
        // go deeper only along src/main/java, where package directories nest much further.
        Files.walkFileTree(
            start,
            emptySet(),
            Int.MAX_VALUE,
            object : SimpleFileVisitor<Path>() {
                override fun preVisitDirectory(dir: Path, attrs: BasicFileAttributes): FileVisitResult {
                    if (dir != root && dir.fileName.toString() in skipped) return FileVisitResult.SKIP_SUBTREE
                    val depth = root.relativize(dir).nameCount
                    val inJava = underJavaRoot(dir)
                    val name = dir.fileName.toString()
                    val towardsJava = name == "src" || (name == "main" && dir.parent?.fileName?.toString() == "src")
                    if (depth > MAX_DEPTH && !inJava && !towardsJava) return FileVisitResult.SKIP_SUBTREE
                    runCatching {
                        dir.register(
                            service,
                            StandardWatchEventKinds.ENTRY_CREATE,
                            StandardWatchEventKinds.ENTRY_MODIFY,
                            StandardWatchEventKinds.ENTRY_DELETE,
                        )
                    }
                    return FileVisitResult.CONTINUE
                }

                override fun visitFileFailed(file: Path, exc: IOException) = FileVisitResult.CONTINUE
            },
        )
    }

    private fun run() {
        try {
            while (true) {
                val key = service.take()
                val dir = key.watchable() as? Path
                var build = false
                var java = false
                val inJava = dir != null && underJavaRoot(dir)
                for (event in key.pollEvents()) {
                    if (event.kind() == StandardWatchEventKinds.OVERFLOW) {
                        build = true
                        java = true
                        continue
                    }
                    val name = (event.context() as? Path)?.fileName?.toString() ?: continue
                    if (isBuildFileName(name)) build = true
                    // A name without an extension may be a package directory appearing or disappearing.
                    if (inJava && (name.endsWith(".java") || !name.contains('.'))) {
                        java = true
                        if (event.kind() == StandardWatchEventKinds.ENTRY_CREATE) {
                            val created = dir.resolve(name)
                            if (Files.isDirectory(created)) runCatching { register(created) }
                        }
                    }
                }
                key.reset()
                if (build) {
                    try {
                        onBuildChange()
                    } catch (e: Exception) {
                        System.err.println("brewery: project/changed failed: $e")
                    }
                }
                if (java) {
                    try {
                        onJavaChange()
                    } catch (e: Exception) {
                        System.err.println("brewery: spring/changed failed: $e")
                    }
                }
            }
        } catch (_: InterruptedException) {
        } catch (_: ClosedWatchServiceException) {
        }
    }

    override fun close() {
        service.close()
        thread.interrupt()
    }

    private companion object {
        const val MAX_DEPTH = 4
    }
}
