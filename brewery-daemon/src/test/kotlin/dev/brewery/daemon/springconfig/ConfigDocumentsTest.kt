package dev.brewery.daemon.springconfig

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

class PropertiesDocumentTest {
    private val doc = PropertiesDocument(
        listOf(
            "# comment", "server.port=8080", "", "spring.jpa.show-sql : true", "name value",
            "long=a${92.toChar()}", "b=c", "bare", "",
        ).joinToString("\n"),
    )

    @Test
    fun `lists every key with its line`() {
        assertEquals(
            listOf("server.port" to 1, "spring.jpa.show-sql" to 3, "name" to 4, "long" to 5, "bare" to 7),
            doc.entries.map { it.key to it.line },
        )
    }

    @Test
    fun `finds the key at a position`() {
        assertEquals("server.port", doc.keyAt(1, 3)?.key)
        assertEquals("server.port", doc.keyAt(1, 11)?.key) // caret right after the key
        assertNull(doc.keyAt(1, 12)) // in the value
        assertNull(doc.keyAt(2, 0))
    }

    @Test
    fun `reads the value and its position`() {
        val e = assertNotNull(doc.entryOnLine(3))
        assertEquals("true", e.value)
        assertEquals(22, e.valueStart)
    }

    @Test
    fun `classifies the completion context`() {
        assertEquals(ConfigContext.Key(emptyList(), 0, 0), doc.contextAt(2, 0))
        assertEquals(ConfigContext.Key(emptyList(), 0, 11), doc.contextAt(1, 6))
        assertEquals(ConfigContext.Value("server.port", 12, 16), doc.contextAt(1, 12))
        assertNull(doc.contextAt(0, 3))
        val open = PropertiesDocument("server.port=")
        assertEquals(ConfigContext.Value("server.port", 12, 12), open.contextAt(0, 12))
    }
}

class YamlDocumentTest {
    private val text = """
        server:
          port: 8080
          servlet:
            context-path: /app
        spring:
          jpa:
            show-sql: true
        items:
          - name: a
            size: 1
        ---
        other: x # note
        script: |
          not: a key
    """.trimIndent()
    private val doc = YamlDocument(text)

    @Test
    fun `lists all key paths with ranges`() {
        val paths = doc.entries.map { it.key }
        assertEquals(
            listOf(
                "server", "server.port", "server.servlet", "server.servlet.context-path", "spring", "spring.jpa",
                "spring.jpa.show-sql", "items", "items.name", "items.size", "other", "script",
            ),
            paths,
        )
        val ctx = doc.entries.first { it.key == "server.servlet.context-path" }
        assertEquals(3, ctx.line)
        assertEquals(4, ctx.keyStart)
        assertEquals(16, ctx.keyEnd)
        assertEquals("/app", ctx.value)
        assertEquals("x", doc.entries.first { it.key == "other" }.value)
    }

    @Test
    fun `finds the key path at a position`() {
        assertEquals("spring.jpa.show-sql", doc.keyAt(6, 8)?.key)
        assertNull(doc.keyAt(6, 25))
        assertEquals("items.size", doc.keyAt(9, 5)?.key)
    }

    @Test
    fun `documents are separated and do not share parents`() {
        assertEquals(listOf(0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1), doc.entries.map { it.document })
        assertEquals(emptyList(), doc.parentPath(11, 0))
    }

    @Test
    fun `parent path follows indentation`() {
        assertEquals(listOf("spring", "jpa"), doc.parentPath(7, 4).also { assertTrue(it.isNotEmpty()) })
        assertEquals(listOf("server"), YamlDocument("server:\n  \n").parentPath(1, 2))
        assertEquals(emptyList(), YamlDocument("server:\n  port: 1\n\n").parentPath(2, 0))
    }

    @Test
    fun `classifies the completion context`() {
        val d = YamlDocument("server:\n  po\nspring:\n  jpa:\n    show-sql: \n")
        assertEquals(ConfigContext.Key(listOf("server"), 2, 4), d.contextAt(1, 4))
        assertEquals(ConfigContext.Value("spring.jpa.show-sql", 14, 14), d.contextAt(4, 14))
        assertEquals(ConfigContext.Key(emptyList(), 0, 0), YamlDocument("\n").contextAt(0, 0))
    }
}
