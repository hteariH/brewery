package dev.brewery.daemon.project

import java.io.File
import java.nio.file.Files
import java.nio.file.Path
import java.util.Properties

/**
 * Which JVM the daemon's Gradle Tooling API calls run Gradle on. Without a choice Gradle runs on the daemon's
 * own JVM, which in a packaged Brewery is the bundled Java 21 runtime: Gradle before 8.5 cannot run on it.
 *
 * Like `gradlew`: `org.gradle.java.home` in gradle.properties wins (Gradle reads it itself), then the user's
 * JAVA_HOME, as long as the build's Gradle can run on it (else the daemon's JVM, which jdtls's launcher does
 * too). [check] turns a Gradle that cannot run on the chosen JVM into an error that says what to change.
 */
class GradleJvm(
    private val env: (String) -> String? = System::getenv,
    private val userHome: Path = Path.of(System.getProperty("user.home")),
    private val daemonJavaFeature: Int = Runtime.version().feature(),
) {
    /** The Java home to hand the Tooling API for [root], or null for Gradle's own choice. */
    fun javaHome(root: Path, fallbackGradleVersion: String): File? {
        if (configuredJavaHome(root) != null) return null
        val home = userJavaHome() ?: return null
        val gradle = gradleVersion(root, fallbackGradleVersion)
        val feature = javaFeature(home)
        if (gradle != null && feature != null && !canRun(gradle, feature)) return null
        return home.toFile()
    }

    /** Throws when the Gradle of [root] cannot run on the JVM [javaHome] chose (null: the daemon's). */
    fun check(root: Path, fallbackGradleVersion: String, javaHome: File?) {
        if (configuredJavaHome(root) != null) return
        val gradle = gradleVersion(root, fallbackGradleVersion) ?: return
        val feature = (if (javaHome == null) daemonJavaFeature else javaFeature(javaHome.toPath())) ?: return
        if (canRun(gradle, feature)) return
        val runsOn = javaHome?.let { "the JDK at $it" } ?: "Brewery's Java $daemonJavaFeature runtime"
        val tooOld = gradle.major >= 9 && feature < 17
        val newest = MIN_GRADLE_FOR_JAVA.lastOrNull { (_, minGradle) -> gradle >= minGradle }?.first
        val want = if (tooOld) "a JDK 17 or newer" else "a JDK ${newest ?: 8} or older"
        val upgrade = MIN_GRADLE_FOR_JAVA.firstOrNull { (java, _) -> java == feature }?.second?.takeUnless { tooOld }
            ?.let { ", or upgrade the Gradle wrapper to ${it.text} or newer" } ?: ""
        throw IllegalStateException(
            "Gradle ${gradle.text} cannot run on Java $feature ($runsOn). Set JAVA_HOME to $want, or " +
                "org.gradle.java.home in gradle.properties$upgrade."
        )
    }

    private fun configuredJavaHome(root: Path): String? {
        val gradleUserHome = env("GRADLE_USER_HOME")?.takeIf { it.isNotBlank() }?.let { Path.of(it) }
            ?: userHome.resolve(".gradle")
        return listOf(gradleUserHome.resolve("gradle.properties"), root.resolve("gradle.properties"))
            .firstNotNullOfOrNull { properties(it)?.getProperty("org.gradle.java.home")?.takeIf(String::isNotBlank) }
    }

    private fun userJavaHome(): Path? {
        val value = env("JAVA_HOME")?.trim()?.removeSurrounding("\"")?.takeIf { it.isNotEmpty() } ?: return null
        val home = try {
            Path.of(value)
        } catch (_: Exception) {
            return null
        }
        val java = home.resolve("bin").resolve(if (WINDOWS) "java.exe" else "java")
        return home.takeIf { Files.isRegularFile(java) }
    }

    companion object {
        private val WINDOWS = System.getProperty("os.name").startsWith("Windows")

        /**
         * The oldest Gradle that runs on each Java, as in jdtls's GradleUtils.getHighestSupportedJava
         * (https://docs.gradle.org/current/userguide/compatibility.html). Newer Java: unknown, so no.
         */
        internal val MIN_GRADLE_FOR_JAVA: List<Pair<Int, GradleVersion>> = listOf(
            8 to "2.0", 9 to "4.3", 10 to "4.7", 11 to "5.0", 12 to "5.4", 13 to "6.0", 14 to "6.3", 15 to "6.7",
            16 to "7.0", 17 to "7.3", 18 to "7.5", 19 to "7.6", 20 to "8.3", 21 to "8.5", 22 to "8.8",
            23 to "8.10", 24 to "8.14", 25 to "9.1", 26 to "9.4",
        ).map { (java, gradle) -> java to GradleVersion.parse(gradle)!! }

        internal fun canRun(gradle: GradleVersion, javaFeature: Int): Boolean {
            if (gradle.major >= 9 && javaFeature < 17) return false
            if (javaFeature > MIN_GRADLE_FOR_JAVA.last().first) return false
            val minGradle = MIN_GRADLE_FOR_JAVA.lastOrNull { (java, _) -> java <= javaFeature }?.second ?: return true
            return gradle >= minGradle
        }

        /** The wrapper's Gradle version, or [fallback] (what the daemon uses) without a wrapper. */
        internal fun gradleVersion(root: Path, fallback: String): GradleVersion? {
            val wrapper = root.resolve("gradle/wrapper/gradle-wrapper.properties")
            if (!Files.exists(wrapper)) return GradleVersion.parse(fallback)
            val url = properties(wrapper)?.getProperty("distributionUrl") ?: return null
            val name = url.substringAfterLast('/')
            if (!name.startsWith("gradle-")) return null
            return GradleVersion.parse(name.removePrefix("gradle-").substringBefore('-'))
        }

        /** The feature version in a Java home's `release` file (1.8.0_x is 8), or null. */
        internal fun javaFeature(home: Path): Int? {
            val version = properties(home.resolve("release"))?.getProperty("JAVA_VERSION")
                ?.removeSurrounding("\"") ?: return null
            val parts = version.split('.', '_', '-', '+')
            val feature = if (parts[0] == "1") parts.getOrNull(1) else parts[0]
            return feature?.toIntOrNull()
        }

        private fun properties(file: Path): Properties? = try {
            if (!Files.isRegularFile(file)) null else Properties().apply { Files.newInputStream(file).use { load(it) } }
        } catch (_: Exception) {
            null
        }
    }
}

/** major.minor of a Gradle version; enough for the compatibility matrix. */
data class GradleVersion(val major: Int, val minor: Int, val text: String) : Comparable<GradleVersion> {
    override fun compareTo(other: GradleVersion) = compareValuesBy(this, other, { it.major }, { it.minor })

    companion object {
        fun parse(text: String): GradleVersion? {
            val parts = text.split('.')
            val major = parts.getOrNull(0)?.toIntOrNull() ?: return null
            val minor = parts.getOrNull(1)?.takeWhile(Char::isDigit)?.toIntOrNull() ?: 0
            return GradleVersion(major, minor, text)
        }
    }
}
