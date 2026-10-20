package dev.brewery.daemon.db

import java.nio.file.Files
import java.sql.DriverManager
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

/** Every unfinished cursor, not only the PostgreSQL streaming ones, ends after the inactivity TTL. */
class CursorTtlTest {
    private val db = Files.createTempFile("brewery-ttl", ".db")
    private var now = 0L
    private val service = DbService(Files.createTempDirectory("brewery-ttl"), InMemorySecretStore(), clock = { now }, streamingTtlMs = 60_000)
    private val id: String

    init {
        DriverManager.getConnection("jdbc:sqlite:$db").use { c ->
            c.createStatement().use { st ->
                st.executeUpdate("CREATE TABLE t (id INTEGER NOT NULL PRIMARY KEY)")
                st.executeUpdate("INSERT INTO t VALUES (1), (2), (3), (4), (5)")
            }
        }
        id = service.save(ConnectionInfo("", "lite", "sqlite", "jdbc:sqlite:$db", null, false, emptyMap()), null).id
    }

    @AfterTest
    fun tearDown() = service.close()

    /** A write from another connection that does not wait for locks. */
    private fun writeElsewhere() = DriverManager.getConnection("jdbc:sqlite:$db").use { c ->
        c.createStatement().use { st ->
            st.execute("PRAGMA busy_timeout = 0")
            st.executeUpdate("INSERT INTO t VALUES (100 + (SELECT count(*) FROM t))")
        }
    }

    @Test
    fun `an abandoned SQLite cursor releases its lock after the TTL`() {
        val abandoned = service.execute(id, "SELECT id FROM t").cursorId!!
        assertEquals(1, service.fetch(abandoned, 0, 1).rows.size)
        val active = service.execute(id, "SELECT id FROM t").cursorId!!
        assertEquals(1, service.fetch(active, 0, 1).rows.size)
        now = 30_000
        service.sessions.sweep()
        assertEquals(1, service.fetch(active, 1, 1).rows.size)
        now = 60_000
        service.sessions.sweep()
        // Idle for 60 s: gone. The active one was used 30 s ago.
        assertEquals(-32013, assertFailsWith<DbException> { service.fetch(abandoned, 1, 1) }.code)
        assertEquals(1, service.fetch(active, 2, 1).rows.size)
        now = 120_000
        service.sessions.sweep()
        assertEquals(-32013, assertFailsWith<DbException> { service.fetch(active, 3, 1) }.code)
        // Nothing holds the SHARED lock any more.
        writeElsewhere()
        assertEquals(6, service.fetch(service.execute(id, "SELECT id FROM t").cursorId!!, 0, 10).rows.size)
    }

    @Test
    fun `by default an abandoned SQLite cursor still ends after a minute`() {
        val defaults = DbService(Files.createTempDirectory("brewery-ttl-default"), InMemorySecretStore(), clock = { now })
        try {
            val lite = defaults.save(ConnectionInfo("", "lite", "sqlite", "jdbc:sqlite:$db", null, false, emptyMap()), null).id
            val abandoned = defaults.execute(lite, "SELECT id FROM t").cursorId!!
            assertEquals(1, defaults.fetch(abandoned, 0, 1).rows.size)
            now = 60_000
            defaults.sessions.sweep()
            assertEquals(-32013, assertFailsWith<DbException> { defaults.fetch(abandoned, 1, 1) }.code)
        } finally {
            defaults.close()
        }
    }

    @Test
    fun `finished cursors are not expired`() {
        val finished = service.execute(id, "SELECT id FROM t").cursorId!!
        assertEquals(true, service.fetch(finished, 0, 10).done)
        now = 300_000
        service.sessions.sweep()
        assertEquals(true, service.fetch(finished, 5, 10).done)
    }
}
