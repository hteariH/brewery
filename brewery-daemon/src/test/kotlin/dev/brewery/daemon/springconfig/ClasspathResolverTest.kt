package dev.brewery.daemon.springconfig

import org.junit.jupiter.api.Tag
import java.io.File
import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class ClasspathResolverTest {
    private class FakeRunner(private val output: String?, private val exit: Int = 0) : CommandRunner {
        var command: List<String> = emptyList()
        var dir: Path? = null
        override fun run(command: List<String>, workingDir: Path): Int {
            this.command = command
            dir = workingDir
            val arg = command.first { it.startsWith("-Dmdep.outputFile=") }
            if (output != null) File(arg.removePrefix("-Dmdep.outputFile=")).writeText(output)
            return exit
        }
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
    fun `maven failure or missing output gives an empty classpath`() {
        val root = Files.createTempDirectory("mvnx")
        assertEquals(emptyList(), MavenClasspathResolver(FakeRunner("x.jar", exit = 1)).resolve(root))
        assertEquals(emptyList(), MavenClasspathResolver(FakeRunner(null)).resolve(root))
    }

    @Test
    fun `parseClasspath splits on the platform separator`() {
        val text = "a.jar${File.pathSeparator}b.jar${File.pathSeparator}\n"
        assertEquals(listOf(File("a.jar"), File("b.jar")), parseClasspath(text))
    }

    @Test
    @Tag("slow")
    fun `gradle resolves implementation dependencies from a flat dir`() {
        val root = Files.createTempDirectory("gradle-cp")
        val libs = Files.createDirectories(root.resolve("libs"))
        makeJar(libs.toFile(), "starter-1.0.jar", mapOf("META-INF/spring-configuration-metadata.json" to MAIN_METADATA))
        Files.writeString(root.resolve("settings.gradle.kts"), "rootProject.name = \"cp\"\n")
        Files.writeString(
            root.resolve("build.gradle.kts"),
            """
            plugins { java }
            repositories { flatDir { dirs("libs") } }
            dependencies { implementation(":starter:1.0") }
            """.trimIndent(),
        )
        val jars = GradleClasspathResolver().resolve(root)
        assertEquals(listOf("starter-1.0.jar"), jars.map { it.name })
        assertTrue(SpringMetadata.fromJars(jars).find("server.port") != null)
    }
}
