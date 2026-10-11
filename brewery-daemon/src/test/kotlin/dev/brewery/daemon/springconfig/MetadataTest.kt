package dev.brewery.daemon.springconfig

import java.io.File
import java.nio.file.Files
import java.util.zip.ZipEntry
import java.util.zip.ZipOutputStream
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** Writes a jar holding the given entries (name to text) into the given directory. */
fun makeJar(dir: File, name: String, entries: Map<String, String>): File {
    val jar = File(dir, name)
    ZipOutputStream(jar.outputStream()).use { zip ->
        for ((entry, text) in entries) {
            zip.putNextEntry(ZipEntry(entry))
            zip.write(text.toByteArray())
            zip.closeEntry()
        }
    }
    return jar
}

const val MAIN_METADATA = """
{
  "groups": [{"name": "server", "type": "x.ServerProperties"}],
  "properties": [
    {"name": "server.port", "type": "java.lang.Integer", "description": "Server HTTP port.", "defaultValue": 8080},
    {"name": "server.servlet.context-path", "type": "java.lang.String", "description": "Context path."},
    {"name": "spring.main.banner-mode", "type": "org.springframework.boot.Banner${'$'}Mode", "defaultValue": "console"},
    {"name": "spring.jpa.show-sql", "type": "java.lang.Boolean", "defaultValue": false},
    {"name": "spring.jpa.properties", "type": "java.util.Map<java.lang.String,java.lang.String>"},
    {"name": "old.setting", "type": "java.lang.String",
     "deprecation": {"level": "error", "replacement": "new.setting", "reason": "Renamed."}},
    {"name": "plain.deprecated", "type": "java.lang.String", "deprecation": {}}
  ],
  "hints": [
    {"name": "spring.main.banner-mode", "values": [
      {"value": "console", "description": "Print to stdout."}, {"value": "off"}]}
  ]
}
"""

class MetadataTest {
    private fun tmp(): File = Files.createTempDirectory("meta").toFile()

    private fun mainJar(): File =
        makeJar(tmp(), "a.jar", mapOf("META-INF/spring-configuration-metadata.json" to MAIN_METADATA))

    @Test
    fun `parses properties hints and deprecation`() {
        val meta = SpringMetadata.fromJars(listOf(mainJar()))
        assertFalse(meta.isEmpty)

        val port = assertNotNull(meta.find("server.port"))
        assertEquals("java.lang.Integer", port.type)
        assertEquals("Server HTTP port.", port.description)
        assertEquals("8080", port.defaultValue)
        assertNull(port.deprecation)

        val old = assertNotNull(meta.find("old.setting")?.deprecation)
        assertEquals("error", old.level)
        assertEquals("new.setting", old.replacement)
        assertEquals("Renamed.", old.reason)
        assertEquals("warning", meta.find("plain.deprecated")?.deprecation?.level)

        val hint = assertNotNull(meta.hint("spring.main.banner-mode"))
        assertEquals(listOf("console", "off"), hint.values.map { it.value })
        assertEquals("Print to stdout.", hint.values[0].description)
        assertNull(hint.values[1].description)
    }

    @Test
    fun `merges jars and lets additional metadata override within a jar`() {
        val dir = tmp()
        val a = makeJar(
            dir, "a.jar",
            mapOf(
                "META-INF/spring-configuration-metadata.json" to
                    """{"properties":[{"name":"app.one","type":"java.lang.String","description":"from main"}]}""",
                "META-INF/additional-spring-configuration-metadata.json" to
                    """{"properties":[{"name":"app.one","type":"java.lang.String","description":"from additional"},
                        {"name":"app.two","type":"java.lang.Boolean"}]}""",
            ),
        )
        val b = makeJar(
            dir, "b.jar",
            mapOf(
                "META-INF/spring-configuration-metadata.json" to
                    """{"properties":[{"name":"app.one","description":"loser"},{"name":"other.key","type":"java.lang.String"}]}""",
            ),
        )
        val broken = makeJar(dir, "c.jar", mapOf("META-INF/spring-configuration-metadata.json" to "{not json"))
        val plain = makeJar(dir, "d.jar", mapOf("x.txt" to "hi"))
        val meta = SpringMetadata.fromJars(listOf(a, b, broken, plain, File(dir, "missing.jar")))
        assertEquals("from additional", meta.find("app.one")?.description)
        assertEquals(setOf("app.one", "app.two", "other.key"), meta.properties.map { it.name }.toSet())
        assertTrue(SpringMetadata.fromJars(listOf(plain)).isEmpty)
    }

    @Test
    fun `canonical keys ignore case separators indices and env var style`() {
        val canonical = canonicalKey("server.servlet.context-path")
        assertEquals("server.servlet.contextpath", canonical)
        assertEquals(canonical, canonicalKey("server.servlet.contextPath"))
        assertEquals(canonical, canonicalKey("server.servlet.context_path"))
        assertEquals(canonical, canonicalKey("SERVER_SERVLET_CONTEXTPATH"))
        assertEquals("a.b.c", canonicalKey("a.b[0].c"))
        assertEquals("a.b", canonicalKey("a[x].b"))
    }

    @Test
    fun `find uses relaxed binding`() {
        val meta = SpringMetadata.fromJars(listOf(mainJar()))
        assertEquals("server.servlet.context-path", meta.find("server.servlet.contextPath")?.name)
        assertEquals("server.servlet.context-path", meta.find("SERVER_SERVLET_CONTEXTPATH")?.name)
        assertEquals("spring.jpa.show-sql", meta.find("spring.jpa.showSql")?.name)
        assertNull(meta.find("server.servlet.nope"))
    }

    @Test
    fun `map typed properties accept any sub key`() {
        val meta = SpringMetadata.fromJars(listOf(mainJar()))
        assertTrue(meta.find("spring.jpa.properties")!!.isMap)
        assertTrue(meta.isMapKey("spring.jpa.properties.hibernate.dialect"))
        assertTrue(meta.isMapKey("spring.jpa.properties[hibernate.x]"))
        assertFalse(meta.isMapKey("spring.jpa.properties"))
        assertFalse(meta.isMapKey("spring.jpa.show-sql.x"))
        assertFalse(meta.isMapKey("server.port"))
    }
}
