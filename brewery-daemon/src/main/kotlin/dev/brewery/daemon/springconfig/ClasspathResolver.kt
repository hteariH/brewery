package dev.brewery.daemon.springconfig

import dev.brewery.daemon.BuildTool
import dev.brewery.daemon.project.MavenModelReader
import dev.brewery.daemon.project.detectBuildTool
import org.gradle.tooling.GradleConnector
import org.gradle.tooling.model.idea.IdeaModuleDependency
import org.gradle.tooling.model.idea.IdeaProject
import org.gradle.tooling.model.idea.IdeaSingleEntryLibraryDependency
import java.io.File
import java.io.OutputStream
import java.nio.file.Files
import java.nio.file.Path
import java.time.Duration
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException

/**
 * One classpath resolution run: [cancel] (on shutdown) stops whatever it is waiting for, and [warn] tells the
 * user why the result may be incomplete.
 */
class Resolution(val warn: (String) -> Unit = {}) {
    private val cancelActions = CopyOnWriteArrayList<() -> Unit>()

    @Volatile
    var isCancelled = false
        private set

    /** Runs [action] on cancellation, or right away when the run is already cancelled. */
    fun onCancel(action: () -> Unit) {
        cancelActions += action
        if (isCancelled) runCatching(action)
    }

    fun cancel() {
        isCancelled = true
        cancelActions.forEach { runCatching(it) }
    }
}

interface ClasspathResolver {
    /**
     * Classpath entries carrying configuration metadata for the project at [root]: the modules' own resource and
     * output directories first (see [projectMetadataRoots]), then the dependency jars; empty when unknown.
     */
    fun resolve(root: Path, resolution: Resolution = Resolution()): List<File>
}

/** Picks the resolver matching the build files in [root]; a failed build still yields the project's own metadata. */
fun resolveClasspath(root: Path, resolution: Resolution = Resolution()): List<File> {
    val resolver = when (detectBuildTool(root)) {
        BuildTool.gradle -> GradleClasspathResolver()
        BuildTool.maven -> MavenClasspathResolver()
        else -> return projectMetadataRoots(listOf(root))
    }
    return try {
        resolver.resolve(root, resolution)
    } catch (e: Exception) {
        if (!resolution.isCancelled) resolution.warn("Spring configuration: could not resolve the classpath: ${rootCause(e)}")
        projectMetadataRoots(listOf(root))
    }
}

private fun rootCause(e: Throwable): String {
    var cause = e
    while (true) {
        val next = cause.cause
        if (next == null || next === cause) break
        cause = next
    }
    return cause.message ?: cause.toString()
}

// Where a module keeps its own metadata: the hand-written additional file in the sources, and both files as the
// annotation processor and resource processing leave them in Gradle's and Maven's build outputs.
private val MODULE_METADATA_DIRS = listOf("src/main/resources", "build/classes/java/main", "build/resources/main", "target/classes")

/** The existing directories of [modules] that may hold `META-INF/` metadata, in module order. */
fun projectMetadataRoots(modules: List<Path>): List<File> =
    modules.asSequence()
        .flatMap { module -> MODULE_METADATA_DIRS.asSequence().map { module.resolve(it) } }
        .filter { Files.isDirectory(it) }
        .map { it.toFile() }
        .distinct()
        .toList()

class GradleClasspathResolver(private val fallbackGradleVersion: String = "9.8.1") : ClasspathResolver {
    override fun resolve(root: Path, resolution: Resolution): List<File> {
        val connector = GradleConnector.newConnector().forProjectDirectory(root.toFile())
        if (!Files.exists(root.resolve("gradle/wrapper/gradle-wrapper.properties"))) {
            connector.useGradleVersion(fallbackGradleVersion)
        }
        val cancellation = GradleConnector.newCancellationTokenSource()
        resolution.onCancel { cancellation.cancel() }
        val connection = connector.connect()
        try {
            // stdout carries the LSP stream, so Gradle's own output must never reach it.
            val project = connection.model(IdeaProject::class.java)
                .withCancellationToken(cancellation.token())
                .setStandardOutput(OutputStream.nullOutputStream())
                .setStandardError(OutputStream.nullOutputStream())
                .get()
            val modules = project.modules.toList()
            val dirByName = modules.associate { it.name to it.gradleProject.projectDirectory.toPath() }
            // Every module of the build is in the model; module dependencies are followed explicitly as well so a
            // sibling's outputs are covered even when a model ever lists only part of the build.
            val moduleDirs = modules.flatMap { module ->
                listOf(module.gradleProject.projectDirectory.toPath()) + module.dependencies
                    .filterIsInstance<IdeaModuleDependency>()
                    .mapNotNull { dirByName[it.targetModuleName] }
            }.distinct()
            val jars = modules.asSequence()
                .flatMap { it.dependencies.asSequence() }
                .filterIsInstance<IdeaSingleEntryLibraryDependency>()
                .mapNotNull { it.file }
                .distinct()
                .toList()
            return projectMetadataRoots(moduleDirs) + jars
        } finally {
            connection.close()
        }
    }
}

data class CommandResult(val exitCode: Int, val output: String)

/** Seam for tests: Maven is not required on the machine running them. */
fun interface CommandRunner {
    /** Runs [command] in [workingDir]; throws when it cannot start or does not finish in time. */
    fun run(command: List<String>, workingDir: Path, resolution: Resolution): CommandResult
}

/**
 * Runs a child process with its output in a log file, killing it (and what it started, as `mvn.cmd` starts a
 * JVM) when it outlives [timeout] or the resolution is cancelled.
 */
class ProcessCommandRunner(
    private val timeout: Duration = Duration.ofMinutes(5),
    private val start: (command: List<String>, workingDir: Path, log: File) -> Process = { command, dir, log ->
        ProcessBuilder(command).directory(dir.toFile()).redirectErrorStream(true).redirectOutput(log).start()
    },
) : CommandRunner {
    override fun run(command: List<String>, workingDir: Path, resolution: Resolution): CommandResult {
        val log = Files.createTempFile("brewery-command", ".log")
        try {
            val process = start(command, workingDir, log.toFile())
            resolution.onCancel { destroyTree(process) }
            if (!process.waitFor(timeout.toMillis(), TimeUnit.MILLISECONDS)) {
                destroyTree(process)
                throw TimeoutException("${command.first()} did not finish within ${timeout.toMillis()} ms")
            }
            if (resolution.isCancelled) throw InterruptedException("cancelled")
            return CommandResult(process.exitValue(), tail(log))
        } finally {
            Files.deleteIfExists(log)
        }
    }

    private fun destroyTree(process: Process) {
        // A Process that is not an OS process (as in tests) has no handle to walk.
        runCatching { process.descendants().forEach { it.destroyForcibly() } }
        process.destroyForcibly()
    }

    private fun tail(log: Path): String = runCatching {
        Files.readAllLines(log).takeLast(20).joinToString("\n")
    }.getOrDefault("")
}

class MavenClasspathResolver(private val runner: CommandRunner = ProcessCommandRunner()) : ClasspathResolver {
    override fun resolve(root: Path, resolution: Resolution): List<File> {
        val output = Files.createTempFile("brewery-classpath", ".txt")
        try {
            // appendOutput so every module of a reactor build adds its classpath to the one file, and -fae so a
            // module that fails to resolve does not keep the others from writing theirs.
            val command = listOf(
                mavenExecutable(root), "-q", "-fae", "dependency:build-classpath",
                "-Dmdep.outputFile=$output", "-Dmdep.appendOutput=true",
            )
            try {
                val result = runner.run(command, root, resolution)
                if (result.exitCode != 0) {
                    resolution.warn(
                        ("Spring configuration: Maven failed with exit code ${result.exitCode} while resolving the " +
                            "classpath; using the part it resolved.\n${result.output}").trimEnd(),
                    )
                }
            } catch (e: Exception) {
                if (!resolution.isCancelled) resolution.warn("Spring configuration: Maven ${e.message ?: e.toString()}")
            }
            if (resolution.isCancelled) return emptyList()
            val jars = if (Files.exists(output)) parseClasspath(Files.readString(output)) else emptyList()
            return projectMetadataRoots(mavenModules(root)) + jars
        } finally {
            Files.deleteIfExists(output)
        }
    }

    private fun mavenModules(root: Path): List<Path> = try {
        MavenModelReader().read(root).modules.map { root.resolve(it.path).normalize() }
    } catch (_: Exception) {
        listOf(root)
    }

    private fun mavenExecutable(root: Path): String {
        val windows = System.getProperty("os.name").startsWith("Windows")
        val wrapper = root.resolve(if (windows) "mvnw.cmd" else "mvnw")
        return if (Files.exists(wrapper)) wrapper.toString() else if (windows) "mvn.cmd" else "mvn"
    }
}

// Appended module classpaths can run together with no separator (`...a.jarC:\...` or `...a.jar/...`); a jar name
// followed directly by the start of an absolute path marks such a seam.
private val JAR_SEAM = Regex("""(?<=\.jar)(?=[A-Za-z]:[\\/]|/)""")

/** Splits a `dependency:build-classpath` output file; duplicates (appended modules) and blanks are dropped. */
fun parseClasspath(text: String): List<File> =
    text.split(File.pathSeparatorChar, '\n', '\r')
        .flatMap { it.trim().split(JAR_SEAM) }
        .map { it.trim() }
        .filter { it.isNotEmpty() }
        .distinct()
        .map { File(it) }
