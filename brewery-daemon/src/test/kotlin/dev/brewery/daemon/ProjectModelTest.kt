package dev.brewery.daemon

import dev.brewery.daemon.project.GradleModelReader
import dev.brewery.daemon.project.MavenModelReader
import dev.brewery.daemon.project.detectBuildTool
import org.eclipse.lsp4j.jsonrpc.Launcher
import org.eclipse.lsp4j.jsonrpc.ResponseErrorException
import org.eclipse.lsp4j.jsonrpc.json.MessageJsonHandler
import org.junit.jupiter.api.Tag
import org.junit.jupiter.api.Test
import java.io.File
import java.io.PipedInputStream
import java.io.PipedOutputStream
import java.nio.file.Files
import java.util.concurrent.CountDownLatch
import java.util.concurrent.ExecutionException
import java.util.concurrent.TimeUnit
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

class ProjectModelTest {
    private fun fixture(name: String) = File("src/test/resources/projects/$name").absoluteFile.toPath()
    private val lifecycle = listOf("clean", "validate", "compile", "test", "package", "verify", "install", "deploy")

    @Test
    fun `detectBuildTool recognises the fixtures`() {
        assertEquals(BuildTool.gradle, detectBuildTool(fixture("gradle-multi")))
        assertEquals(BuildTool.maven, detectBuildTool(fixture("maven-multi")))
        assertEquals(BuildTool.none, detectBuildTool(fixture("plain")))
    }

    @Test
    fun `maven model lists root and modules with lifecycle tasks`() {
        val model = MavenModelReader().read(fixture("maven-multi"))
        assertEquals(BuildTool.maven, model.buildTool)
        assertEquals(listOf("maven-multi", "core", "web"), model.modules.map { it.name })
        assertEquals(listOf(".", "core", "web"), model.modules.map { it.path })
        for (module in model.modules) {
            assertEquals(lifecycle, module.tasks.map { it.name })
            assertTrue(module.tasks.all { it.group == "lifecycle" && it.description == null })
        }
    }

    @Test
    @Tag("slow")
    fun `gradle model lists projects and tasks`() {
        val model = GradleModelReader().read(fixture("gradle-multi"))
        assertEquals(BuildTool.gradle, model.buildTool)
        assertEquals(listOf(":", ":app", ":lib"), model.modules.map { it.path })
        val app = model.modules.first { it.path == ":app" }
        val names = app.tasks.map { it.name }
        assertTrue("build" in names && "test" in names, "tasks: $names")
        assertEquals(names.sorted(), names)

        // The fixture holds a reduced example, so only its shape is checked against the real model.
        val gson = MessageJsonHandler(emptyMap()).gson
        val sample = gson.fromJson(File("protocol/fixtures/project-model.gradle.json").readText(), ProjectModel::class.java)
        assertEquals(BuildTool.gradle, sample.buildTool)
        val build = sample.modules.single().tasks.single()
        assertEquals(BuildTask("build", "build", "Assembles and tests this project."), build)
        assertTrue(app.tasks.any { it.name == "build" && it.group == build.group })
    }

    @Test
    fun `projectModel before openProject fails with NOT_OPENED over the wire`() {
        val clientToServer = PipedOutputStream()
        val serverIn = PipedInputStream(clientToServer)
        val serverToClient = PipedOutputStream()
        val clientIn = PipedInputStream(serverToClient)

        val daemon = BreweryDaemon()
        val serverLauncher = Launcher.createLauncher(daemon, BreweryClient::class.java, serverIn, serverToClient)
        daemon.connect(serverLauncher.remoteProxy)
        serverLauncher.startListening()
        val clientLauncher = Launcher.createLauncher(object : BreweryClient {
            override fun projectChanged(params: ProjectChanged) {}
        }, BreweryServer::class.java, clientIn, clientToServer)
        clientLauncher.startListening()

        val e = assertFailsWith<ExecutionException> {
            clientLauncher.remoteProxy.projectModel(ProjectModelParams()).get(10, TimeUnit.SECONDS)
        }
        val cause = e.cause as ResponseErrorException
        assertEquals(ErrorCodes.NOT_OPENED, cause.responseError.code)
        daemon.close()
    }

    @Test
    fun `maven project opened through the daemon yields a model and a broken pom gives BUILD_IMPORT_FAILED`() {
        val daemon = BreweryDaemon()
        daemon.openProject(OpenProjectParams(fixture("maven-multi").toString())).get(10, TimeUnit.SECONDS)
        val model = daemon.projectModel(ProjectModelParams()).get(10, TimeUnit.SECONDS)
        assertEquals(3, model.modules.size)

        val broken = Files.createTempDirectory("brewery-broken")
        Files.writeString(broken.resolve("pom.xml"), "<project><oops")
        daemon.openProject(OpenProjectParams(broken.toString())).get(10, TimeUnit.SECONDS)
        val e = assertFailsWith<ExecutionException> { daemon.projectModel(ProjectModelParams()).get(10, TimeUnit.SECONDS) }
        assertEquals(ErrorCodes.BUILD_IMPORT_FAILED, (e.cause as ResponseErrorException).responseError.code)
        daemon.close()
    }

    @Test
    fun `editing a build file sends project changed and drops the cache`() {
        val dir = Files.createTempDirectory("brewery-watch")
        val pom = dir.resolve("pom.xml")
        Files.writeString(pom, "<project><modelVersion>4.0.0</modelVersion><artifactId>one</artifactId></project>")
        val changed = CountDownLatch(1)
        val daemon = BreweryDaemon()
        daemon.connect(object : BreweryClient {
            override fun projectChanged(params: ProjectChanged) = changed.countDown()
        })
        daemon.openProject(OpenProjectParams(dir.toString())).get(10, TimeUnit.SECONDS)
        assertEquals("one", daemon.projectModel(ProjectModelParams()).get(10, TimeUnit.SECONDS).modules[0].name)

        Files.writeString(pom, "<project><modelVersion>4.0.0</modelVersion><artifactId>two</artifactId></project>")
        assertTrue(changed.await(20, TimeUnit.SECONDS), "no project/changed notification")
        assertEquals("two", daemon.projectModel(ProjectModelParams()).get(10, TimeUnit.SECONDS).modules[0].name)
        daemon.close()
    }

    @Test
    @Tag("slow")
    fun `gradle script error message carries the cause chain`() {
        val daemon = BreweryDaemon()
        daemon.openProject(OpenProjectParams(fixture("gradle-broken").toString())).get(10, TimeUnit.SECONDS)
        val e = assertFailsWith<ExecutionException> { daemon.projectModel(ProjectModelParams()).get(5, TimeUnit.MINUTES) }
        val error = (e.cause as ResponseErrorException).responseError
        assertEquals(ErrorCodes.BUILD_IMPORT_FAILED, error.code)
        assertTrue("build.gradle.kts" in error.message, error.message)
        daemon.close()
    }

    @Test
    fun `openProject on a missing directory fails with BUILD_IMPORT_FAILED`() {
        val daemon = BreweryDaemon()
        val e = assertFailsWith<ExecutionException> {
            daemon.openProject(OpenProjectParams("C:/definitely/not/here")).get(10, TimeUnit.SECONDS)
        }
        val error = (e.cause as ResponseErrorException).responseError
        assertEquals(ErrorCodes.BUILD_IMPORT_FAILED, error.code)
        assertTrue(error.message.startsWith("Project directory does not exist"), error.message)
        daemon.close()
    }
}
