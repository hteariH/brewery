package dev.brewery.daemon

import java.io.File
import java.util.zip.ZipFile
import kotlin.test.Test
import kotlin.test.assertTrue

class NoticesTest {
    @Test
    fun `shadow jar ships third-party notices`() {
        val jar = File("build/libs/brewery-daemon-all.jar")
        assertTrue(jar.isFile, "jar missing: the test task must depend on shadowJar")
        ZipFile(jar).use { zip ->
            val entry = zip.getEntry("THIRD-PARTY-NOTICES.txt")
            assertTrue(entry != null, "jar has no THIRD-PARTY-NOTICES.txt")
            val text = zip.getInputStream(entry).readBytes().toString(Charsets.UTF_8)
            assertTrue("Apache License" in text, "notices lack license text")

            // The build writes the runtime jar list, so a dependency without a section fails here.
            val runtimeJars = File("build/notices/runtime-jars.txt").readLines().filter { it.isNotBlank() }
            assertTrue(runtimeJars.size > 3, "runtime jar list looks empty: $runtimeJars")
            for (name in runtimeJars) {
                assertTrue("==== $name (" in text, "no notices section for $name")
            }
            assertTrue("about.html" in text, "lsp4j's about.html is missing")
        }
    }
}
