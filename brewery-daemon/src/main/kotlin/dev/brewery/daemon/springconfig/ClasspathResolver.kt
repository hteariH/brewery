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

fun interface ClasspathResolver {
    /**
     * Classpath entries carrying configuration metadata for the project at [root]: the modules' own resource and
     * output directories first (see [projectMetadataRoots]), then the dependency jars; empty when unknown.
     */
    fun resolve(root: Path): List<File>
}

/** Picks the resolver matching the build files in [root]. */
fun resolveClasspath(root: Path): List<File> = when (detectBuildTool(root)) {
    BuildTool.gradle -> GradleClasspathResolver().resolve(root)
    BuildTool.maven -> MavenClasspathResolver().resolve(root)
    else -> projectMetadataRoots(listOf(root))
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
            return projectMetadataRoots(mavenModules(root)) + parseClasspath(Files.readString(output))
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

/** Splits a `dependency:build-classpath` output file; duplicates (appended modules) and blanks are dropped. */
fun parseClasspath(text: String): List<File> =
    text.split(File.pathSeparatorChar, '\n', '\r').map { it.trim() }.filter { it.isNotEmpty() }
        .distinct().map { File(it) }
