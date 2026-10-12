package dev.brewery.daemon.db

import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotEquals
import kotlin.test.assertTrue

class ConnectionStoreTest {
    private fun tempRoot(): Path = Files.createTempDirectory("brewery-db-store")

    private fun conn(id: String = "", name: String = "Local") = ConnectionInfo(
        id = id, name = name, driverId = "postgresql", url = "jdbc:postgresql://localhost/app",
        user = "app", hasPassword = true, options = mapOf("ssl" to "true", "app" to "x"),
    )

    @Test
    fun `save assigns an id and lists the connection`() {
        val store = ConnectionStore(tempRoot())
        val saved = store.saveConnection(conn())
        assertTrue(saved.id.isNotBlank())
        assertEquals(listOf(saved.id), store.connections().map { it.id })
    }

    @Test
    fun `round trip through a new store instance`() {
        val root = tempRoot()
        val saved = ConnectionStore(root).saveConnection(conn())
        val again = ConnectionStore(root).connections().single()
        assertEquals(saved.id, again.id)
        assertEquals("Local", again.name)
        assertEquals("app", again.user)
        assertEquals(mapOf("app" to "x", "ssl" to "true"), again.options)
    }

    @Test
    fun `save with an existing id replaces in place`() {
        val store = ConnectionStore(tempRoot())
        val a = store.saveConnection(conn(name = "A"))
        val b = store.saveConnection(conn(name = "B"))
        store.saveConnection(a.copy(name = "A2"))
        assertEquals(listOf("A2", "B"), store.connections().map { it.name })
        assertNotEquals(a.id, b.id)
    }

    @Test
    fun `delete removes and reports unknown ids`() {
        val store = ConnectionStore(tempRoot())
        val a = store.saveConnection(conn())
        assertTrue(store.deleteConnection(a.id))
        assertFalse(store.deleteConnection(a.id))
        assertTrue(store.connections().isEmpty())
    }

    @Test
    fun `file is stable pretty LF json without password data`() {
        val root = tempRoot()
        val store = ConnectionStore(root)
        store.saveConnection(conn(id = "c1"))
        val first = Files.readString(root.resolve(".brewery/db.json"))
        store.saveConnection(conn(id = "c1"))
        val second = Files.readString(root.resolve(".brewery/db.json"))
        assertEquals(first, second)
        assertFalse('\r' in first)
        assertTrue(first.endsWith("}\n"))
        assertTrue("\n  \"connections\"" in first)
        assertFalse("hasPassword" in first)
        assertFalse("assword" in first)
        // options are written in key order
        assertTrue(first.indexOf("\"app\"") < first.indexOf("\"ssl\""))
    }

    @Test
    fun `user drivers persist`() {
        val root = tempRoot()
        ConnectionStore(root).saveDriver(StoredDriver("h2", "H2", "org.h2.Driver", listOf("/x/h2.jar")))
        val d = ConnectionStore(root).drivers().single()
        assertEquals("org.h2.Driver", d.className)
        assertEquals(listOf("/x/h2.jar"), d.jarPaths)
    }

    @Test
    fun `corrupt file reads as empty instead of crashing`() {
        val root = tempRoot()
        Files.createDirectories(root.resolve(".brewery"))
        Files.writeString(root.resolve(".brewery/db.json"), "{not json")
        assertTrue(ConnectionStore(root).connections().isEmpty())
    }
}
