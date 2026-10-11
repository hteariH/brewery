package dev.brewery.daemon.springconfig

import dev.brewery.daemon.BuildTool
import dev.brewery.daemon.project.detectBuildTool
import org.gradle.tooling.GradleConnector
import org.gradle.tooling.model.idea.IdeaProject
import org.gradle.tooling.model.idea.IdeaSingleEntryLibraryDependency
import java.io.File
import java.io.OutputStream
import java.nio.file.Files
import java.nio.file.Path

fun interface ClasspathResolver {
    /** Dependency jars of the project at [root]; empty when they cannot be determined. */
    fun resolve(root: Path): List<File>
}

/** Picks the resolver matching the build files in [root]. */
fun resolveClasspath(root: Path): List<File> = when (detectBuildTool(root)) {
    BuildTool.gradle -> GradleClasspathResolver().resolve(root)
    BuildTool.maven -> MavenClasspathResolver().resolve(root)
    else -> emptyList()
}

class GradleClasspathResolver(private val fallbackGradleVersion: String = "9.8.1") : ClasspathResolver {
    override fun resolve(root: Path): List<File> {
        val connector = GradleConnector.newConnector().forProjectDirectory(root.toFile())
        if (!Files.exists(root.resolve("gradle/wrapper/gradle-wrapper.properties"))) {
            connector.useGradleVersion(fallbackGradleVersion)
        }
        val connection = connector.connect()
        try {
            // stdout carries the LSP stream, so Gradle's own output must never reach it.
            val project = connection.model(IdeaProject::class.java)
                .setStandardOutput(OutputStream.nullOutputStream())
                .setStandardError(OutputStream.nullOutputStream())
                .get()
            return project.modules.asSequence()
                .flatMap { it.dependencies.asSequence() }
                .filterIsInstance<IdeaSingleEntryLibraryDependency>()
                .mapNotNull { it.file }
                .distinct()
                .toList()
        } finally {
            connection.close()
        }
    }
}

/** Seam for tests: Maven is not required on the machine running them. */
fun interface CommandRunner {
    /** Runs [command] in [workingDir] and returns its exit code. */
    fun run(command: List<String>, workingDir: Path): Int

    companion object {
        val SYSTEM = CommandRunner { command, workingDir ->
            val process = ProcessBuilder(command).directory(workingDir.toFile())
                .redirectErrorStream(true)
                .redirectOutput(ProcessBuilder.Redirect.DISCARD)
                .start()
            process.waitFor()
        }
    }
}

class MavenClasspathResolver(private val runner: CommandRunner = CommandRunner.SYSTEM) : ClasspathResolver {
    override fun resolve(root: Path): List<File> {
        val output = Files.createTempFile("brewery-classpath", ".txt")
        try {
            // appendOutput so every module of a reactor build adds its classpath to the one file.
            val command = listOf(
                mavenExecutable(root), "-q", "dependency:build-classpath",
                "-Dmdep.outputFile=$output", "-Dmdep.appendOutput=true",
            )
            val exit = try {
                runner.run(command, root)
            } catch (_: Exception) {
                return emptyList()
            }
            if (exit != 0) return emptyList()
            return parseClasspath(Files.readString(output))
        } finally {
            Files.deleteIfExists(output)
        }
    }

    private fun mavenExecutable(root: Path): String {
        val windows = System.getProperty("os.name").startsWith("Windows")
        val wrapper = root.resolve(if (windows) "mvnw.cmd" else "mvnw")
        return if (Files.exists(wrapper)) wrapper.toString() else if (windows) "mvn.cmd" else "mvn"
    }
}

/** Splits a `dependency:build-classpath` output file; duplicates (appended modules) and blanks are dropped. */
fun parseClasspath(text: String): List<File> =
    text.split(File.pathSeparatorChar, '\n', '\r').map { it.trim() }.filter { it.isNotEmpty() }
        .distinct().map { File(it) }
