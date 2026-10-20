package dev.brewery.daemon.mcp

import com.google.gson.JsonObject
import dev.brewery.daemon.BuildModule
import dev.brewery.daemon.BuildTask
import dev.brewery.daemon.BuildTool
import dev.brewery.daemon.ProjectModel
import dev.brewery.daemon.run.RunConfiguration
import org.junit.jupiter.api.Test
import java.io.File
import java.nio.file.Path
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class ProjectToolsTest {
    private val petshop: Path = File("src/test/resources/projects/spring-petshop").absoluteFile.toPath().normalize()
    private var reads = 0
    private val model = ProjectModel(BuildTool.gradle, petshop.toString(), listOf(BuildModule("spring-petshop", ":", listOf(BuildTask("bootRun", "application", null)))))
    private val tools = ProjectTools(petshop, readModel = { reads++; model }, windows = false).tools().associateBy { it.name }

    private fun args(vararg pairs: Pair<String, Boolean>) = JsonObject().apply { pairs.forEach { (k, v) -> addProperty(k, v) } }

    @Test
    fun `offers the four project tools`() {
        assertEquals(setOf("project_model", "spring_beans", "spring_endpoints", "run_configurations"), tools.keys)
    }

    @Test
    fun `project model is cached until refresh`() {
        assertEquals(model, tools.getValue("project_model").call(args()))
        tools.getValue("project_model").call(args())
        assertEquals(1, reads)
        tools.getValue("project_model").call(args("refresh" to true))
        assertEquals(2, reads)
    }

    @Test
    fun `spring tools scan the project sources`() {
        @Suppress("UNCHECKED_CAST")
        val beans = tools.getValue("spring_beans").call(args()) as List<Any>
        assertTrue(beans.isNotEmpty())
        @Suppress("UNCHECKED_CAST")
        val endpoints = tools.getValue("spring_endpoints").call(args()) as List<Any>
        assertTrue(endpoints.isNotEmpty())
    }

    @Test
    fun `run configurations use the bootRun task`() {
        @Suppress("UNCHECKED_CAST")
        val configs = tools.getValue("run_configurations").call(args()) as List<RunConfiguration>
        assertTrue(configs.isNotEmpty())
        assertTrue(configs.all { "bootRun" in it.args.joinToString(" ") })
    }
}
