package dev.brewery.daemon.springconfig

import org.junit.jupiter.api.Tag
import java.io.File
import java.io.InputStream
import java.io.OutputStream
import java.nio.file.Files
import java.nio.file.Path
import java.time.Duration
import java.util.concurrent.CompletableFuture
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class ClasspathResolverTest {
    private class FakeRunner(private val output: String?, private val exit: Int = 0) : CommandRunner {
        var command: List<String> = emptyList()
        var dir: Path? = null
        override fun run(command: List<String>, workingDir: Path, resolution: Resolution): CommandResult {
            this.command = command
            dir = workingDir
            val arg = command.first { it.startsWith("-Dmdep.outputFile=") }
            if (output != null) File(arg.removePrefix("-Dmdep.outputFile=")).writeText(output)
            return CommandResult(exit, "[ERROR] Failed to execute goal on project broken")
        }
    }

    /** A child process that never finishes by itself; it ends only once destroyed. */
    private class HangingProcess : Process() {
        val destroyed = CountDownLatch(1)
        override fun getOutputStream(): OutputStream = OutputStream.nullOutputStream()
        override fun getInputStream(): InputStream = InputStream.nullInputStream()
        override fun getErrorStream(): InputStream = InputStream.nullInputStream()
        override fun waitFor(): Int { destroyed.await(); return exitValue() }
        override fun waitFor(timeout: Long, unit: TimeUnit): Boolean = destroyed.await(timeout, unit)
        override fun exitValue(): Int = if (destroyed.count == 0L) 137 else throw IllegalThreadStateException()
        override fun destroy() { destroyed.countDown() }
        override fun destroyForcibly(): Process { destroyed.countDown(); return this }
        override fun isAlive(): Boolean = destroyed.count > 0
    }

    @Test
    fun `maven partial output with a non-zero exit is still used and reported`() {
        val root = Files.createTempDirectory("mvn-partial")
        Files.writeString(root.resolve("pom.xml"), "<project/>")
        val warnings = ArrayList<String>()
        val runner = FakeRunner(File("/repo/a.jar").path, exit = 1)
        val jars = MavenClasspathResolver(runner).resolve(root, Resolution(warn = { warnings += it }))
        assertEquals(listOf(File(File("/repo/a.jar").path)), jars)
        assertTrue("-fae" in runner.command)
        assertEquals(1, warnings.size, warnings.toString())
        assertTrue(warnings[0].contains("exit code 1"), warnings[0])
        assertTrue(warnings[0].contains("Failed to execute goal"), warnings[0])
    }

    @Test
    fun `a maven run past its timeout is destroyed`() {
        val root = Files.createTempDirectory("mvn-timeout")
        Files.writeString(root.resolve("pom.xml"), "<project/>")
        val process = HangingProcess()
        val runner = ProcessCommandRunner(timeout = Duration.ofMillis(100)) { _, _, _ -> process }
        val warnings = ArrayList<String>()
        val jars = MavenClasspathResolver(runner).resolve(root, Resolution(warn = { warnings += it }))
        assertEquals(emptyList(), jars)
        assertFalse(process.isAlive)
        assertTrue(warnings.single().contains("did not finish"), warnings.toString())
    }

    @Test
    fun `cancelling a resolution destroys the running maven process`() {
        val root = Files.createTempDirectory("mvn-cancel")
        Files.writeString(root.resolve("pom.xml"), "<project/>")
        val process = HangingProcess()
        val started = CountDownLatch(1)
        val runner = ProcessCommandRunner(timeout = Duration.ofMinutes(5)) { _, _, _ -> started.countDown(); process }
        val resolution = Resolution()
        val done = CompletableFuture.supplyAsync { MavenClasspathResolver(runner).resolve(root, resolution) }
        assertTrue(started.await(10, TimeUnit.SECONDS))
        resolution.cancel()
        assertEquals(emptyList(), done.get(10, TimeUnit.SECONDS))
        assertFalse(process.isAlive)
    }

    @Test
    fun `maven builds the command line and parses the output file`() {
        val root = Files.createTempDirectory("mvn")
        Files.writeString(root.resolve("pom.xml"), "<project/>")
        val a = File("/repo/a.jar").path
        val b = File("/repo/b.jar").path
        val runner = FakeRunner(listOf(a, b, a, "").joinToString(File.pathSeparator) + "\n")
        val jars = MavenClasspathResolver(runner).resolve(root)
        assertEquals(listOf(File(a), File(b)), jars)
        assertEquals(root, runner.dir)
        assertTrue("-q" in runner.command)
        assertTrue("dependency:build-classpath" in runner.command)
        assertTrue(runner.command.any { it.startsWith("-Dmdep.outputFile=") })
        assertTrue("-Dmdep.appendOutput=true" in runner.command)
        // No wrapper in the fixture, so the plain executable is used.
        assertTrue(runner.command.first().startsWith("mvn"))
    }

    @Test
    fun `maven prefers the wrapper when present`() {
        val root = Files.createTempDirectory("mvnw")
        Files.writeString(root.resolve("pom.xml"), "<project/>")
        val name = if (System.getProperty("os.name").startsWith("Windows")) "mvnw.cmd" else "mvnw"
        Files.writeString(root.resolve(name), "")
        val runner = FakeRunner("")
        MavenClasspathResolver(runner).resolve(root)
        assertEquals(root.resolve(name).toString(), runner.command.first())
    }

    @Test
    fun `maven without output gives an empty classpath`() {
        val root = Files.createTempDirectory("mvnx")
        assertEquals(emptyList(), MavenClasspathResolver(FakeRunner(null, exit = 1)).resolve(root))
        assertEquals(emptyList(), MavenClasspathResolver(FakeRunner(null)).resolve(root))
    }

    @Test
    fun `parseClasspath splits on the platform separator`() {
        val text = "a.jar${File.pathSeparator}b.jar${File.pathSeparator}\n"
        assertEquals(listOf(File("a.jar"), File("b.jar")), parseClasspath(text))
    }

    @Test
    fun `parseClasspath splits module entries appended without a separator`() {
        assertEquals(
            listOf("/m2/a.jar", "/m2/b.jar", "/m2/c.jar").map(::File),
            parseClasspath("/m2/a.jar/m2/b.jar${File.pathSeparator}/m2/c.jar\n"),
        )
        // Drive letters only appear where `;` separates entries; with `:` they would be split apart anyway.
        if (File.pathSeparatorChar == ';') {
            assertEquals(
                listOf("C:\\m2\\a.jar", "D:/m2/b.jar", "C:\\m2\\c.jar").map(::File),
                parseClasspath("C:\\m2\\a.jarD:/m2/b.jar;C:\\m2\\c.jar\n"),
            )
        }
    }

    @Test
    fun `project metadata roots are the existing resource and output directories of each module`() {
        val module = Files.createTempDirectory("module")
        for (dir in listOf("src/main/resources", "build/classes/java/main", "build/resources/main", "target/classes")) {
            Files.createDirectories(module.resolve(dir).resolve("META-INF"))
        }
        val bare = Files.createTempDirectory("bare")
        Files.createDirectories(bare.resolve("build/resources/main/META-INF"))
        assertEquals(
            listOf(
                module.resolve("src/main/resources"), module.resolve("build/classes/java/main"),
                module.resolve("build/resources/main"), module.resolve("target/classes"),
                bare.resolve("build/resources/main"),
            ).map { it.toFile() },
            projectMetadataRoots(listOf(module, bare)),
        )
    }

    @Test
    fun `maven lists the module outputs before the jars`() {
        val root = Files.createTempDirectory("mvn-modules")
        Files.writeString(root.resolve("pom.xml"), "<project><modules><module>lib</module></modules></project>")
        Files.createDirectories(root.resolve("lib"))
        Files.writeString(root.resolve("lib/pom.xml"), "<project/>")
        val classes = writeMetadataDir(root.resolve("lib/target/classes").toFile(), "{}")
        val a = File("/repo/a.jar").path
        val jars = MavenClasspathResolver(FakeRunner(a)).resolve(root)
        assertEquals(listOf(classes, File(a)), jars)
    }

    @Test
    @Tag("slow")
    fun `gradle resolves implementation dependencies from a flat dir`() {
        val root = Files.createTempDirectory("gradle-cp")
        val libs = Files.createDirectories(root.resolve("libs"))
        makeJar(libs.toFile(), "starter-1.0.jar", mapOf("META-INF/spring-configuration-metadata.json" to MAIN_METADATA))
        Files.writeString(root.resolve("settings.gradle.kts"), "rootProject.name = \"cp\"\ninclude(\"lib\")\n")
        Files.createDirectories(root.resolve("lib"))
        Files.writeString(root.resolve("lib/build.gradle.kts"), "plugins { java }\n")
        val libResources = writeMetadataDir(root.resolve("lib/src/main/resources").toFile(), "{}")
        Files.writeString(
            root.resolve("build.gradle.kts"),
            """
            plugins { java }
            repositories { flatDir { dirs("libs") } }
            dependencies { implementation(":starter:1.0"); implementation(project(":lib")) }
            """.trimIndent(),
        )
        val jars = GradleClasspathResolver().resolve(root)
        // The sibling module's metadata directory comes first, then the jars.
        assertEquals(listOf(libResources.canonicalFile, File(libs.toFile(), "starter-1.0.jar").canonicalFile), jars.map { it.canonicalFile })
        assertTrue(SpringMetadata.fromClasspath(jars).find("server.port") != null)
    }
}
