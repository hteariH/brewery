package dev.brewery.daemon

import dev.brewery.daemon.spring.HttpMethod
import dev.brewery.daemon.spring.SpringBeanKind
import dev.brewery.daemon.spring.SpringModel
import dev.brewery.daemon.spring.SpringScanner
import org.eclipse.lsp4j.jsonrpc.Launcher
import org.eclipse.lsp4j.jsonrpc.ResponseErrorException
import org.eclipse.lsp4j.jsonrpc.json.MessageJsonHandler
import org.junit.jupiter.api.Test
import java.io.File
import java.io.PipedInputStream
import java.io.PipedOutputStream
import java.nio.file.Files
import java.nio.file.Path
import java.util.concurrent.CountDownLatch
import java.util.concurrent.ExecutionException
import java.util.concurrent.TimeUnit
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

class SpringScannerTest {
    private val gson = MessageJsonHandler(emptyMap()).gson
    private val petshop: Path = File("src/test/resources/projects/spring-petshop").absoluteFile.toPath().normalize()

    private fun model(root: Path) =
        ProjectModel(BuildTool.gradle, root.toString(), listOf(BuildModule(root.fileName.toString(), ":", emptyList())))

    private fun scan(root: Path = petshop) = SpringScanner().scan(model(root))

    private fun copyPetshop(): Path {
        val target = Files.createTempDirectory("brewery-spring")
        petshop.toFile().copyRecursively(target.toFile())
        return target
    }

    @Test
    fun `fixture yields the application, beans and endpoints`() {
        val result = scan()
        assertEquals(listOf("com.example.petshop.PetshopApplication"), result.applications.map { it.mainClass })
        assertEquals("PetshopApplication", result.applications.single().name)
        assertEquals(":", result.applications.single().modulePath)

        assertEquals(
            listOf(
                Triple("appConfig", SpringBeanKind.configuration, "com.example.petshop.AppConfig"),
                Triple("clock", SpringBeanKind.bean, "Clock"),
                Triple("ownerController", SpringBeanKind.restController, "com.example.petshop.OwnerController"),
                Triple("ownerRepository", SpringBeanKind.repository, "com.example.petshop.OwnerRepository"),
                Triple("ownerService", SpringBeanKind.service, "com.example.petshop.OwnerService"),
            ),
            result.beans.map { Triple(it.name, it.kind, it.type) },
        )

        assertEquals(
            listOf(
                Triple(HttpMethod.GET, "/owners", "OwnerController#list"),
                Triple(HttpMethod.GET, "/owners/{id}", "OwnerController#get"),
                Triple(HttpMethod.POST, "/owners", "OwnerController#create"),
                Triple(HttpMethod.ANY, "/owners/legacy", "OwnerController#legacy"),
            ),
            result.endpoints.map { Triple(it.method, it.path, it.handler) },
        )
    }

    @Test
    fun `files are absolute and lines point at the annotated declaration`() {
        val result = scan()
        fun lineText(file: String, line: Int) = File(file).readLines()[line - 1].trim()

        for (file in result.beans.map { it.file } + result.endpoints.map { it.file } + result.applications.map { it.file }) {
            assertTrue(File(file).isAbsolute && File(file).isFile, file)
        }
        assertEquals("@SpringBootApplication", lineText(result.applications.single().file, result.applications.single().line))
        assertEquals("@RestController", lineText(result.beans.first { it.name == "ownerController" }.let { it.file }, result.beans.first { it.name == "ownerController" }.line))
        assertEquals("@Bean", lineText(result.beans.first { it.name == "clock" }.file, result.beans.first { it.name == "clock" }.line))
        val expected = listOf("@GetMapping", "@GetMapping(\"/{id}\")", "@PostMapping", "@RequestMapping(\"/legacy\")")
        assertEquals(expected, result.endpoints.map { lineText(it.file, it.line) })
    }

    @Test
    fun `a file with a syntax error is skipped without failing the scan`() {
        val root = copyPetshop()
        val dir = root.resolve("src/main/java/com/example/petshop")
        Files.writeString(
            dir.resolve("Broken.java"),
            "package com.example.petshop;\n@org.springframework.stereotype.Service\npublic class Broken { void f( { }\n",
        )
        val result = scan(root)
        assertEquals(5, result.beans.size)
        assertTrue(result.beans.none { it.name == "broken" })
    }

    @Test
    fun `explicit names, qualified annotations, request methods and nested paths`() {
        val root = Files.createTempDirectory("brewery-spring-edge")
        val dir = Files.createDirectories(root.resolve("src/main/java/p"))
        Files.writeString(
            dir.resolve("Edge.java"),
            """
            package p;
            @org.springframework.stereotype.Service("custom")
            class Edge {
                @org.springframework.context.annotation.Bean(name = "named")
                Foo foo() { return null; }
            }
            @Controller
            @RequestMapping(path = {"api/", "other"})
            class Ctl {
                @RequestMapping(value = "x/", method = RequestMethod.PUT) void put() {}
                @RequestMapping(method = {DELETE, GET}) void del() {}
                @PatchMapping("/p") void patch() {}
            }
            """.trimIndent(),
        )
        val result = scan(root)
        assertEquals(listOf("custom" to SpringBeanKind.service, "named" to SpringBeanKind.bean, "ctl" to SpringBeanKind.controller),
            result.beans.map { it.name to it.kind })
        assertEquals(
            listOf(
                Triple(HttpMethod.PUT, "/api/x", "Ctl#put"),
                Triple(HttpMethod.DELETE, "/api", "Ctl#del"),
                Triple(HttpMethod.PATCH, "/api/p", "Ctl#patch"),
            ),
            result.endpoints.map { Triple(it.method, it.path, it.handler) },
        )
    }

    @Test
    fun `maven and nested gradle modules map to their source directories`() {
        val root = Files.createTempDirectory("brewery-spring-modules")
        for (module in listOf("core", "a/b")) {
            val dir = Files.createDirectories(root.resolve("$module/src/main/java"))
            Files.writeString(dir.resolve("S.java"), "package x; @Service class S {}")
        }
        val maven = SpringScanner().scan(
            ProjectModel(BuildTool.maven, root.toString(), listOf(BuildModule("r", ".", emptyList()), BuildModule("core", "core", emptyList()))),
        )
        assertEquals(1, maven.beans.size)
        val gradle = SpringScanner().scan(
            ProjectModel(BuildTool.gradle, root.toString(), listOf(BuildModule("b", ":a:b", emptyList()))),
        )
        assertEquals(1, gradle.beans.size)
        assertTrue(gradle.beans.single().file.replace('\\', '/').contains("/a/b/src/main/java/"))
    }

    @Test
    fun `generated and build output directories are not scanned`() {
        val root = copyPetshop()
        val java = "package x; @Service class Hidden {}"
        for (dir in listOf("src/main/java/com/example/petshop/build", "src/main/java/target", "build/generated", "target/classes")) {
            Files.writeString(Files.createDirectories(root.resolve(dir)).resolve("Hidden.java"), java)
        }
        val result = scan(root)
        assertEquals(5, result.beans.size)
        assertTrue(result.beans.none { it.name == "hidden" })
    }

    private class Wire(daemon: BreweryDaemon, client: BreweryClient) {
        val server: BreweryServer
        init {
            val clientToServer = PipedOutputStream()
            val serverIn = PipedInputStream(clientToServer)
            val serverToClient = PipedOutputStream()
            val clientIn = PipedInputStream(serverToClient)
            val serverLauncher = Launcher.createLauncher(daemon, BreweryClient::class.java, serverIn, serverToClient)
            daemon.connect(serverLauncher.remoteProxy)
            serverLauncher.startListening()
            val clientLauncher = Launcher.createLauncher(client, BreweryServer::class.java, clientIn, clientToServer)
            clientLauncher.startListening()
            server = clientLauncher.remoteProxy
        }
    }

    private val quietClient = object : BreweryClient {
        override fun projectChanged(params: ProjectChanged) {}
        override fun springChanged(params: SpringChanged) {}
    }

    @Test
    fun `spring model over the wire round-trips the fixture shape`() {
        val daemon = BreweryDaemon(modelReader = { model(it) })
        val wire = Wire(daemon, quietClient)
        wire.server.openProject(OpenProjectParams(petshop.toString())).get(10, TimeUnit.SECONDS)
        val received = wire.server.springModel(SpringModelParams()).get(20, TimeUnit.SECONDS)

        val placeholder = "/path/to/spring-petshop"
        val text = File("protocol/fixtures/spring-model.json").readText().replace(placeholder, petshop.toString().replace('\\', '/'))
        val expected = gson.fromJson(text, SpringModel::class.java)
        fun SpringModel.slashed() = copy(
            applications = applications.map { it.copy(file = it.file.replace('\\', '/')) },
            beans = beans.map { it.copy(file = it.file.replace('\\', '/')) },
            endpoints = endpoints.map { it.copy(file = it.file.replace('\\', '/')) },
        )
        assertEquals(expected, received.slashed())
        // Enums travel by name, not ordinal.
        val json = gson.toJson(received)
        assertTrue("\"kind\":\"restController\"" in json && "\"method\":\"ANY\"" in json, json)
        daemon.close()
    }

    @Test
    fun `spring model before project open fails with NOT_OPENED`() {
        val daemon = BreweryDaemon()
        val wire = Wire(daemon, quietClient)
        val e = assertFailsWith<ExecutionException> { wire.server.springModel(SpringModelParams()).get(10, TimeUnit.SECONDS) }
        assertEquals(ErrorCodes.NOT_OPENED, (e.cause as ResponseErrorException).responseError.code)
        daemon.close()
    }

    @Test
    fun `editing a java file sends spring changed and refreshes the model`() {
        val root = copyPetshop()
        val changed = CountDownLatch(1)
        val projectChanged = CountDownLatch(1)
        val daemon = BreweryDaemon(modelReader = { model(it) }, debounceMillis = 200)
        daemon.connect(object : BreweryClient {
            override fun projectChanged(params: ProjectChanged) = projectChanged.countDown()
            override fun springChanged(params: SpringChanged) = changed.countDown()
        })
        daemon.openProject(OpenProjectParams(root.toString())).get(10, TimeUnit.SECONDS)
        assertEquals(5, daemon.springModel(SpringModelParams()).get(20, TimeUnit.SECONDS).beans.size)

        // A brand-new package directory exercises registering watches for directories created later.
        val pkg = Files.createDirectories(root.resolve("src/main/java/com/example/petshop/extra"))
        Files.writeString(pkg.resolve("Extra.java"), "package com.example.petshop.extra;\n@Component\npublic class Extra {}\n")
        assertTrue(changed.await(20, TimeUnit.SECONDS), "no spring/changed notification")
        val names = daemon.springModel(SpringModelParams()).get(20, TimeUnit.SECONDS).beans.map { it.name }
        assertTrue("extra" in names, "beans: $names")
        assertEquals(1, projectChanged.count, "a java edit must not look like a build-file edit")
        daemon.close()
    }

    @Test
    fun `a failed import is retried by the next spring model request`() {
        val attempts = java.util.concurrent.atomic.AtomicInteger()
        val daemon = BreweryDaemon(modelReader = {
            if (attempts.incrementAndGet() == 1) error("boom")
            model(it)
        })
        daemon.openProject(OpenProjectParams(petshop.toString())).get(10, TimeUnit.SECONDS)
        assertFailsWith<ExecutionException> { daemon.springModel(SpringModelParams()).get(10, TimeUnit.SECONDS) }
        assertEquals(5, daemon.springModel(SpringModelParams()).get(20, TimeUnit.SECONDS).beans.size)
        assertEquals(2, attempts.get())
        daemon.close()
    }

    @Test
    fun `second scan parses only changed files and drops removed ones`() {
        val root = copyPetshop()
        val scanner = SpringScanner()
        val first = scanner.scan(model(root))
        val total = scanner.parsedFiles
        assertTrue(total > 0)
        assertEquals(first, scanner.scan(model(root)))
        assertEquals(total, scanner.parsedFiles, "unchanged files must not be parsed again")

        val dir = Files.createDirectories(root.resolve("src/main/java/com/example/petshop/extra"))
        val extra = dir.resolve("Extra.java")
        Files.writeString(extra, "package x;\n@Component\npublic class Extra {}\n")
        assertTrue("extra" in scanner.scan(model(root)).beans.map { it.name })
        assertEquals(total + 1, scanner.parsedFiles)

        Files.delete(extra)
        assertEquals(first, scanner.scan(model(root)))
        assertEquals(total + 1, scanner.parsedFiles)
    }

    @Test
    fun `files over one megabyte are skipped`() {
        val root = copyPetshop()
        val dir = Files.createDirectories(root.resolve("src/main/java/big"))
        Files.writeString(dir.resolve("Big.java"), "package big;\n@Component\npublic class Big {}\n" + "//" + "x".repeat(1 shl 20) + "\n")
        assertTrue("big" !in scan(root).beans.map { it.name })
    }
}
