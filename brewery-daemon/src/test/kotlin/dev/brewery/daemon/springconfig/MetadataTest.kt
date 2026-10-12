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

/** Property shapes that used to produce false "unknown property" warnings. */
const val SHAPES_METADATA = """
{
  "properties": [
    {"name": "spring.cloud.gateway.routes", "type": "java.util.List<org.springframework.cloud.gateway.route.RouteDefinition>"},
    {"name": "spring.datasource.hikari.data-source-properties", "type": "java.util.Properties"},
    {"name": "spring.datasource.hikari.maximum-pool-size", "type": "java.lang.Integer"},
    {"name": "spring.profiles.active", "type": "java.util.List<java.lang.String>"},
    {"name": "app.headers", "type": "org.springframework.util.MultiValueMap<java.lang.String,java.lang.String>"},
    {"name": "app.cache", "type": "java.util.concurrent.ConcurrentHashMap<java.lang.String,java.lang.Integer>"},
    {"name": "app.tags", "type": "java.lang.String[]"},
    {"name": "app.pojo", "type": "com.example.Settings"},
    {"name": "app.described", "type": "com.example.Described"},
    {"name": "app.described.name", "type": "java.lang.String"}
  ]
}
"""

/** Writes [text] as `META-INF/<file>` below [classesDir], the way a build leaves it in its outputs. */
fun writeMetadataDir(classesDir: File, text: String, file: String = "additional-spring-configuration-metadata.json"): File {
    val meta = File(classesDir, "META-INF").apply { mkdirs() }
    File(meta, file).writeText(text)
    return classesDir
}

class MetadataTest {
    private fun tmp(): File = Files.createTempDirectory("meta").toFile()

    private fun mainJar(): File =
        makeJar(tmp(), "a.jar", mapOf("META-INF/spring-configuration-metadata.json" to MAIN_METADATA))

    @Test
    fun `parses properties hints and deprecation`() {
        val meta = SpringMetadata.fromClasspath(listOf(mainJar()))
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
        val meta = SpringMetadata.fromClasspath(listOf(a, b, broken, plain, File(dir, "missing.jar")))
        assertEquals("from additional", meta.find("app.one")?.description)
        assertEquals(setOf("app.one", "app.two", "other.key"), meta.properties.map { it.name }.toSet())
        assertTrue(SpringMetadata.fromClasspath(listOf(plain)).isEmpty)
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
        val meta = SpringMetadata.fromClasspath(listOf(mainJar()))
        assertEquals("server.servlet.context-path", meta.find("server.servlet.contextPath")?.name)
        assertEquals("server.servlet.context-path", meta.find("SERVER_SERVLET_CONTEXTPATH")?.name)
        assertEquals("spring.jpa.show-sql", meta.find("spring.jpa.showSql")?.name)
        assertNull(meta.find("server.servlet.nope"))
    }

    @Test
    fun `map typed properties accept any sub key`() {
        val meta = SpringMetadata.fromClasspath(listOf(mainJar()))
        assertTrue(meta.find("spring.jpa.properties")!!.isMap)
        assertTrue(meta.isMapKey("spring.jpa.properties.hibernate.dialect"))
        assertTrue(meta.isMapKey("spring.jpa.properties[hibernate.x]"))
        assertFalse(meta.isMapKey("spring.jpa.properties"))
        assertFalse(meta.isMapKey("spring.jpa.show-sql.x"))
        assertFalse(meta.isMapKey("server.port"))
    }

    @Test
    fun `reads project metadata directories and prefers them over jars`() {
        val project = writeMetadataDir(
            File(tmp(), "classes"),
            """{"properties":[{"name":"app.custom","type":"java.lang.String","description":"project"},
                {"name":"server.port","type":"java.lang.Integer","description":"overridden"}]}""",
        )
        val meta = SpringMetadata.fromClasspath(listOf(project, mainJar()))
        assertEquals("project", meta.find("app.custom")?.description)
        assertEquals("overridden", meta.find("server.port")?.description)
        assertNotNull(meta.find("spring.jpa.show-sql"))
    }

    @Test
    fun `project metadata alone is treated as no metadata`() {
        // Without any library metadata the classpath was not resolved, so every Spring key would look unknown.
        val project = writeMetadataDir(File(tmp(), "classes"), """{"properties":[{"name":"app.custom"}]}""")
        assertTrue(SpringMetadata.fromClasspath(listOf(project)).isEmpty)
    }

    @Test
    fun `map types are recognised by their simple name`() {
        fun map(type: String) = ConfigProperty("x", type, null, null, null).isMap
        assertTrue(map("java.util.Map<java.lang.String,java.lang.String>"))
        assertTrue(map("java.util.Properties"))
        assertTrue(map("org.springframework.util.MultiValueMap<java.lang.String,java.lang.String>"))
        assertTrue(map("java.util.concurrent.ConcurrentHashMap<java.lang.String,java.lang.Integer>"))
        assertTrue(map("com.example.Outer${'$'}SettingsMap"))
        assertFalse(map("org.springframework.boot.autoconfigure.web.ServerProperties"))
        assertFalse(map("java.lang.String"))
        assertFalse(map("java.util.List<java.util.Map<java.lang.String,java.lang.String>>"))
    }

    @Test
    fun `keys that cannot be verified are never unknown`() {
        val shapes = makeJar(tmp(), "shapes.jar", mapOf("META-INF/spring-configuration-metadata.json" to SHAPES_METADATA))
        val meta = SpringMetadata.fromClasspath(listOf(mainJar(), shapes))
        // Below a list of objects, a map or an object type without nested metadata.
        assertFalse(meta.isUnknown("spring.cloud.gateway.routes[0].id"))
        assertFalse(meta.isUnknown("spring.cloud.gateway.routes.id")) // the YAML `- id:` form
        assertFalse(meta.isUnknown("spring.cloud.gateway.routes[0].predicates[0]"))
        assertFalse(meta.isUnknown("spring.datasource.hikari.data-source-properties.cachePrepStmts"))
        assertFalse(meta.isUnknown("app.headers.x-trace"))
        assertFalse(meta.isUnknown("app.cache.users"))
        assertFalse(meta.isUnknown("app.pojo.anything.deep"))
        // Group keys.
        assertFalse(meta.isUnknown("server"))
        assertFalse(meta.isUnknown("spring.datasource.hikari"))
        assertFalse(meta.isUnknown("spring.datasource"))
        // Known properties.
        assertFalse(meta.isUnknown("server.port"))
        assertFalse(meta.isUnknown("spring.profiles.active[0]"))
        // Real typos and keys below scalars or types whose nested keys are known.
        assertTrue(meta.isUnknown("spring.datasource.hikari.maximum-pool-sise"))
        assertTrue(meta.isUnknown("server.prot"))
        assertTrue(meta.isUnknown("server.port.x"))
        assertTrue(meta.isUnknown("spring.profiles.active.x"))
        assertTrue(meta.isUnknown("app.tags.x"))
        assertTrue(meta.isUnknown("app.described.nmae"))
        assertTrue(meta.isUnknown("bogus"))
    }
}
