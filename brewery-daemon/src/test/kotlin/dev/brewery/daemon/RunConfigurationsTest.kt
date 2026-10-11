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

    private val bootPlugin = "<build><plugins><plugin><groupId>org.springframework.boot</groupId>" +
        "<artifactId>spring-boot-maven-plugin</artifactId></plugin></plugins></build>"

    private fun pom(body: String = "") =
        "<project><modelVersion>4.0.0</modelVersion><groupId>g</groupId><artifactId>a</artifactId><version>1</version>$body</project>"

    private fun writePom(dir: Path, text: String) {
        Files.createDirectories(dir)
        Files.writeString(dir.resolve("pom.xml"), text)
    }

    @Test
    fun `maven runs spring-boot run, selecting a submodule with -pl and no -am`() {
        val root = rootWith("mvnw", "mvnw.cmd")
        writePom(root, pom(bootPlugin))
        writePom(root.resolve("core"), pom(bootPlugin))
        val project = ProjectModel(BuildTool.maven, root.toString(), listOf(BuildModule("r", ".", emptyList()), BuildModule("core", "core", emptyList())))
        val result = RunConfigurations.build(project, spring(app("Top", "."), app("Core", "core")), windows = true)
        assertEquals(listOf(listOf("spring-boot:run"), listOf("-pl", "core", "spring-boot:run")), result.map { it.args })
        assertTrue(result.all { it.kind == RunKind.springBoot && it.command == root.resolve("mvnw.cmd").toString() && it.cwd == root.toString() })
        val bare = Files.createTempDirectory("brewery-run-bare")
        writePom(bare, pom(bootPlugin))
        val noWrapper = ProjectModel(BuildTool.maven, bare.toString(), listOf(BuildModule("r", ".", emptyList())))
        assertEquals("mvn", RunConfigurations.build(noWrapper, spring(app("Top", ".")), windows = false).single().command)
    }

    @Test
    fun `maven emits nothing unless the boot plugin is declared`() {
        val root = rootWith()
        writePom(root, pom())
        writePom(root.resolve("core"), pom())
        val project = ProjectModel(BuildTool.maven, root.toString(), listOf(BuildModule("r", ".", emptyList()), BuildModule("core", "core", emptyList())))
        assertEquals(emptyList(), RunConfigurations.build(project, spring(app("Top", "."), app("Core", "core")), windows = false))
        // No pom at all (stale model) is also nothing.
        Files.delete(root.resolve("core/pom.xml"))
        assertEquals(emptyList(), RunConfigurations.build(project, spring(app("Core", "core")), windows = false))
    }

    @Test
    fun `maven finds the boot plugin in an ancestor pom or the starter parent`() {
        val root = rootWith()
        val parent = "<parent><groupId>g</groupId><artifactId>a</artifactId><version>1</version></parent>"
        writePom(root, pom("<build><pluginManagement><plugins><plugin><groupId>org.springframework.boot</groupId>" +
            "<artifactId>spring-boot-maven-plugin</artifactId></plugin></plugins></pluginManagement></build>"))
        writePom(root.resolve("core"), pom(parent))
        writePom(root.resolve("starter"), pom(
            "<parent><groupId>org.springframework.boot</groupId><artifactId>spring-boot-starter-parent</artifactId>" +
                "<version>3.3.0</version><relativePath/></parent>",
        ))
        val project = ProjectModel(
            BuildTool.maven, root.toString(),
            listOf(BuildModule("r", ".", emptyList()), BuildModule("core", "core", emptyList()), BuildModule("s", "starter", emptyList())),
        )
        val result = RunConfigurations.build(project, spring(app("Core", "core"), app("Starter", "starter")), windows = false)
        assertEquals(listOf("Core", "Starter"), result.map { it.label })
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
    fun `run configurations over the wire round-trip every fixture entry`() {
        val daemon = BreweryDaemon(
            windows = false,
            modelReader = {
                ProjectModel(
                    BuildTool.gradle, it.toString(),
                    listOf(BuildModule("r", ":", tasks("bootRun")), BuildModule("reports", ":reports", tasks("run"))),
                )
            },
        )
        val root = Files.createTempDirectory("brewery-run-wire")
        petshop.toFile().copyRecursively(root.toFile())
        Files.writeString(root.resolve("gradlew"), "")
        val reports = Files.createDirectories(root.resolve("reports/src/main/java/com/example/reports"))
        Files.writeString(
            reports.resolve("ReportsApplication.java"),
            "package com.example.reports;\n@SpringBootApplication\npublic class ReportsApplication {}\n",
        )
        val server = wire(daemon)
        server.openProject(OpenProjectParams(root.toString())).get(10, TimeUnit.SECONDS)
        val received = server.runConfigurations(RunConfigurationsParams()).get(20, TimeUnit.SECONDS)

        val placeholder = "/path/to/spring-petshop"
        val text = File("protocol/fixtures/run-configurations.json").readText()
            .replace(placeholder, root.toString().replace('\\', '/'))
        val expected = gson.fromJson(text, Array<RunConfiguration>::class.java).toList()
        fun RunConfiguration.slashed() = copy(command = command.replace('\\', '/'), cwd = cwd.replace('\\', '/'))
        assertEquals(expected.map { it.label }, received.map { it.label })
        assertEquals(expected, received.map { it.slashed() })
        val json = gson.toJson(received)
        assertTrue("\"kind\":\"springBoot\"" in json && "\"kind\":\"application\"" in json, json)
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
