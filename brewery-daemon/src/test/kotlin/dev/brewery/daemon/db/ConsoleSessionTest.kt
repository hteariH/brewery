package dev.brewery.daemon.db

import java.nio.file.Files
import java.nio.file.Path
import java.sql.DriverManager
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNotSame

/** Query consoles run in sessions of their own (`sessionId`), apart from each other and the shared session. */
class ConsoleSessionTest {
    private val root: Path = Files.createTempDirectory("brewery-console-session")
    private val service = DbService(root, InMemorySecretStore())
    private val db = Files.createTempFile("brewery-console-session", ".db")
    private val id: String

    init {
        DriverManager.getConnection("jdbc:sqlite:$db").use { c ->
            c.createStatement().use { st ->
                st.executeUpdate("CREATE TABLE t (id INTEGER PRIMARY KEY)")
                st.executeUpdate("INSERT INTO t VALUES (1), (2), (3)")
            }
        }
        id = service.save(ConnectionInfo("", "lite", "sqlite", "jdbc:sqlite:$db", null, false, emptyMap()), null).id
    }

    @AfterTest
    fun tearDown() = service.close()

    private fun code(block: () -> Unit): Int = assertFailsWith<DbException> { block() }.code

    @Test
    fun `each console session has its own connection and transaction`() {
        service.execute(id, "SELECT 1", "a")
        service.execute(id, "SELECT 1", "b")
        assertEquals(2, service.sessions.openCount)
        assertNotSame(
            service.sessions.session(id, "a").currentConnection,
            service.sessions.session(id, "b").currentConnection,
        )

        assertEquals(true, service.execute(id, "BEGIN", "a").inTransaction)
        assertEquals(false, service.execute(id, "SELECT 1", "b").inTransaction, "b has no transaction")
        assertEquals(false, service.execute(id, "ROLLBACK", "a").inTransaction)
    }

    @Test
    fun `closing a console session ends only its results`() {
        val a = service.execute(id, "SELECT id FROM t ORDER BY id", "a")
        val b = service.execute(id, "SELECT id FROM t ORDER BY id", "b")
        assertEquals(1, service.fetch(a.cursorId!!, 0, 1).rows.size)

        service.closeSession(id, "a")
        assertEquals(-32013, code { service.fetch(a.cursorId!!, 1, 1) })
        assertEquals(listOf(listOf("1"), listOf("2")), service.fetch(b.cursorId!!, 0, 2).rows)
        assertEquals(1, service.sessions.openCount)
        // Closing a session that is gone already is fine.
        service.closeSession(id, "a")
    }

    @Test
    fun `cancel without a running statement creates no session`() {
        service.cancel(id, null, "nobody")
        assertEquals(0, service.sessions.openCount)
        assertEquals(-32013, code { service.cancel("missing", null, "a") })
    }

    @Test
    fun `saving or deleting the data source closes every session of it`() {
        val other = service.save(ConnectionInfo("", "other", "sqlite", "jdbc:sqlite:$db", null, false, emptyMap()), null).id
        service.execute(id, "SELECT 1", "a")
        service.execute(id, "SELECT 1", "b")
        service.execute(id, "SELECT 1")
        service.execute(other, "SELECT 1", "a")
        assertEquals(4, service.sessions.openCount)

        val saved = service.list().single { it.id == id }
        service.save(saved, null)
        assertEquals(1, service.sessions.openCount, "only the other data source's session is left")
        service.delete(other)
        assertEquals(0, service.sessions.openCount)
    }
}
