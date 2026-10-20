package dev.brewery.daemon.mcp

import com.google.gson.JsonObject
import dev.brewery.daemon.ProjectModel
import dev.brewery.daemon.project.describeImportFailure
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
    private val windows: Boolean = System.getProperty("os.name").orEmpty().startsWith("Windows", ignoreCase = true),
) {
    private val scanner = SpringScanner()
    private var model: ProjectModel? = null

    @Synchronized
    private fun model(refresh: Boolean): ProjectModel {
        if (refresh || model == null) {
            model = try {
                readModel(root)
            } catch (e: Throwable) {
                // Throwable: a Gradle/Maven linkage Error must become a tool error, not end the MCP session.
                throw McpToolError("Could not import the ${root.fileName} build: ${describeImportFailure(e)}")
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
