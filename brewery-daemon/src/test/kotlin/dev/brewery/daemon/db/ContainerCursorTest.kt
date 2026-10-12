package dev.brewery.daemon.db

import org.junit.jupiter.api.AfterAll
import org.junit.jupiter.api.BeforeAll
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.TestInstance
import org.junit.jupiter.api.condition.EnabledIf
import org.testcontainers.postgresql.PostgreSQLContainer
import java.nio.file.Files
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

@EnabledIf("dev.brewery.daemon.db.DockerSupport#available", disabledReason = "Docker is not available")
@TestInstance(TestInstance.Lifecycle.PER_CLASS)
class PostgresCursorTest {
    private val postgres = PostgreSQLContainer("postgres:16-alpine")
    private lateinit var service: DbService
    private lateinit var id: String

    @BeforeAll
    fun start() {
        postgres.start()
        service = DbService(Files.createTempDirectory("brewery-pg-cursor"), InMemorySecretStore())
        id = service.save(ConnectionInfo("", "pg", "postgresql", postgres.jdbcUrl, postgres.username, false, emptyMap()), postgres.password).id
    }

    @AfterAll
    fun stop() {
        if (::service.isInitialized) service.close()
        postgres.stop()
    }

    @Test
    fun `select pages and renders`() {
        val r = service.execute(id, "SELECT g AS n, now() AS ts, '\\x0102'::bytea AS b FROM generate_series(1, 3) g")
        assertEquals(listOf("n", "ts", "b"), r.columns.map { it.name })
        val page = service.fetch(r.cursorId!!, 0, 10)
        assertEquals(3, page.rows.size)
        assertTrue(page.done)
        assertTrue(page.rows[0][1]!!.contains("T"), page.rows[0][1])
        assertEquals("0x0102", page.rows[0][2])
    }

    @Test
    fun `query error carries the SQLState`() {
        val e = assertFailsWith<DbException> { service.execute(id, "SELECT * FROM no_such_table") }
        assertEquals(-32012, e.code)
        assertTrue(e.message!!.contains("SQLState 42P01"), e.message)
    }

    @Test
    fun `cancel stops pg_sleep and the next execute works`() {
        val failure = AtomicReference<Throwable?>()
        val finished = CountDownLatch(1)
        val worker = Thread {
            try {
                service.execute(id, "SELECT pg_sleep(30)")
            } catch (t: Throwable) {
                failure.set(t)
            } finally {
                finished.countDown()
            }
        }
        val started = System.nanoTime()
        worker.start()
        while (service.sessions.session(id).runningStatement == null && System.nanoTime() - started < 15_000_000_000L) Thread.sleep(10)
        Thread.sleep(300)
        service.cancel(id, null)
        assertTrue(finished.await(10, TimeUnit.SECONDS), "pg_sleep was not cancelled")
        val error = failure.get()
        assertTrue(error is DbException && error.code == -32012, "unexpected: $error")
        assertTrue(error.message!!.contains("57014"), error.message)
        assertEquals(listOf(listOf("1")), service.fetch(service.execute(id, "SELECT 1").cursorId!!, 0, 1).rows)
    }
}
