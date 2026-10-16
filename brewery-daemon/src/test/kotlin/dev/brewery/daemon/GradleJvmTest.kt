package dev.brewery.daemon

import dev.brewery.daemon.project.GradleJvm
import dev.brewery.daemon.project.GradleModelReader
import dev.brewery.daemon.project.GradleVersion
import dev.brewery.daemon.springconfig.GradleClasspathResolver
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.io.TempDir
import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

class GradleJvmTest {
    @TempDir
    lateinit var tmp: Path

    private val windows = System.getProperty("os.name").startsWith("Windows")

    private fun jdk(name: String, version: String?): Path {
        val home = tmp.resolve(name)
        val java = home.resolve("bin").resolve(if (windows) "java.exe" else "java")
        Files.createDirectories(java.parent)
        Files.writeString(java, "")
        if (version != null) Files.writeString(home.resolve("release"), "JAVA_VERSION=\"$version\"\nOS_NAME=\"x\"\n")
        return home
    }

    private fun project(gradle: String?): Path {
        val root = Files.createDirectories(tmp.resolve("project-$gradle"))
        if (gradle != null) {
            val wrapper = root.resolve("gradle/wrapper/gradle-wrapper.properties")
            Files.createDirectories(wrapper.parent)
            Files.writeString(
                wrapper,
                "distributionUrl=https\\://services.gradle.org/distributions/gradle-$gradle-bin.zip\n",
            )
        }
        return root
    }

    private fun jvm(vararg env: Pair<String, String>, daemonJava: Int = 21) =
        GradleJvm(env = mapOf(*env)::get, userHome = tmp.resolve("home"), daemonJavaFeature = daemonJava)

    @Test
    fun `gradle runs on JAVA_HOME when the wrapper can run on it`() {
        val home = jdk("jdk17", "17.0.12")
        val root = project("8.4")
        val jvm = jvm("JAVA_HOME" to home.toString())
        assertEquals(home.toFile(), jvm.javaHome(root, FALLBACK))
        jvm.check(root, FALLBACK, home.toFile())
    }

    @Test
    fun `a JAVA_HOME the wrapper cannot run on falls back to the daemon's JVM`() {
        val root = project("8.14.3")
        assertNull(jvm("JAVA_HOME" to jdk("jdk25", "25.0.1").toString()).javaHome(root, FALLBACK))
        // Gradle 9 needs Java 17.
        assertNull(jvm("JAVA_HOME" to jdk("jdk11", "11.0.2").toString()).javaHome(project("9.4.0"), FALLBACK))
    }

    @Test
    fun `an old wrapper on the bundled Java 21 is a clear error`() {
        val root = project("8.4")
        val jvm = jvm("JAVA_HOME" to jdk("jdk21", "21.0.2").toString())
        assertNull(jvm.javaHome(root, FALLBACK))
        val error = assertFailsWith<IllegalStateException> { jvm.check(root, FALLBACK, null) }
        assertEquals(
            "Gradle 8.4 cannot run on Java 21 (Brewery's Java 21 runtime). Set JAVA_HOME to a JDK 20 or older, " +
                "or org.gradle.java.home in gradle.properties, or upgrade the Gradle wrapper to 8.5 or newer.",
            error.message,
        )
    }

    @Test
    fun `org_gradle_java_home is left to Gradle`() {
        val root = project("8.4")
        Files.writeString(root.resolve("gradle.properties"), "org.gradle.java.home=/opt/jdk17\n")
        val jvm = jvm("JAVA_HOME" to jdk("jdk17", "17").toString())
        assertNull(jvm.javaHome(root, FALLBACK))
        jvm.check(root, FALLBACK, null)

        val other = project("8.3")
        val userHome = Files.createDirectories(tmp.resolve("gradle-user-home"))
        Files.writeString(userHome.resolve("gradle.properties"), "org.gradle.java.home=/opt/jdk17\n")
        val withUserHome = jvm("JAVA_HOME" to jdk("jdk17b", "17").toString(), "GRADLE_USER_HOME" to userHome.toString())
        assertNull(withUserHome.javaHome(other, FALLBACK))
        withUserHome.check(other, FALLBACK, null)
    }

    @Test
    fun `without a wrapper the fallback version decides`() {
        val root = project(null)
        val home = jdk("jdk25", "25")
        assertEquals(home.toFile(), jvm("JAVA_HOME" to home.toString()).javaHome(root, FALLBACK))
        jvm().check(root, FALLBACK, null)
    }

    @Test
    fun `no usable JAVA_HOME means the daemon's JVM`() {
        val root = project("9.4.0")
        assertNull(jvm().javaHome(root, FALLBACK))
        assertNull(jvm("JAVA_HOME" to tmp.resolve("missing").toString()).javaHome(root, FALLBACK))
        assertNull(jvm("JAVA_HOME" to "").javaHome(root, FALLBACK))
        // A JDK without a release file: its version is unknown, so it is used as it is.
        val bare = jdk("bare", null)
        assertEquals(bare.toFile(), jvm("JAVA_HOME" to bare.toString()).javaHome(root, FALLBACK))
    }

    @Test
    fun `the model reader and the classpath resolver report an old wrapper before starting Gradle`() {
        // Gradle 8.4 is never downloaded: the check comes first.
        val root = project("8.4")
        Files.writeString(root.resolve("settings.gradle.kts"), "rootProject.name = \"old\"\n")
        val message = "Gradle 8.4 cannot run on Java 21"
        val error = assertFailsWith<IllegalStateException> { GradleModelReader(jvm = jvm()).read(root) }
        assertTrue(error.message!!.startsWith(message), error.message)
        val classpath = assertFailsWith<IllegalStateException> { GradleClasspathResolver(jvm = jvm()).resolve(root) }
        assertTrue(classpath.message!!.startsWith(message), classpath.message)
    }

    @Test
    fun `compatibility matrix`() {
        fun can(gradle: String, java: Int) = GradleJvm.canRun(GradleVersion.parse(gradle)!!, java)
        assertTrue(can("8.4", 17))
        assertTrue(can("8.4", 8))
        assertFalse(can("8.4", 21))
        assertTrue(can("8.5", 21))
        assertTrue(can("8.10.2", 23))
        assertFalse(can("8.14.3", 25))
        assertTrue(can("9.1.0", 25))
        assertTrue(can("9.8.1", 26))
        assertFalse(can("9.8.1", 27))
        assertFalse(can("9.0.0", 11))
        assertEquals(8, GradleJvm.javaFeature(jdk("jdk8", "1.8.0_402")))
        assertEquals(GradleVersion(8, 10, "8.10.2"), GradleJvm.gradleVersion(project("8.10.2"), FALLBACK))
    }

    private companion object {
        const val FALLBACK = "9.8.1"
    }
}
