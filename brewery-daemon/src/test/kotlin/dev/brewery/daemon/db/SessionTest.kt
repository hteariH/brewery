package dev.brewery.daemon.db

import java.nio.file.Files
import java.sql.Connection
import java.sql.DriverManager
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertNotEquals
import kotlin.test.assertSame
import kotlin.test.assertTrue

class SessionTest {
    private val file = Files.createTempFile("brewery-session", ".db")
    private val opened = AtomicInteger()
    private var now = 1_000L
    private val known = mutableSetOf("a", "b")
    private val manager = SessionManager(
        exists = { it in known },
        opener = { opened.incrementAndGet(); DriverManager.getConnection("jdbc:sqlite:$file") },
        clock = { now },
        idleTimeoutMs = 600_000,
        sweepIntervalMs = 3_600_000,
    )

    @AfterTest
    fun tearDown() = manager.close()

    @Test
    fun `statements of one connection share one thread and one connection`() {
        val (thread1, conn1) = manager.run("a") { Thread.currentThread() to it }
        val (thread2, conn2) = manager.run("a") { Thread.currentThread() to it }
        assertSame(thread1, thread2)
        assertSame(conn1, conn2)
        assertNotEquals(Thread.currentThread(), thread1)
        assertEquals(1, opened.get())
        // A second connection id gets its own thread and connection.
        val (thread3, conn3) = manager.run("b") { Thread.currentThread() to it }
        assertNotEquals(thread1, thread3)
        assertNotEquals<Connection>(conn1, conn3)
    }

    @Test
    fun `the connection is opened lazily`() {
        manager.session("a")
        assertEquals(0, opened.get())
        manager.run("a") { 1 }
        assertEquals(1, opened.get())
    }

    @Test
    fun `calls from several threads are serialised on the session thread`() {
        val running = AtomicInteger()
        val maxRunning = AtomicInteger()
        val done = CountDownLatch(8)
        repeat(8) {
            Thread {
                manager.run("a") {
                    maxRunning.accumulateAndGet(running.incrementAndGet(), ::maxOf)
                    Thread.sleep(10)
                    running.decrementAndGet()
                }
                done.countDown()
            }.start()
        }
        assertTrue(done.await(10, TimeUnit.SECONDS))
        assertEquals(1, maxRunning.get())
    }

    @Test
    fun `unknown connection id is DB_NOT_FOUND and creates no session`() {
        val e = assertFailsWith<DbException> { manager.run("nope") { 1 } }
        assertEquals(DbErrorCodes.NOT_FOUND, e.code)
        assertEquals(0, manager.openCount)
    }

    @Test
    fun `idle sessions close after the timeout and reopen on demand`() {
        val first = manager.run("a") { it }
        now += 599_999
        manager.sweep()
        assertFalse(first.isClosed)
        assertEquals(1, manager.openCount)

        now += 1
        manager.sweep()
        assertTrue(first.isClosed)
        assertEquals(0, manager.openCount)

        val second = manager.run("a") { it }
        assertNotEquals<Connection>(first, second)
        assertEquals(2, opened.get())
    }

    @Test
    fun `use resets the idle timer`() {
        val first = manager.run("a") { it }
        now += 400_000
        manager.run("a") { 1 }
        now += 400_000
        manager.sweep()
        assertFalse(first.isClosed)
    }

    @Test
    fun `a running statement keeps the session from idling out`() {
        val started = CountDownLatch(1)
        val release = CountDownLatch(1)
        var conn: Connection? = null
        val worker = Thread {
            manager.run("a") { conn = it; started.countDown(); release.await(10, TimeUnit.SECONDS) }
        }
        worker.start()
        assertTrue(started.await(10, TimeUnit.SECONDS))
        now += 10_000_000
        manager.sweep()
        assertFalse(conn!!.isClosed)
        release.countDown()
        worker.join(10_000)
    }

    @Test
    fun `closing one id closes its connection and listeners run on the session thread`() {
        var listenerThread: Thread? = null
        val session = manager.session("a")
        val (thread, conn) = session.run { Thread.currentThread() to it }
        session.onClose { listenerThread = Thread.currentThread() }
        manager.close("a")
        assertTrue(conn.isClosed)
        assertSame(thread, listenerThread)
        assertTrue(session.closed)
        // The id is usable again with a fresh session.
        assertEquals(7, manager.run("a") { 7 })
    }

    @Test
    fun `closing the manager closes every connection`() {
        val a = manager.run("a") { it }
        val b = manager.run("b") { it }
        manager.close()
        assertTrue(a.isClosed)
        assertTrue(b.isClosed)
        assertFailsWith<DbException> { manager.run("a") { 1 } }
    }

    @Test
    fun `a failing open is not cached and the next call retries`() {
        var fail = true
        val flaky = SessionManager(
            exists = { true },
            opener = { if (fail) throw DbException(DbErrorCodes.CONNECTION_FAILED, "down") else DriverManager.getConnection("jdbc:sqlite:$file") },
            clock = { now },
        )
        try {
            assertEquals(DbErrorCodes.CONNECTION_FAILED, assertFailsWith<DbException> { flaky.run("x") { 1 } }.code)
            fail = false
            assertEquals(1, flaky.run("x") { 1 })
        } finally {
            flaky.close()
        }
    }

    @Test
    fun `a dropped connection error reconnects on the next call`() {
        val first = manager.run("a") { it }
        assertFailsWith<java.sql.SQLException> { manager.run("a") { throw java.sql.SQLException("gone", "08006") } }
        assertTrue(first.isClosed)
        val second = manager.run("a") { it }
        assertNotEquals<Connection>(first, second)
        // Other errors keep the connection.
        assertFailsWith<java.sql.SQLException> { manager.run("a") { throw java.sql.SQLException("syntax", "42000") } }
        assertSame(second, manager.run("a") { it })
    }

    @Test
    fun `a nested call on the session thread runs inline`() {
        val session = manager.session("a")
        assertEquals(5, session.run { c -> session.run { c2 -> assertSame(c, c2); 5 } })
    }

    @Test
    fun `a connection opened while the session closes is closed again`() {
        val opening = CountDownLatch(1)
        val proceed = CountDownLatch(1)
        val opened = java.util.concurrent.CopyOnWriteArrayList<Connection>()
        val slow = SessionManager(
            exists = { true },
            opener = { opening.countDown(); proceed.await(10, TimeUnit.SECONDS); DriverManager.getConnection("jdbc:sqlite:$file").also { opened += it } },
            clock = { now },
        )
        val caller = Thread { runCatching { slow.run("x") { 1 } } }
        caller.start()
        assertTrue(opening.await(10, TimeUnit.SECONDS))
        val closer = Thread { slow.close("x") }
        closer.start()
        Thread.sleep(200)
        proceed.countDown()
        caller.join(10_000)
        closer.join(10_000)
        // The retry after the closed session opens a second connection; the first, late one must be closed.
        assertTrue(opened.first().isClosed)
        slow.close()
    }
}
