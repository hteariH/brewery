package dev.brewery.daemon

import dev.brewery.daemon.run.RunConfiguration
import dev.brewery.daemon.run.RunConfigurations
import dev.brewery.daemon.run.RunConfigurationsParams
import dev.brewery.daemon.run.RunKind
import dev.brewery.daemon.spring.SpringApp
import dev.brewery.daemon.spring.SpringModel
import org.eclipse.lsp4j.jsonrpc.Launcher
import org.eclipse.lsp4j.jsonrpc.ResponseErrorException
import org.eclipse.lsp4j.jsonrpc.json.MessageJsonHandler
import org.junit.jupiter.api.Test
import java.io.File
import java.io.PipedInputStream
import java.io.PipedOutputStream
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.ExecutionException
import java.util.concurrent.TimeUnit
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

class RunConfigurationsTest {
    private val gson = MessageJsonHandler(emptyMap()).gson
    private val petshop: Path = File("src/test/resources/projects/spring-petshop").absoluteFile.toPath().normalize()

    private fun tasks(vararg names: String) = names.map { BuildTask(it, null, null) }

    private fun app(name: String, modulePath: String, main: String = "com.example.$name") =
        SpringApp(name, main, modulePath, "/x/$name.java", 1)

    private fun spring(vararg apps: SpringApp) = SpringModel(apps.toList(), emptyList(), emptyList())

    private fun rootWith(vararg files: String): Path {
        val root = Files.createTempDirectory("brewery-run")
        files.forEach { Files.writeString(root.resolve(it), "") }
        return root
    }

    @Test
    fun `gradle root module with bootRun uses the wrapper on non-windows`() {
        val root = rootWith("gradlew", "gradlew.bat")
        val project = ProjectModel(BuildTool.gradle, root.toString(), listOf(BuildModule("r", ":", tasks("bootRun", "build"))))
        val result = RunConfigurations.build(project, spring(app("PetshopApplication", ":")), windows = false)
        assertEquals(
            listOf(RunConfiguration("PetshopApplication", root.resolve("gradlew").toString(), listOf("bootRun"), root.toString(), RunKind.springBoot)),
            result,
        )
    }

    @Test
    fun `windows picks the bat wrapper and falls back to gradle without one`() {
        val root = rootWith("gradlew.bat")
        val project = ProjectModel(BuildTool.gradle, root.toString(), listOf(BuildModule("r", ":", tasks("bootRun"))))
        val win = RunConfigurations.build(project, spring(app("A", ":")), windows = true).single()
        assertEquals(root.resolve("gradlew.bat").toString(), win.command)
        // Only gradlew.bat exists, so the non-Windows spelling is absent and the system gradle is used.
        assertEquals("gradle", RunConfigurations.build(project, spring(app("A", ":")), windows = false).single().command)
    }

    @Test
    fun `gradle submodule gets a qualified task`() {
        val root = rootWith("gradlew")
        val project = ProjectModel(
            BuildTool.gradle, root.toString(),
            listOf(BuildModule("r", ":", emptyList()), BuildModule("app", ":app", tasks("bootRun")), BuildModule("n", ":a:b", tasks("bootRun"))),
        )
        val result = RunConfigurations.build(project, spring(app("App", ":app"), app("Nested", ":a:b")), windows = false)
        assertEquals(listOf(listOf(":app:bootRun"), listOf(":a:b:bootRun")), result.map { it.args })
    }

    @Test
    fun `gradle without bootRun falls back to run as an application, else is skipped`() {
        val root = rootWith("gradlew")
        val project = ProjectModel(
            BuildTool.gradle, root.toString(),
            listOf(BuildModule("r", ":", tasks("run")), BuildModule("lib", ":lib", tasks("build")), BuildModule("tool", ":tool", tasks("run"))),
        )
        val apps = spring(app("Root", ":", "com.example.Root"), app("Lib", ":lib"), app("Tool", ":tool", "com.example.Tool"))
        val result = RunConfigurations.build(project, apps, windows = false)
        assertEquals(
            listOf(
                Triple("Root", listOf("run", "-PmainClass=com.example.Root"), RunKind.application),
                Triple("Tool", listOf(":tool:run", "-PmainClass=com.example.Tool"), RunKind.application),
            ),
            result.map { Triple(it.label, it.args, it.kind) },
        )
    }

    @Test
    fun `maven uses spring-boot run with the module selected by -pl`() {
        val root = rootWith("mvnw", "mvnw.cmd")
        val project = ProjectModel(BuildTool.maven, root.toString(), listOf(BuildModule("r", ".", emptyList()), BuildModule("core", "core", emptyList())))
        val result = RunConfigurations.build(project, spring(app("Top", "."), app("Core", "core")), windows = true)
        assertEquals(listOf(listOf("spring-boot:run"), listOf("-pl", "core", "-am", "spring-boot:run")), result.map { it.args })
        assertTrue(result.all { it.kind == RunKind.springBoot && it.command == root.resolve("mvnw.cmd").toString() && it.cwd == root.toString() })
        val bare = ProjectModel(BuildTool.maven, petshop.toString(), listOf(BuildModule("r", ".", emptyList())))
        assertEquals("mvn", RunConfigurations.build(bare, spring(app("Top", ".")), windows = false).single().command)
    }

    @Test
    fun `projects without a build tool have no run configurations`() {
        val project = ProjectModel(BuildTool.none, petshop.toString(), emptyList())
        assertEquals(emptyList(), RunConfigurations.build(project, spring(app("A", ":")), windows = false))
    }

    private fun wire(daemon: BreweryDaemon): BreweryServer {
        val clientToServer = PipedOutputStream()
        val serverIn = PipedInputStream(clientToServer)
        val serverToClient = PipedOutputStream()
        val clientIn = PipedInputStream(serverToClient)
        val serverLauncher = Launcher.createLauncher(daemon, BreweryClient::class.java, serverIn, serverToClient)
        daemon.connect(serverLauncher.remoteProxy)
        serverLauncher.startListening()
        val quiet = object : BreweryClient {
            override fun projectChanged(params: ProjectChanged) {}
            override fun springChanged(params: SpringChanged) {}
        }
        val clientLauncher = Launcher.createLauncher(quiet, BreweryServer::class.java, clientIn, clientToServer)
        clientLauncher.startListening()
        return clientLauncher.remoteProxy
    }

    @Test
    fun `run configurations over the wire round-trip the fixture shape`() {
        val daemon = BreweryDaemon(
            modelReader = { ProjectModel(BuildTool.gradle, it.toString(), listOf(BuildModule("r", ":", tasks("bootRun")))) },
            windows = false,
        )
        val root = Files.createTempDirectory("brewery-run-wire")
        petshop.toFile().copyRecursively(root.toFile())
        Files.writeString(root.resolve("gradlew"), "")
        val server = wire(daemon)
        server.openProject(OpenProjectParams(root.toString())).get(10, TimeUnit.SECONDS)
        val received = server.runConfigurations(RunConfigurationsParams()).get(20, TimeUnit.SECONDS)

        // The fixture's second entry documents the application fallback; only the first applies to this project.
        val placeholder = "/path/to/spring-petshop"
        val text = File("protocol/fixtures/run-configurations.json").readText()
            .replace("$placeholder/gradlew", root.resolve("gradlew").toString().replace('\\', '/'))
            .replace(placeholder, root.toString().replace('\\', '/'))
        val expected = gson.fromJson(text, Array<RunConfiguration>::class.java).toList().first()
        fun RunConfiguration.slashed() = copy(command = command.replace('\\', '/'), cwd = cwd.replace('\\', '/'))
        assertEquals(listOf(expected), received.map { it.slashed() })
        assertTrue("\"kind\":\"springBoot\"" in gson.toJson(received.first()))
        daemon.close()
    }

    @Test
    fun `run configurations before project open fail with NOT_OPENED`() {
        val daemon = BreweryDaemon()
        val e = assertFailsWith<ExecutionException> { wire(daemon).runConfigurations(RunConfigurationsParams()).get(10, TimeUnit.SECONDS) }
        assertEquals(ErrorCodes.NOT_OPENED, (e.cause as ResponseErrorException).responseError.code)
        daemon.close()
    }
}
