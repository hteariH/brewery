package dev.brewery.daemon.db

import java.nio.file.Files
import java.sql.DriverManager
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

/** MariaDB's driver drains a pending streaming result into memory before any other command; so we end it first. */
class UnfinishedCursorTest {
    private val db = Files.createTempFile("brewery-unfinished", ".db")
    private val service = DbService(
        Files.createTempDirectory("brewery-unfinished"),
        InMemorySecretStore(),
        manageTransactions = { false },
        expireUnfinishedBeforeCommands = { true },
    )
    private val id: String

    init {
        DriverManager.getConnection("jdbc:sqlite:$db").use { c ->
            c.createStatement().use { st ->
                st.executeUpdate("CREATE TABLE t (id INTEGER NOT NULL PRIMARY KEY)")
                st.executeUpdate("INSERT INTO t VALUES (1), (2), (3)")
            }
        }
        id = service.save(ConnectionInfo("", "lite", "sqlite", "jdbc:sqlite:$db", null, false, emptyMap()), null).id
    }

    @AfterTest
    fun tearDown() = service.close()

    private fun code(block: () -> Unit) = assertFailsWith<DbException> { block() }.code

    @Test
    fun `another statement ends an unfinished cursor`() {
        val a = service.execute(id, "SELECT id FROM t").cursorId!!
        assertEquals(1, service.fetch(a, 0, 1).rows.size)
        val b = service.execute(id, "SELECT id FROM t").cursorId!!
        assertEquals(-32013, code { service.fetch(a, 1, 1) })
        assertEquals(3, service.fetch(b, 0, 10).rows.size)
    }

    @Test
    fun `introspection ends an unfinished cursor`() {
        val a = service.execute(id, "SELECT id FROM t").cursorId!!
        service.introspect(id, emptyList())
        assertEquals(-32013, code { service.fetch(a, 0, 1) })
    }

    @Test
    fun `finished cursors survive`() {
        val a = service.execute(id, "SELECT id FROM t").cursorId!!
        assertEquals(true, service.fetch(a, 0, 10).done)
        service.execute(id, "SELECT 1")
        service.introspect(id, emptyList())
        assertEquals(true, service.fetch(a, 3, 10).done)
    }
}
