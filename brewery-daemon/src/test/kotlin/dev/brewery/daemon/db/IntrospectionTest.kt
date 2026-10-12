package dev.brewery.daemon.db

import java.nio.file.Files
import java.nio.file.Path
import java.sql.DriverManager
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class IntrospectionTest {
    private val root: Path = Files.createTempDirectory("brewery-introspect")
    private var now = 0L
    private val service = DbService(root, InMemorySecretStore(), clock = { now })
    private val db = Files.createTempFile("brewery-introspect", ".db")
    private val connectionId: String

    init {
        DriverManager.getConnection("jdbc:sqlite:$db").use { c ->
            c.createStatement().use { st ->
                st.executeUpdate("CREATE TABLE owners (id INTEGER NOT NULL PRIMARY KEY, name TEXT NOT NULL, email VARCHAR(50))")
                st.executeUpdate(
                    "CREATE TABLE pets (id INTEGER NOT NULL PRIMARY KEY, owner_id INTEGER NOT NULL, name TEXT, " +
                        "CONSTRAINT fk_pets_owner FOREIGN KEY (owner_id) REFERENCES owners (id))",
                )
                st.executeUpdate("CREATE UNIQUE INDEX idx_owners_email ON owners (email)")
                st.executeUpdate("CREATE INDEX idx_pets_owner_name ON pets (owner_id, name)")
                st.executeUpdate("CREATE VIEW pet_names AS SELECT name FROM pets")
            }
        }
        connectionId = service.save(ConnectionInfo("", "lite", "sqlite", "jdbc:sqlite:$db", null, false, emptyMap()), null).id
    }

    @AfterTest
    fun tearDown() = service.close()

    private fun ls(vararg path: String) = service.introspect(connectionId, path.toList())

    @Test
    fun `sqlite root lists tables then views with lazy children`() {
        assertEquals(
            listOf(
                DbNode("owners", "table", listOf("owners"), true),
                DbNode("pets", "table", listOf("pets"), true),
                DbNode("pet_names", "view", listOf("pet_names"), true),
            ),
            ls(),
        )
    }

    @Test
    fun `a table has the three folders and a view only columns`() {
        assertEquals(
            listOf(
                DbNode("columns", "folder", listOf("pets", "columns"), true),
                DbNode("indexes", "folder", listOf("pets", "indexes"), true),
                DbNode("foreignKeys", "folder", listOf("pets", "foreignKeys"), true),
            ),
            ls("pets"),
        )
        assertEquals(listOf("columns"), ls("pet_names").map { it.name })
        assertEquals(emptyList(), ls("missing"))
    }

    @Test
    fun `columns carry type nullability and primary key in detail`() {
        val cols = ls("owners", "columns")
        assertEquals(listOf("id", "name", "email"), cols.map { it.name })
        assertTrue(cols.all { it.kind == "column" && !it.hasChildren })
        assertEquals(listOf("owners", "columns", "id"), cols[0].path)
        assertEquals("INTEGER NOT NULL PK", cols[0].detail)
        assertEquals("TEXT NOT NULL", cols[1].detail)
        assertTrue(cols[2].detail!!.startsWith("VARCHAR") && cols[2].detail!!.endsWith(" NULL"), cols[2].detail)
        assertFalse(cols[2].detail!!.endsWith("PK"))
    }

    @Test
    fun `indexes and foreign keys`() {
        val owners = ls("owners", "indexes")
        val unique = owners.single { it.name == "idx_owners_email" }
        assertEquals("index", unique.kind)
        assertEquals("UNIQUE (email)", unique.detail)
        assertEquals(listOf("owners", "indexes", "idx_owners_email"), unique.path)

        assertEquals("(owner_id, name)", ls("pets", "indexes").single { it.name == "idx_pets_owner_name" }.detail)

        val fk = ls("pets", "foreignKeys").single()
        assertEquals("foreignKey", fk.kind)
        assertEquals("(owner_id) -> owners(id)", fk.detail)
        assertEquals(listOf("pets", "foreignKeys", fk.name), fk.path)
        assertEquals(emptyList(), ls("owners", "foreignKeys"))
    }

    @Test
    fun `internal sqlite tables are hidden unless asked`() {
        DriverManager.getConnection("jdbc:sqlite:$db").use { it.createStatement().executeUpdate("CREATE TABLE seq (id INTEGER PRIMARY KEY AUTOINCREMENT)") }
        assertFalse(ls().any { it.name.startsWith("sqlite_") })
        assertTrue(service.introspect(connectionId, emptyList(), includeSystem = true).any { it.name == "sqlite_sequence" })
    }

    @Test
    fun `table names with LIKE wildcards do not match lookalikes`() {
        DriverManager.getConnection("jdbc:sqlite:$db").use {
            it.createStatement().executeUpdate("CREATE TABLE a_b (only_in_a_b INTEGER)")
            it.createStatement().executeUpdate("CREATE TABLE aXb (only_in_axb INTEGER)")
            it.createStatement().executeUpdate("CREATE TABLE \"p%q\" (only_in_pq INTEGER)")
        }
        assertEquals(listOf("only_in_a_b"), ls("a_b", "columns").map { it.name })
        assertEquals(listOf("only_in_axb"), ls("aXb", "columns").map { it.name })
        assertEquals(listOf("only_in_pq"), ls("p%q", "columns").map { it.name })
        assertEquals(3, ls("a_b").size)
    }

    @Test
    fun `introspection uses the shared session and unknown ids fail`() {
        ls()
        ls("owners")
        assertEquals(1, service.sessions.openCount)
        assertEquals(DbErrorCodes.NOT_FOUND, assertFailsWith<DbException> { service.introspect("nope", emptyList()) }.code)
    }

    @Test
    fun `the session closes when idle and when the connection is deleted or saved`() {
        ls()
        val first = service.sessions.session(connectionId).currentConnection!!
        now += SessionManager.DEFAULT_IDLE_TIMEOUT_MS
        service.sessions.sweep()
        assertTrue(first.isClosed)
        assertEquals(0, service.sessions.openCount)

        ls()
        val second = service.sessions.session(connectionId).currentConnection!!
        service.save(service.list().single().copy(name = "renamed"), null)
        assertTrue(second.isClosed)

        ls()
        val third = service.sessions.session(connectionId).currentConnection!!
        service.delete(connectionId)
        assertTrue(third.isClosed)
        assertEquals(DbErrorCodes.NOT_FOUND, assertFailsWith<DbException> { ls() }.code)
    }

    @Test
    fun `a bad url is DB_CONNECTION_FAILED`() {
        val bad = service.save(ConnectionInfo("", "bad", "sqlite", "jdbc:sqlite:/no/such/dir/x.db", null, false, emptyMap()), null)
        assertEquals(DbErrorCodes.CONNECTION_FAILED, assertFailsWith<DbException> { service.introspect(bad.id, emptyList()) }.code)
    }

    @Test
    fun `closing the service closes open sessions`() {
        ls()
        val c = service.sessions.session(connectionId).currentConnection!!
        service.close()
        assertTrue(c.isClosed)
    }
}
