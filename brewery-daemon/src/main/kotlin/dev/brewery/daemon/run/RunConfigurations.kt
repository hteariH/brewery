package dev.brewery.daemon.run

import dev.brewery.daemon.BuildModule
import dev.brewery.daemon.BuildTool
import dev.brewery.daemon.ProjectModel
import dev.brewery.daemon.spring.NameEnumAdapter
import dev.brewery.daemon.spring.SpringApp
import dev.brewery.daemon.spring.SpringModel
import com.google.gson.annotations.JsonAdapter
import org.apache.maven.model.io.xpp3.MavenXpp3Reader
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.Paths

enum class RunKind { springBoot, application }

class RunKindAdapter : NameEnumAdapter<RunKind>(RunKind::class.java)

class RunConfigurationsParams

data class RunConfiguration(
    val label: String,
    val command: String,
    val args: List<String>,
    val cwd: String,
    @field:JsonAdapter(RunKindAdapter::class) val kind: RunKind,
    /** Fully-qualified class of the application, so clients pair configurations with apps unambiguously. */
    val mainClass: String,
)

/**
 * Builds the commands that start each Spring application. Wrapper detection mirrors the Rust side's
 * `brewery_build::task_command`: the wrapper is addressed by absolute path because the terminal's shell does
 * not look in the current directory, and [windows] is injected so the logic never reads `os.name` itself.
 *
 * Maven submodules run as `-pl <module> spring-boot:run` without `-am`: `-am` would also run the goal on every
 * upstream library module, which fails there. The price is that sibling modules the application depends on must
 * already be installed (`mvn install`) before the run configuration works.
 *
 * The Gradle `application` fallback passes `-PmainClass=<fqcn>` and assumes the build script reads that
 * property (for example `application.mainClass = findProperty("mainClass")`); Gradle's `application` plugin
 * itself only reads `application.mainClass`.
 */
object RunConfigurations {
    fun build(project: ProjectModel, spring: SpringModel, windows: Boolean): List<RunConfiguration> {
        val root = Paths.get(project.rootDir)
        return spring.applications.mapNotNull { app -> configurationFor(project, root, app, windows) }
    }

    private fun configurationFor(project: ProjectModel, root: Path, app: SpringApp, windows: Boolean): RunConfiguration? {
        val module = project.modules.firstOrNull { it.path == app.modulePath }
        return when (project.buildTool) {
            BuildTool.maven -> {
                if (!declaresBootPlugin(root, app.modulePath)) return null
                val args = if (isMavenRoot(app.modulePath)) {
                    listOf("spring-boot:run")
                } else {
                    listOf("-pl", app.modulePath, "spring-boot:run")
                }
                RunConfiguration(app.name, wrapperOr(root, if (windows) "mvnw.cmd" else "mvnw", "mvn"), args, project.rootDir, RunKind.springBoot, app.mainClass)
            }
            BuildTool.gradle -> {
                val command = wrapperOr(root, if (windows) "gradlew.bat" else "gradlew", "gradle")
                val prefix = gradlePrefix(app.modulePath)
                when {
                    module.hasTask("bootRun") ->
                        RunConfiguration(app.name, command, listOf("${prefix}bootRun"), project.rootDir, RunKind.springBoot, app.mainClass)
                    // Without the Boot plugin only the `application` plugin's run task can start it; the main
                    // class is passed as a property because the build script rarely sets it for a Boot app.
                    module.hasTask("run") ->
                        RunConfiguration(
                            app.name, command, listOf("${prefix}run", "-PmainClass=${app.mainClass}"), project.rootDir, RunKind.application, app.mainClass,
                        )
                    else -> null
                }
            }
            BuildTool.none -> null
        }
    }

    /**
     * True when the module's pom, an ancestor pom in the reactor, or the Spring Boot starter parent declares
     * `spring-boot-maven-plugin`; without it `spring-boot:run` is not resolvable from the command line.
     */
    private fun declaresBootPlugin(root: Path, modulePath: String): Boolean {
        var dir: Path? = if (isMavenRoot(modulePath)) root else root.resolve(modulePath)
        var depth = 0
        while (dir != null && depth++ < MAX_PARENT_DEPTH) {
            val pom = dir.resolve("pom.xml")
            val model = try {
                Files.newBufferedReader(pom).use { MavenXpp3Reader().read(it) }
            } catch (e: Exception) {
                // Missing or malformed pom: nothing to prove the plugin is there.
                return false
            }
            val plugins = model.build?.let { it.plugins + (it.pluginManagement?.plugins ?: emptyList()) } ?: emptyList()
            if (plugins.any { it.artifactId == BOOT_PLUGIN && (it.groupId == null || it.groupId == BOOT_GROUP) }) return true
            val parent = model.parent ?: return false
            if (parent.groupId == BOOT_GROUP && parent.artifactId == BOOT_PARENT) return true
            // Maven's default relativePath; a parent outside the project directory is not followed.
            val relative = parent.relativePath.takeUnless { it.isNullOrEmpty() } ?: "../pom.xml"
            val next = dir.resolve(relative).normalize().let { if (Files.isDirectory(it)) it else it.parent }
            dir = next?.takeIf { it.startsWith(root) && it != dir }
        }
        return false
    }

    private const val BOOT_PLUGIN = "spring-boot-maven-plugin"
    private const val BOOT_GROUP = "org.springframework.boot"
    private const val BOOT_PARENT = "spring-boot-starter-parent"
    private const val MAX_PARENT_DEPTH = 16

    private fun BuildModule?.hasTask(name: String) = this?.tasks?.any { it.name == name } == true

    private fun isMavenRoot(modulePath: String) = modulePath.isEmpty() || modulePath == "."

    private fun gradlePrefix(modulePath: String): String =
        if (modulePath.isEmpty() || modulePath == ":") "" else "${modulePath.trimEnd(':')}:"

    private fun wrapperOr(root: Path, wrapper: String, fallback: String): String {
        val path = root.resolve(wrapper)
        return if (Files.isRegularFile(path)) path.toString() else fallback
    }
}
