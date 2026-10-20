package dev.brewery.daemon.db

import java.nio.file.Files
import java.nio.file.Path
import java.sql.DriverManager
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals

class TransactionStateTest {
    private val root: Path = Files.createTempDirectory("brewery-tx")
    private var now = 1_000_000L
    private val service = DbService(root, InMemorySecretStore(), clock = { now }, idleTimeoutMs = 1_000)
    private val db = Files.createTempFile("brewery-tx", ".db")
    private val id: String

    init {
        DriverManager.getConnection("jdbc:sqlite:$db").use { c ->
            c.createStatement().use { it.executeUpdate("CREATE TABLE t (id INTEGER PRIMARY KEY)") }
        }
        id = service.save(ConnectionInfo("", "lite", "sqlite", "jdbc:sqlite:$db", null, false, emptyMap()), null).id
    }

    @AfterTest
    fun tearDown() = service.close()

    @Test
    fun `execute reports a user transaction from BEGIN until COMMIT or ROLLBACK`() {
        assertEquals(false, service.execute(id, "INSERT INTO t VALUES (1)").inTransaction)
        assertEquals(true, service.execute(id, "BEGIN").inTransaction)
        assertEquals(true, service.execute(id, "INSERT INTO t VALUES (2)").inTransaction)
        val read = service.execute(id, "SELECT id FROM t")
        assertEquals(true, read.inTransaction)
        service.closeCursor(read.cursorId!!)
        assertEquals(false, service.execute(id, "COMMIT").inTransaction)
        assertEquals(true, service.execute(id, "BEGIN TRANSACTION").inTransaction)
        assertEquals(false, service.execute(id, "ROLLBACK").inTransaction)
    }

    @Test
    fun `an idle session with an open transaction is not closed`() {
        service.execute(id, "BEGIN")
        service.execute(id, "INSERT INTO t VALUES (3)")
        now += 60_000
        service.sessions.sweep()
        assertEquals(1, service.sessions.openCount, "closing it would roll the insert back")

        service.execute(id, "COMMIT")
        now += 60_000
        service.sessions.sweep()
        assertEquals(0, service.sessions.openCount)
        val rows = service.execute(id, "SELECT id FROM t WHERE id = 3")
        assertEquals(listOf(listOf("3")), service.fetch(rows.cursorId!!, 0, 10).rows)
    }
}
