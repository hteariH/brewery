package dev.brewery.daemon.db

import java.nio.file.Files
import java.nio.file.Path
import java.sql.DriverManager
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNotSame
import kotlin.test.assertSame
import kotlin.test.assertTrue

class SchemaSnapshotTest {
    private val root: Path = Files.createTempDirectory("brewery-schema")
    private val db = Files.createTempFile("brewery-schema", ".db")
    private var service = DbService(root, InMemorySecretStore())
    private val connectionId: String

    init {
        DriverManager.getConnection("jdbc:sqlite:$db").use { c ->
            c.createStatement().use { st ->
                st.executeUpdate("CREATE TABLE owners (id INTEGER NOT NULL PRIMARY KEY, name TEXT NOT NULL, email VARCHAR(50))")
                st.executeUpdate(
                    "CREATE TABLE pets (id INTEGER NOT NULL PRIMARY KEY, owner_id INTEGER NOT NULL, name TEXT, " +
                        "CONSTRAINT fk_pets_owner FOREIGN KEY (owner_id) REFERENCES owners (id))",
                )
                st.executeUpdate("CREATE VIEW pet_names AS SELECT name FROM pets")
            }
        }
        connectionId = service.save(ConnectionInfo("", "lite", "sqlite", "jdbc:sqlite:$db", null, false, emptyMap()), null).id
    }

    @AfterTest
    fun tearDown() = service.close()

    @Test
    fun `sqlite snapshot has one unnamed schema with tables then views and typed columns`() {
        val snapshot = service.schema(connectionId, false)
        assertEquals(false, snapshot.truncated)
        val schema = snapshot.schemas.single()
        assertEquals("", schema.name)
        assertEquals(listOf("owners" to "table", "pets" to "table", "pet_names" to "view"), schema.tables.map { it.name to it.kind })
        val owners = schema.tables[0].columns
        assertEquals(listOf("id", "name", "email"), owners.map { it.name })
        assertEquals("INTEGER", owners[0].type)
        assertEquals("TEXT", owners[1].type)
        assertTrue(owners[2].type.startsWith("VARCHAR"), owners[2].type)
        assertEquals(listOf("id", "owner_id", "name"), schema.tables[1].columns.map { it.name })
        assertEquals(listOf("name"), schema.tables[2].columns.map { it.name })
    }

    @Test
    fun `internal sqlite tables are not in the snapshot`() {
        DriverManager.getConnection("jdbc:sqlite:$db").use { it.createStatement().executeUpdate("CREATE TABLE seq (id INTEGER PRIMARY KEY AUTOINCREMENT)") }
        val names = service.schema(connectionId, false).schemas.single().tables.map { it.name }
        assertTrue("seq" in names && names.none { it.startsWith("sqlite_") }, names.toString())
    }

    @Test
    fun `a cache hit returns the same snapshot without touching the database`() {
        val first = service.schema(connectionId, false)
        assertEquals(1, service.schemaBuilds.get())
        assertSame(first, service.schema(connectionId, false))
        assertSame(first, service.schema(connectionId, null))
        assertEquals(1, service.schemaBuilds.get())
    }

    @Test
    fun `refresh rebuilds`() {
        val first = service.schema(connectionId, false)
        DriverManager.getConnection("jdbc:sqlite:$db").use { it.createStatement().executeUpdate("CREATE TABLE later (x INTEGER)") }
        assertSame(first, service.schema(connectionId, false))
        val second = service.schema(connectionId, true)
        assertNotSame(first, second)
        assertEquals(2, service.schemaBuilds.get())
        assertTrue(second.schemas.single().tables.any { it.name == "later" })
        assertSame(second, service.schema(connectionId, false))
    }

    @Test
    fun `ddl through execute invalidates but queries and dml do not`() {
        val first = service.schema(connectionId, false)
        service.execute(connectionId, "INSERT INTO owners (id, name) VALUES (1, 'a')")
        service.execute(connectionId, "-- note\nSELECT * FROM owners").cursorId?.let(service::closeCursor)
        assertSame(first, service.schema(connectionId, false))

        service.execute(connectionId, "CREATE TABLE toys (id INTEGER, label TEXT)")
        val second = service.schema(connectionId, false)
        assertNotSame(first, second)
        assertTrue(second.schemas.single().tables.any { it.name == "toys" })

        service.execute(connectionId, "/* c */ drop table toys")
        val third = service.schema(connectionId, false)
        assertTrue(third.schemas.single().tables.none { it.name == "toys" })

        service.execute(connectionId, "ALTER TABLE owners ADD COLUMN phone TEXT")
        assertTrue(service.schema(connectionId, false).schemas.single().tables.first { it.name == "owners" }.columns.any { it.name == "phone" })
    }

    @Test
    fun `ddl later in a multi statement script invalidates too`() {
        val first = service.schema(connectionId, false)
        // The SQLite driver runs only the first statement; the invalidation looks at the whole text anyway.
        runCatching { service.execute(connectionId, "SELECT 1; CREATE TABLE multi (a INTEGER)") }
        assertNotSame(first, service.schema(connectionId, false))
    }

    @Test
    fun `a failing ddl statement still drops the cache`() {
        val first = service.schema(connectionId, false)
        assertFailsWith<DbException> { service.execute(connectionId, "DROP TABLE nope") }
        assertNotSame(first, service.schema(connectionId, false))
    }

    @Test
    fun `saving or deleting the connection invalidates`() {
        val first = service.schema(connectionId, false)
        service.save(service.list().single().copy(name = "renamed"), null)
        val second = service.schema(connectionId, false)
        assertNotSame(first, second)
        service.delete(connectionId)
        assertEquals(DbErrorCodes.NOT_FOUND, assertFailsWith<DbException> { service.schema(connectionId, false) }.code)
    }

    @Test
    fun `an invalidation during a build keeps the stale result out of the cache`() {
        val cache = SchemaCache()
        val stale = DbSchema(emptyList(), false)
        val fresh = DbSchema(listOf(SchemaInfo("x", emptyList())), false)
        val got = cache.get("c", false) {
            cache.invalidate("c")
            stale
        }
        assertSame(stale, got)
        assertSame(fresh, cache.get("c", false) { fresh })
    }

    /** Waits until the thread in [ref] is parked on the in-flight build (so it really joined it). */
    private fun awaitWaiting(ref: java.util.concurrent.atomic.AtomicReference<Thread>) {
        val deadline = System.nanoTime() + 10_000_000_000L
        while (ref.get()?.state != Thread.State.WAITING) {
            check(System.nanoTime() < deadline) { "joiner never waited" }
            Thread.sleep(5)
        }
    }

    @Test
    fun `concurrent misses share one build`() {
        val cache = SchemaCache()
        val builds = java.util.concurrent.atomic.AtomicInteger()
        val started = java.util.concurrent.CountDownLatch(1)
        val release = java.util.concurrent.CountDownLatch(1)
        val result = DbSchema(listOf(SchemaInfo("s", emptyList())), false)
        val pool = java.util.concurrent.Executors.newFixedThreadPool(6)
        try {
            val first = pool.submit<DbSchema> { cache.get("c", false) { builds.incrementAndGet(); started.countDown(); release.await(); result } }
            started.await()
            val joiner = java.util.concurrent.atomic.AtomicReference<Thread>()
            val others = (1..5).map { pool.submit<DbSchema> { joiner.set(Thread.currentThread()); cache.get("c", false) { builds.incrementAndGet(); result } } }
            awaitWaiting(joiner)
            release.countDown()
            assertSame(result, first.get(10, java.util.concurrent.TimeUnit.SECONDS))
            others.forEach { assertSame(result, it.get(10, java.util.concurrent.TimeUnit.SECONDS)) }
            assertEquals(1, builds.get())
        } finally {
            pool.shutdownNow()
        }
    }

    @Test
    fun `a failed build reaches its waiters and is not remembered`() {
        val cache = SchemaCache()
        val started = java.util.concurrent.CountDownLatch(1)
        val release = java.util.concurrent.CountDownLatch(1)
        val pool = java.util.concurrent.Executors.newFixedThreadPool(2)
        try {
            val first = pool.submit<DbSchema> { cache.get("c", false) { started.countDown(); release.await(); throw DbException(DbErrorCodes.CONNECTION_FAILED, "down") } }
            started.await()
            val joiner = java.util.concurrent.atomic.AtomicReference<Thread>()
            val second = pool.submit<DbSchema> { joiner.set(Thread.currentThread()); cache.get("c", false) { error("must join the first build") } }
            awaitWaiting(joiner)
            release.countDown()
            for (f in listOf(first, second)) {
                val e = assertFailsWith<java.util.concurrent.ExecutionException> { f.get(10, java.util.concurrent.TimeUnit.SECONDS) }
                assertEquals(DbErrorCodes.CONNECTION_FAILED, (e.cause as DbException).code)
            }
            val ok = DbSchema(emptyList(), false)
            assertSame(ok, cache.get("c", false) { ok })
        } finally {
            pool.shutdownNow()
        }
    }

    @Test
    fun `a refresh during a build starts a new build and the older result is not cached`() {
        val cache = SchemaCache()
        val started = java.util.concurrent.CountDownLatch(1)
        val release = java.util.concurrent.CountDownLatch(1)
        val old = DbSchema(emptyList(), false)
        val fresh = DbSchema(listOf(SchemaInfo("new", emptyList())), false)
        val pool = java.util.concurrent.Executors.newFixedThreadPool(1)
        try {
            val first = pool.submit<DbSchema> { cache.get("c", false) { started.countDown(); release.await(); old } }
            started.await()
            assertSame(fresh, cache.get("c", true) { fresh })
            release.countDown()
            assertSame(old, first.get(10, java.util.concurrent.TimeUnit.SECONDS))
            assertSame(fresh, cache.get("c", false) { error("fresh must be cached") })
        } finally {
            pool.shutdownNow()
        }
    }

    @Test
    fun `concurrent requests through the service build once`() {
        val pool = java.util.concurrent.Executors.newFixedThreadPool(8)
        try {
            val all = (1..8).map { pool.submit<DbSchema> { service.schema(connectionId, false) } }.map { it.get(20, java.util.concurrent.TimeUnit.SECONDS) }
            assertTrue(all.all { it === all[0] })
            assertEquals(1, service.schemaBuilds.get())
        } finally {
            pool.shutdownNow()
        }
    }

    @Test
    fun `a build for a non mariadb url runs on the session without a probe connection or thread`() {
        service.openMetadataConnection = { error("no extra connection expected") }
        service.schema(connectionId, false)
        service.schema(connectionId, true)
        assertEquals(1, service.sessions.openCount)
        assertEquals(0, service.metadataThreadCount)
    }

    @Test
    fun `the path is chosen from the url`() {
        assertTrue(isMariaDbUrl("jdbc:mariadb://h/db"))
        assertTrue(isMariaDbUrl("jdbc:mysql://h/db"))
        assertTrue(!isMariaDbUrl("jdbc:postgresql://h/db"))
        assertTrue(!isMariaDbUrl("jdbc:sqlite:x.db"))
    }

    @Test
    fun `a separate connection that cannot be opened falls back to the session`() {
        service.close()
        service = DbService(root, InMemorySecretStore(), manageTransactions = { false }, expireUnfinishedBeforeCommands = { true }, snapshotOnSeparateConnection = { true })
        service.openMetadataConnection = { throw DbException(DbErrorCodes.CONNECTION_FAILED, "too many connections") }
        DriverManager.getConnection("jdbc:sqlite:$db").use { c ->
            c.createStatement().use { st -> st.executeUpdate("INSERT INTO owners (id, name) VALUES (1, 'a'), (2, 'b')") }
        }
        val cursor = service.execute(connectionId, "SELECT id FROM owners").cursorId!!
        assertEquals(1, service.fetch(cursor, 0, 1).rows.size)
        val snapshot = service.schema(connectionId, false)
        assertEquals(listOf("owners", "pets", "pet_names"), snapshot.schemas.single().tables.map { it.name })
        assertEquals(1, service.schemaBuilds.get())
        // The documented price of the fallback: like introspection it ends unfinished cursors.
        assertEquals(-32013, assertFailsWith<DbException> { service.fetch(cursor, 1, 1) }.code)
        // Not remembered as broken: a later build tries the separate connection again.
        var attempts = 0
        service.openMetadataConnection = { attempts++; throw DbException(DbErrorCodes.AUTH_FAILED, "no") }
        service.schema(connectionId, true)
        assertEquals(1, attempts)
    }

    @Test
    fun `other failures of the separate connection are not swallowed`() {
        service.close()
        service = DbService(root, InMemorySecretStore(), snapshotOnSeparateConnection = { true })
        service.openMetadataConnection = { throw DbException(DbErrorCodes.NOT_FOUND, "gone") }
        assertEquals(DbErrorCodes.NOT_FOUND, assertFailsWith<DbException> { service.schema(connectionId, false) }.code)
    }

    @Test
    fun `with the mariadb policy the build uses a separate connection and keeps cursors alive`() {
        service.close()
        service = DbService(root, InMemorySecretStore(), manageTransactions = { false }, expireUnfinishedBeforeCommands = { true }, snapshotOnSeparateConnection = { true })
        DriverManager.getConnection("jdbc:sqlite:$db").use { c ->
            c.createStatement().use { st -> st.executeUpdate("INSERT INTO owners (id, name) VALUES (1, 'a'), (2, 'b'), (3, 'c')") }
        }
        val cursor = service.execute(connectionId, "SELECT id FROM owners").cursorId!!
        assertEquals(1, service.fetch(cursor, 0, 1).rows.size)

        val snapshot = service.schema(connectionId, true)
        assertEquals(listOf("owners", "pets", "pet_names"), snapshot.schemas.single().tables.map { it.name })
        service.schema(connectionId, true)

        assertEquals(2, service.fetch(cursor, 1, 10).rows.size)
        // introspection still expires them (user-initiated), the snapshot did not.
        val another = service.execute(connectionId, "SELECT id FROM owners").cursorId!!
        assertEquals(1, service.fetch(another, 0, 1).rows.size)
        service.introspect(connectionId, emptyList())
        assertEquals(-32013, assertFailsWith<DbException> { service.fetch(another, 1, 1) }.code)
    }

    @Test
    fun `the separate connection path maps errors like the session path`() {
        service.close()
        service = DbService(root, InMemorySecretStore(), expireUnfinishedBeforeCommands = { true }, snapshotOnSeparateConnection = { true })
        assertEquals(DbErrorCodes.NOT_FOUND, assertFailsWith<DbException> { service.schema("missing", false) }.code)
        val bad = service.save(ConnectionInfo("", "bad", "sqlite", "jdbc:sqlite:/no/such/dir/x.db", null, false, emptyMap()), null)
        assertEquals(DbErrorCodes.CONNECTION_FAILED, assertFailsWith<DbException> { service.schema(bad.id, false) }.code)
    }

    @Test
    fun `truncation at the injected limit`() {
        service.close()
        service = DbService(root, InMemorySecretStore(), maxSchemaTables = 2)
        val snapshot = service.schema(connectionId, false)
        assertEquals(true, snapshot.truncated)
        val tables = snapshot.schemas.single().tables
        assertEquals(listOf("owners", "pets"), tables.map { it.name })
        assertTrue(tables.all { it.columns.isNotEmpty() })

        service.close()
        service = DbService(root, InMemorySecretStore(), maxSchemaTables = 3)
        val exact = service.schema(connectionId, false)
        assertEquals(false, exact.truncated)
        assertEquals(3, exact.schemas.single().tables.size)
    }

    @Test
    fun `unknown and unreachable connections use the introspection error codes`() {
        assertEquals(DbErrorCodes.NOT_FOUND, assertFailsWith<DbException> { service.schema("missing", false) }.code)
        val bad = service.save(ConnectionInfo("", "bad", "sqlite", "jdbc:sqlite:/no/such/dir/x.db", null, false, emptyMap()), null)
        assertEquals(DbErrorCodes.CONNECTION_FAILED, assertFailsWith<DbException> { service.schema(bad.id, false) }.code)
    }

    @Test
    fun `ddl detection looks at every statement and ignores comments and strings`() {
        assertTrue(SqlText.isDdl("CREATE TABLE t (a int)"))
        assertTrue(SqlText.isDdl("  -- x\n /* y */ alter table t add b int"))
        assertTrue(SqlText.isDdl("(truncate t)"))
        assertTrue(SqlText.isDdl("RENAME TABLE a TO b"))
        assertTrue(SqlText.isDdl("select 1; drop table t"))
        assertTrue(!SqlText.isDdl("SELECT 'create table x' -- drop table y"))
        assertTrue(!SqlText.isDdl("INSERT INTO t VALUES ('; drop table t')"))
        assertTrue(!SqlText.isDdl("SELECT * FROM create_log"))
        assertTrue(!SqlText.isDdl(""))
    }
}
