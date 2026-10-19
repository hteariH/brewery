package dev.brewery.daemon.db

import java.lang.reflect.InvocationTargetException
import java.lang.reflect.Proxy
import java.nio.file.Files
import java.sql.Connection
import java.sql.DriverManager
import java.sql.SQLException
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference
import org.postgresql.core.BaseConnection
import org.postgresql.core.TransactionState
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNotNull
import kotlin.test.assertTrue

/** The implicit streaming transaction (PostgreSQL policy forced on over SQLite) and user-controlled transactions. */
class StreamingTransactionTest {
    private val db = Files.createTempFile("brewery-stream-tx", ".db")
    private var now = 0L
    private val service = DbService(
        Files.createTempDirectory("brewery-stream-tx"),
        InMemorySecretStore(),
        clock = { now },
        manageTransactions = { true },
        streamingTtlMs = 60_000,
    )
    private val id: String
    private val toClose = mutableListOf<Session>()

    init {
        DriverManager.getConnection("jdbc:sqlite:$db").use { c ->
            c.createStatement().use { st ->
                st.executeUpdate("CREATE TABLE t (id INTEGER NOT NULL PRIMARY KEY, name TEXT)")
                st.executeUpdate("INSERT INTO t VALUES (1, 'n1'), (2, 'n2'), (3, 'n3')")
            }
        }
        id = service.save(ConnectionInfo("", "lite", "sqlite", "jdbc:sqlite:$db", null, false, emptyMap()), null).id
    }

    @AfterTest
    fun tearDown() {
        service.close()
        toClose.forEach { it.close() }
    }

    private fun autoCommit() = service.sessions.run(id) { it.autoCommit }
    private fun name(rowId: Int): String? {
        val r = service.execute(id, "SELECT name FROM t WHERE id = $rowId")
        return service.fetch(r.cursorId!!, 0, 1).rows.single()[0].also { service.closeCursor(r.cursorId!!) }
    }

    @Test
    fun `BEGIN UPDATE ROLLBACK really rolls back`() {
        service.execute(id, "BEGIN")
        assertEquals(1L, service.execute(id, "UPDATE t SET name = 'rolled' WHERE id = 1").updateCount)
        assertEquals("rolled", name(1))
        service.execute(id, "ROLLBACK")
        assertEquals("n1", name(1))
        assertTrue(autoCommit())
    }

    @Test
    fun `BEGIN UPDATE COMMIT keeps the change and reads inside a user transaction do not touch autocommit`() {
        service.execute(id, "BEGIN")
        service.execute(id, "UPDATE t SET name = 'kept' WHERE id = 2")
        val cursor = service.execute(id, "SELECT id FROM t").cursorId!!
        assertTrue(autoCommit())
        assertEquals(3, service.fetch(cursor, 0, 10).rows.size)
        service.execute(id, "COMMIT")
        assertEquals("kept", name(2))
        // After the user transaction the implicit streaming works again.
        service.execute(id, "SELECT id FROM t")
        assertEquals(false, autoCommit())
    }

    @Test
    fun `writes that return rows are not streamed in an implicit transaction`() {
        val r = service.execute(id, "UPDATE t SET name = 'r' WHERE id = 3 RETURNING id")
        assertNotNull(r.cursorId)
        assertTrue(autoCommit())
        assertEquals(listOf(listOf("3")), service.fetch(r.cursorId!!, 0, 5).rows)
    }

    @Test
    fun `buffered results are capped and say so`() {
        val capped = DbService(
            Files.createTempDirectory("brewery-capped"),
            InMemorySecretStore(),
            manageTransactions = { true },
            maxBufferedRows = 2,
        )
        try {
            val cid = capped.save(ConnectionInfo("", "lite", "sqlite", "jdbc:sqlite:$db", null, false, emptyMap()), null).id
            // Streamed: no cap.
            val streamed = capped.fetch(capped.execute(cid, "SELECT id FROM t").cursorId!!, 0, 10)
            assertEquals(3, streamed.rows.size)
            assertEquals(false, streamed.truncated)
            // Buffered by the driver: DML ... RETURNING, and reads inside a user transaction.
            val returning = capped.fetch(capped.execute(cid, "UPDATE t SET name = name RETURNING id").cursorId!!, 0, 10)
            assertEquals(2, returning.rows.size)
            assertTrue(returning.done && returning.truncated)
            capped.execute(cid, "BEGIN")
            val inTx = capped.execute(cid, "SELECT id FROM t ORDER BY id").cursorId!!
            val first = capped.fetch(inTx, 0, 1)
            assertEquals(false, first.done || first.truncated)
            val rest = capped.fetch(inTx, 1, 10)
            assertEquals(listOf(listOf("2")), rest.rows)
            assertTrue(rest.done && rest.truncated)
            // Exactly the cap is not a truncation.
            val exact = capped.fetch(capped.execute(cid, "SELECT id FROM t WHERE id < 3").cursorId!!, 0, 10)
            assertEquals(2, exact.rows.size)
            assertTrue(exact.done && !exact.truncated)
            capped.execute(cid, "ROLLBACK")
        } finally {
            capped.close()
        }
    }

    @Test
    fun `an implicit streaming transaction expires after inactivity`() {
        val cursor = service.execute(id, "SELECT id FROM t").cursorId!!
        now = 30_000
        service.sessions.sweep()
        assertEquals(false, autoCommit())
        // A fetch counts as activity.
        service.fetch(cursor, 0, 1)
        now = 80_000
        service.sessions.sweep()
        assertEquals(false, autoCommit())
        now = 95_000
        service.sessions.sweep()
        assertTrue(autoCommit())
        assertEquals(-32013, assertFailsWith<DbException> { service.fetch(cursor, 1, 1) }.code)
        assertEquals(3, service.fetch(service.execute(id, "SELECT id FROM t").cursorId!!, 0, 10).rows.size)
    }

    @Test
    fun `by default a streaming result survives minutes of reading its first page`() {
        val defaults = DbService(
            Files.createTempDirectory("brewery-stream-ttl"),
            InMemorySecretStore(),
            clock = { now },
            manageTransactions = { true },
        )
        try {
            val lite = defaults.save(ConnectionInfo("", "lite", "sqlite", "jdbc:sqlite:$db", null, false, emptyMap()), null).id
            val cursor = defaults.execute(lite, "SELECT id FROM t").cursorId!!
            assertEquals(1, defaults.fetch(cursor, 0, 1).rows.size)
            now = 4 * 60_000L
            defaults.sessions.sweep()
            assertEquals(1, defaults.fetch(cursor, 1, 1).rows.size, "still there after four minutes")
            now += 5 * 60_000L
            defaults.sessions.sweep()
            assertEquals(-32013, assertFailsWith<DbException> { defaults.fetch(cursor, 2, 1) }.code)
        } finally {
            defaults.close()
        }
    }

    @Test
    fun `a failing fetch rolls back and drops every streaming cursor`() {
        val a = service.execute(id, "SELECT id FROM t").cursorId!!
        val bad = service.execute(id, "SELECT 1 AS v UNION ALL SELECT abs(-9223372036854775808)").cursorId!!
        assertEquals(-32012, assertFailsWith<DbException> { service.fetch(bad, 0, 1) }.code)
        assertTrue(autoCommit())
        assertEquals(-32013, assertFailsWith<DbException> { service.fetch(a, 0, 1) }.code)
        assertEquals(-32013, assertFailsWith<DbException> { service.fetch(bad, 0, 1) }.code)
        assertEquals(3, service.fetch(service.execute(id, "SELECT id FROM t").cursorId!!, 0, 10).rows.size)
    }

    @Test
    fun `an unexpected failure while starting a read never leaves an ownerless transaction`() {
        val fail = AtomicBoolean()
        val session = Session("s", { flaky(DriverManager.getConnection("jdbc:sqlite:$db"), fail) }, { now }).also(toClose::add)
        val manager = CursorManager(session, ConcurrentHashMap(), manageTransactions = { true })
        fail.set(true)
        assertFailsWith<IllegalStateException> { session.run { manager.execute(it, "SELECT id FROM t") } }
        assertTrue(session.run { it.autoCommit })
        // The next statement is an ordinary implicit transaction again and it ends properly.
        val r = session.run { manager.execute(it, "SELECT id FROM t") }
        assertEquals(false, session.run { it.autoCommit })
        session.run { manager.close(r.cursorId!!) }
        assertTrue(session.run { it.autoCommit })
    }

    @Test
    fun `a dropped connection forgets its streaming cursors and the next connection starts clean`() {
        val index = ConcurrentHashMap<String, CursorManager>()
        val session = Session("s", { DriverManager.getConnection("jdbc:sqlite:$db") }, { now }).also(toClose::add)
        val manager = CursorManager(session, index, manageTransactions = { true })
        val r = session.run { manager.execute(it, "SELECT id FROM t") }
        assertEquals(1, index.size)
        assertFailsWith<SQLException> { session.run<Unit> { throw SQLException("link lost", "08006") } }
        assertEquals(0, manager.openCount)
        assertEquals(0, index.size)
        assertTrue(session.run { it.autoCommit })
        val again = session.run { manager.execute(it, "SELECT id FROM t") }
        assertEquals(3, session.run { manager.fetch(again.cursorId!!, 0, 10) }.rows.size)
        assertTrue(session.run { it.autoCommit })
        assertEquals(null, r.updateCount)
    }

    /** SQLite that reports [state] as its server transaction state, the way pgjdbc's [BaseConnection] does. */
    private fun reporting(real: Connection, state: AtomicReference<TransactionState>): Connection {
        lateinit var proxy: Connection
        proxy = Proxy.newProxyInstance(javaClass.classLoader, arrayOf(BaseConnection::class.java)) { _, method, args ->
            when (method.name) {
                "getTransactionState" -> state.get()
                "isWrapperFor" -> (args!![0] as Class<*>).isInstance(proxy) || real.isWrapperFor(args[0] as Class<*>)
                "unwrap" -> if ((args!![0] as Class<*>).isInstance(proxy)) proxy else real.unwrap(args[0] as Class<*>)
                else -> try {
                    method.invoke(real, *(args ?: emptyArray()))
                } catch (e: InvocationTargetException) {
                    throw e.targetException
                }
            }
        } as Connection
        return proxy
    }

    @Test
    fun `the server transaction state wins over the statement keywords`() {
        val state = AtomicReference(TransactionState.IDLE)
        val session = Session("s", { reporting(DriverManager.getConnection("jdbc:sqlite:$db"), state) }, { now }).also(toClose::add)
        val manager = CursorManager(session, ConcurrentHashMap(), manageTransactions = { true })
        // COMMIT AND CHAIN, a CALL or a BEGIN inside a script: the keywords say "no transaction", the server says open.
        state.set(TransactionState.OPEN)
        session.run { manager.execute(it, "UPDATE t SET name = name WHERE id = 1") }
        val inUserTx = session.run { manager.execute(it, "SELECT id FROM t") }
        assertTrue(session.run { it.autoCommit }, "a read inside the user's transaction must not be wrapped")
        session.run { manager.close(inUserTx.cursorId!!) }
        // The keywords say "BEGIN", but the server is idle (e.g. the BEGIN failed): reads stream again.
        state.set(TransactionState.IDLE)
        session.run { manager.execute(it, "UPDATE t SET name = name WHERE id = 1") }
        val streamed = session.run { manager.execute(it, "SELECT id FROM t") }
        assertEquals(false, session.run { it.autoCommit })
        session.run { manager.close(streamed.cursorId!!) }
        assertTrue(session.run { it.autoCommit })
        // A failed transaction is still the user's to end.
        state.set(TransactionState.FAILED)
        session.run { manager.execute(it, "UPDATE t SET name = name WHERE id = 1") }
        session.run { manager.execute(it, "SELECT id FROM t") }
        assertTrue(session.run { it.autoCommit })
    }

    private fun flaky(real: Connection, failNextCreate: AtomicBoolean): Connection =
        Proxy.newProxyInstance(javaClass.classLoader, arrayOf(Connection::class.java)) { _, method, args ->
            if (method.name == "createStatement" && failNextCreate.getAndSet(false)) throw IllegalStateException("injected")
            try {
                method.invoke(real, *(args ?: emptyArray()))
            } catch (e: InvocationTargetException) {
                throw e.targetException
            }
        } as Connection
}
