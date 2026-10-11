package dev.brewery.daemon

import dev.brewery.daemon.project.GradleModelReader
import dev.brewery.daemon.project.MavenModelReader
import dev.brewery.daemon.project.detectBuildTool
import dev.brewery.daemon.project.isBuildFileName
import org.eclipse.lsp4j.jsonrpc.ResponseErrorException
import org.eclipse.lsp4j.jsonrpc.messages.ResponseError
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
import kotlin.system.exitProcess

class BreweryDaemon : BreweryServer, AutoCloseable {
    private var client: BreweryClient? = null
    private val lock = Any()
    private var root: Path? = null
    private var cached: CompletableFuture<ProjectModel>? = null
    private var watcher: BuildFileWatcher? = null

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
        }
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
            BuildFileWatcher(dir) { onBuildFileChanged(dir) }
        } catch (e: Exception) {
            System.err.println("brewery: build-file watching disabled for $dir: $e")
            null
        }
        synchronized(lock) {
            watcher?.close()
            watcher = newWatcher
            root = dir
            cached = null
        }
        return CompletableFuture.completedFuture(null)
    }

    override fun projectModel(params: ProjectModelParams): CompletableFuture<ProjectModel> {
        synchronized(lock) {
            val dir = root ?: return failed(ErrorCodes.NOT_OPENED, "No project is open")
            cached?.let { return it }
            val future = CompletableFuture.supplyAsync { readModel(dir) }
            cached = future
            // A failed import must not stick: the next request retries.
            future.whenComplete { _, error ->
                if (error != null) synchronized(lock) { if (cached === future) cached = null }
            }
            return future
        }
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

    private fun onBuildFileChanged(dir: Path) {
        synchronized(lock) {
            if (root != dir) return
            cached = null
        }
        client?.projectChanged(ProjectChanged())
    }

    private companion object {
        const val MAX_MESSAGE = 2000
    }

    private fun <T> failed(code: Int, message: String): CompletableFuture<T> =
        CompletableFuture.failedFuture(ResponseErrorException(ResponseError(code, message, null)))
}

/**
 * Watches the project root and its module directories for build-file edits.
 * Runs on its own daemon thread so it never keeps the JVM alive.
 */
private class BuildFileWatcher(root: Path, private val onChange: () -> Unit) : AutoCloseable {
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

    private fun register(root: Path) {
        // Module layouts are shallow; a bounded walk avoids registering whole dependency trees.
        Files.walkFileTree(
            root,
            emptySet(),
            MAX_DEPTH,
            object : SimpleFileVisitor<Path>() {
                override fun preVisitDirectory(dir: Path, attrs: BasicFileAttributes): FileVisitResult {
                    if (dir != root && dir.fileName.toString() in skipped) return FileVisitResult.SKIP_SUBTREE
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
                val relevant = key.pollEvents().any {
                    it.kind() == StandardWatchEventKinds.OVERFLOW ||
                        (it.context() as? Path)?.fileName?.toString()?.let(::isBuildFileName) == true
                }
                key.reset()
                if (relevant) {
                    try {
                        onChange()
                    } catch (e: Exception) {
                        System.err.println("brewery: project/changed failed: $e")
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
