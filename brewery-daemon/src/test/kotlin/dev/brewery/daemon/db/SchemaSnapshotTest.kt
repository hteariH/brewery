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
