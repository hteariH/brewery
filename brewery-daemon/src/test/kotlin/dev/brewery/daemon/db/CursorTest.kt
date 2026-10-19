package dev.brewery.daemon.db

import java.nio.file.Files
import java.nio.file.Path
import java.sql.DriverManager
import java.sql.Timestamp
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

class CursorTest {
    private val root: Path = Files.createTempDirectory("brewery-cursor")
    private val service = DbService(root, InMemorySecretStore())
    private val db = Files.createTempFile("brewery-cursor", ".db")
    private val id: String

    init {
        DriverManager.getConnection("jdbc:sqlite:$db").use { c ->
            c.createStatement().use { st ->
                st.executeUpdate("CREATE TABLE t (id INTEGER NOT NULL PRIMARY KEY, name TEXT, blob_col BLOB, ts TIMESTAMP, big TEXT)")
            }
            c.prepareStatement("INSERT INTO t VALUES (?, ?, ?, ?, ?)").use { ps ->
                for (i in 1..5) {
                    ps.setInt(1, i)
                    if (i == 5) ps.setNull(2, java.sql.Types.VARCHAR) else ps.setString(2, "n$i")
                    ps.setBytes(3, ByteArray(i) { (it + 1).toByte() })
                    ps.setTimestamp(4, Timestamp.valueOf("2024-03-05 10:20:3$i"))
                    ps.setString(5, "x".repeat(10_050))
                    ps.executeUpdate()
                }
            }
        }
        id = service.save(ConnectionInfo("", "lite", "sqlite", "jdbc:sqlite:$db", null, false, emptyMap()), null).id
    }

    @AfterTest
    fun tearDown() = service.close()

    private fun code(block: () -> Unit): Int = assertFailsWith<DbException> { block() }.code

    @Test
    fun `select returns columns and a cursor that pages with done`() {
        val r = service.execute(id, "SELECT id, name FROM t ORDER BY id")
        assertNotNull(r.cursorId)
        assertNull(r.updateCount)
        assertEquals(listOf("id", "name"), r.columns.map { it.name })
        assertTrue(r.elapsedMs >= 0)
        val p1 = service.fetch(r.cursorId!!, 0, 2)
        assertEquals(listOf(listOf("1", "n1"), listOf("2", "n2")), p1.rows)
        assertEquals(false, p1.done)
        val p2 = service.fetch(r.cursorId!!, 2, 2)
        assertEquals(listOf(listOf("3", "n3"), listOf("4", "n4")), p2.rows)
        assertEquals(false, p2.done)
        val p3 = service.fetch(r.cursorId!!, 4, 2)
        assertEquals(listOf(listOf("5", null)), p3.rows)
        assertEquals(true, p3.done)
        // A finished cursor stays known until closed and just answers empty.
        val p4 = service.fetch(r.cursorId!!, 5, 2)
        assertEquals(emptyList(), p4.rows)
        assertEquals(true, p4.done)
        assertEquals(false, p3.moreResults, "one statement has no further results")
    }

    @Test
    fun `an update of one statement has no further results`() {
        val r = service.execute(id, "UPDATE t SET name = name WHERE id = 1")
        assertEquals(1L, r.updateCount)
        assertEquals(false, r.moreResults)
    }

    @Test
    fun `done is exact when the last page is full`() {
        val r = service.execute(id, "SELECT id FROM t ORDER BY id")
        val p = service.fetch(r.cursorId!!, 0, 5)
        assertEquals(5, p.rows.size)
        assertEquals(true, p.done)
    }

    @Test
    fun `offset ahead of the position skips rows and behind is invalid params`() {
        val r = service.execute(id, "SELECT id FROM t ORDER BY id")
        assertEquals(listOf(listOf("3"), listOf("4")), service.fetch(r.cursorId!!, 2, 2).rows)
        assertEquals(-32602, code { service.fetch(r.cursorId!!, 0, 2) })
        assertEquals(-32602, code { service.fetch(r.cursorId!!, 3, 2) })
        assertEquals(listOf(listOf("5")), service.fetch(r.cursorId!!, 4, 2).rows)
        val beyond = service.execute(id, "SELECT id FROM t")
        val past = service.fetch(beyond.cursorId!!, 50, 10)
        assertEquals(emptyList(), past.rows)
        assertEquals(true, past.done)
    }

    @Test
    fun `invalid offset or limit and the limit cap`() {
        val r = service.execute(id, "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 2500) SELECT x FROM c")
        assertEquals(-32602, code { service.fetch(r.cursorId!!, -1, 10) })
        assertEquals(-32602, code { service.fetch(r.cursorId!!, 0, 0) })
        val page = service.fetch(r.cursorId!!, 0, 5000)
        assertEquals(1000, page.rows.size)
        assertEquals(false, page.done)
        assertEquals("1000", page.rows.last()[0])
    }

    @Test
    fun `update statements return the count and no cursor`() {
        val ins = service.execute(id, "INSERT INTO t (id, name) VALUES (6, 'six')")
        assertNull(ins.cursorId)
        assertEquals(1L, ins.updateCount)
        assertEquals(emptyList(), ins.columns)
        val upd = service.execute(id, "UPDATE t SET name = 'z' WHERE id > 3")
        assertEquals(3L, upd.updateCount)
        val ddl = service.execute(id, "CREATE TABLE u (a INTEGER)")
        assertNull(ddl.cursorId)
        // SQLite's own count would repeat the previous statement's 3; like PostgreSQL, DDL reports 0.
        assertEquals(0L, ddl.updateCount)
        assertEquals(2L, service.execute(id, "INSERT INTO u VALUES (1), (2)").updateCount)
        assertEquals(0L, service.execute(id, "ALTER TABLE u ADD COLUMN b TEXT").updateCount)
        assertEquals(0L, service.execute(id, "CREATE INDEX u_a ON u (a)").updateCount)
        assertEquals(0L, service.execute(id, "UPDATE u SET b = 'x' WHERE a > 5").updateCount)
        assertEquals(0L, service.execute(id, "DROP TABLE u").updateCount)
    }

    @Test
    fun `values are rendered as strings`() {
        val r = service.execute(id, "SELECT name, blob_col, ts, big FROM t WHERE id = 5")
        val row = service.fetch(r.cursorId!!, 0, 10).rows.single()
        assertNull(row[0])
        assertEquals("0x0102030405", row[1])
        assertEquals("2024-03-05T10:20:35", row[2])
        assertEquals("x".repeat(10_000) + "…", row[3])
    }

    @Test
    fun `query errors are DB_QUERY_FAILED and leave the session usable`() {
        val e = assertFailsWith<DbException> { service.execute(id, "SELECT * FROM missing_table") }
        assertEquals(-32012, e.code)
        assertTrue(e.message!!.contains("no such table"), e.message)
        assertEquals(1L, service.execute(id, "UPDATE t SET name = 'q' WHERE id = 1").updateCount)
    }

    @Test
    fun `unknown ids are DB_NOT_FOUND`() {
        assertEquals(-32013, code { service.execute("missing", "SELECT 1") })
        assertEquals(-32013, code { service.fetch("nope", 0, 10) })
        assertEquals(-32013, code { service.cancel("missing", null) })
        // Closing an unknown or already closed cursor is a no-op.
        service.closeCursor("nope")
    }

    @Test
    fun `close forgets the cursor`() {
        val r = service.execute(id, "SELECT id FROM t")
        service.closeCursor(r.cursorId!!)
        assertEquals(-32013, code { service.fetch(r.cursorId!!, 0, 1) })
        service.closeCursor(r.cursorId!!)
    }

    @Test
    fun `the 21st open cursor closes the oldest`() {
        val cursors = (1..21).map { service.execute(id, "SELECT $it AS v").cursorId!! }
        assertEquals(-32013, code { service.fetch(cursors[0], 0, 1) })
        assertEquals(listOf(listOf("2")), service.fetch(cursors[1], 0, 1).rows)
        assertEquals(listOf(listOf("21")), service.fetch(cursors[20], 0, 1).rows)
        // Another connection has its own budget.
        val other = service.save(ConnectionInfo("", "lite2", "sqlite", "jdbc:sqlite:$db", null, false, emptyMap()), null).id
        val first = service.execute(other, "SELECT 1").cursorId!!
        repeat(19) { service.execute(other, "SELECT 1") }
        assertEquals(listOf(listOf("1")), service.fetch(first, 0, 1).rows)
    }

    @Test
    fun `cursors die with their session`() {
        val r = service.execute(id, "SELECT id FROM t")
        // Saving the connection again closes its session.
        service.save(ConnectionInfo(id, "lite", "sqlite", "jdbc:sqlite:$db", null, false, emptyMap()), null)
        assertEquals(-32013, code { service.fetch(r.cursorId!!, 0, 1) })
        val again = service.execute(id, "SELECT id FROM t")
        assertEquals(5, service.fetch(again.cursorId!!, 0, 10).rows.size)
        service.delete(id)
        assertEquals(-32013, code { service.fetch(again.cursorId!!, 0, 1) })
    }

    @Test
    fun `cancel with nothing running does nothing`() {
        service.cancel(id, null)
        assertEquals(1L, service.execute(id, "UPDATE t SET name = 'q' WHERE id = 1").updateCount)
    }

    @Test
    fun `cancel stops a long running query and the next execute works`() {
        val failure = AtomicReference<Throwable?>()
        val finished = CountDownLatch(1)
        val worker = Thread {
            try {
                service.execute(
                    id,
                    "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 1000000000000) SELECT count(*) FROM c",
                )
            } catch (t: Throwable) {
                failure.set(t)
            } finally {
                finished.countDown()
            }
        }
        val started = System.nanoTime()
        worker.start()
        // Exactly one cancel, sent as soon as the statement is registered: the session re-sends it a few times
        // itself, because an interrupt that lands before the engine started stepping would be lost.
        val deadline = started + 15_000_000_000L
        while (service.sessions.session(id).runningStatement == null && System.nanoTime() < deadline) Thread.sleep(1)
        service.cancel(id, null)
        finished.await(10, TimeUnit.SECONDS)
        assertEquals(0L, finished.count, "the query was not cancelled")
        val elapsedMs = (System.nanoTime() - started) / 1_000_000
        assertTrue(elapsedMs < 10_000, "cancel took $elapsedMs ms")
        val error = failure.get()
        assertTrue(error is DbException && error.code == -32012, "unexpected: $error")
        assertEquals(1L, service.execute(id, "UPDATE t SET name = 'after' WHERE id = 2").updateCount)
        val r = service.execute(id, "SELECT name FROM t WHERE id = 2")
        assertEquals(listOf(listOf("after")), service.fetch(r.cursorId!!, 0, 1).rows)
    }

    // --- streaming transactions (what PostgreSQL needs for fetchSize), exercised on SQLite with the policy forced on ---

    private fun streamingService(): Pair<DbService, String> {
        val svc = DbService(Files.createTempDirectory("brewery-stream"), InMemorySecretStore(), manageTransactions = { true })
        val sid = svc.save(ConnectionInfo("", "lite", "sqlite", "jdbc:sqlite:$db", null, false, emptyMap()), null).id
        return svc to sid
    }

    private fun autoCommit(svc: DbService, sid: String) = svc.sessions.run(sid) { it.autoCommit }

    @Test
    fun `a streaming cursor holds autocommit off until the last one is gone`() {
        val (svc, sid) = streamingService()
        try {
            assertTrue(autoCommit(svc, sid))
            val a = svc.execute(sid, "SELECT id FROM t").cursorId!!
            assertEquals(false, autoCommit(svc, sid))
            val b = svc.execute(sid, "SELECT id FROM t").cursorId!!
            svc.closeCursor(a)
            assertEquals(false, autoCommit(svc, sid))
            // Reading b to its end releases it, too.
            assertTrue(svc.fetch(b, 0, 100).done)
            assertTrue(autoCommit(svc, sid))
        } finally {
            svc.close()
        }
    }

    @Test
    fun `a closed cursor and evicted cursors restore autocommit`() {
        val (svc, sid) = streamingService()
        try {
            val a = svc.execute(sid, "SELECT id FROM t").cursorId!!
            svc.closeCursor(a)
            assertTrue(autoCommit(svc, sid))
            val cursors = (1..21).map { svc.execute(sid, "SELECT $it").cursorId!! }
            assertEquals(false, autoCommit(svc, sid))
            cursors.drop(1).forEach { svc.closeCursor(it) }
            assertTrue(autoCommit(svc, sid))
        } finally {
            svc.close()
        }
    }

    @Test
    fun `an update commits and stays autocommit`() {
        val (svc, sid) = streamingService()
        try {
            assertEquals(1L, svc.execute(sid, "UPDATE t SET name = 'committed' WHERE id = 1").updateCount)
            assertTrue(autoCommit(svc, sid))
            DriverManager.getConnection("jdbc:sqlite:$db").use { other ->
                other.createStatement().use { st ->
                    st.executeQuery("SELECT name FROM t WHERE id = 1").use { rs ->
                        rs.next()
                        assertEquals("committed", rs.getString(1))
                    }
                }
            }
        } finally {
            svc.close()
        }
    }

    @Test
    fun `an update while a cursor streams commits and drops the streaming cursors`() {
        val (svc, sid) = streamingService()
        try {
            val a = svc.execute(sid, "SELECT id FROM t").cursorId!!
            assertEquals(1L, svc.execute(sid, "UPDATE t SET name = 'x' WHERE id = 2").updateCount)
            assertTrue(autoCommit(svc, sid))
            assertEquals(-32013, assertFailsWith<DbException> { svc.fetch(a, 0, 1) }.code)
            // The session is still fine.
            assertEquals(5, svc.fetch(svc.execute(sid, "SELECT id FROM t").cursorId!!, 0, 10).rows.size)
        } finally {
            svc.close()
        }
    }

    @Test
    fun `a failing statement rolls back and keeps other cursors alive`() {
        val (svc, sid) = streamingService()
        try {
            assertEquals(-32012, assertFailsWith<DbException> { svc.execute(sid, "SELECT * FROM missing_table") }.code)
            assertTrue(autoCommit(svc, sid))
            val a = svc.execute(sid, "SELECT id FROM t ORDER BY id").cursorId!!
            assertEquals(listOf(listOf("1")), svc.fetch(a, 0, 1).rows)
            assertEquals(-32012, assertFailsWith<DbException> { svc.execute(sid, "SELECT * FROM missing_table") }.code)
            assertEquals(false, autoCommit(svc, sid))
            assertEquals(listOf(listOf("2")), svc.fetch(a, 1, 1).rows)
        } finally {
            svc.close()
        }
    }
}
