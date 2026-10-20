package dev.brewery.daemon.project

import dev.brewery.daemon.BuildTool
import dev.brewery.daemon.ProjectModel
import java.nio.file.Files
import java.nio.file.Path

fun interface ProjectModelReader {
    fun read(root: Path): ProjectModel
}

val GRADLE_BUILD_FILES = setOf("settings.gradle", "settings.gradle.kts", "build.gradle", "build.gradle.kts")
const val MAVEN_BUILD_FILE = "pom.xml"

/** Gradle wins over Maven so a repo carrying both is treated as the tool its wrapper drives. */
fun detectBuildTool(root: Path): BuildTool = when {
    GRADLE_BUILD_FILES.any { Files.exists(root.resolve(it)) } -> BuildTool.gradle
    Files.exists(root.resolve(MAVEN_BUILD_FILE)) -> BuildTool.maven
    else -> BuildTool.none
}

// Inputs that change the imported model without touching a build script: properties, the version catalog
// and the wrapper pin.
private val GRADLE_INPUT_FILES = setOf("gradle.properties", "libs.versions.toml", "gradle-wrapper.properties")

fun isBuildFileName(name: String): Boolean =
    name in GRADLE_BUILD_FILES || name in GRADLE_INPUT_FILES || name == MAVEN_BUILD_FILE

/** The model of the project in [dir], read by the reader its build files call for. Throws whatever the reader throws. */
fun readProjectModel(dir: Path): ProjectModel {
    val reader = when (detectBuildTool(dir)) {
        BuildTool.gradle -> GradleModelReader()
        BuildTool.maven -> MavenModelReader()
        BuildTool.none -> return ProjectModel(BuildTool.none, dir.toString(), emptyList())
    }
    return reader.read(dir)
}

/**
 * A build import failure as text: Tooling API (and Maven) wrap the real script error (file, line) in the cause
 * chain, so the top message alone is useless. Each distinct message once, outermost first, at most [max] chars.
 */
fun describeImportFailure(error: Throwable, max: Int = 2000): String {
    val parts = mutableListOf<String>()
    var current: Throwable? = error
    var depth = 0
    while (current != null && depth++ < 20) {
        val text = current.message?.trim().takeUnless { it.isNullOrEmpty() } ?: current.javaClass.simpleName
        if (text !in parts) parts += text
        current = current.cause
    }
    return parts.joinToString("\n").take(max)
}
