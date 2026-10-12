package dev.brewery.daemon.db

import java.nio.file.Files
import java.sql.Driver
import java.sql.SQLException
import java.util.ServiceLoader
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNull
import kotlin.test.assertTrue

class ReviewFixesTest {
    private fun info(name: String, url: String = "jdbc:sqlite:x") =
        ConnectionInfo("", name, "sqlite", url, null, false, emptyMap())

    private fun sqliteInfo(): ConnectionInfo =
        info("S", "jdbc:sqlite:" + Files.createTempFile("brewery", ".db"))

    @Test
    fun `writes refuse to overwrite an invalid db json`() {
        val root = Files.createTempDirectory("brewery-db-bad")
        Files.createDirectories(root.resolve(".brewery"))
        val file = root.resolve(".brewery/db.json")
        Files.writeString(file, "{\"connections\": [oops")
        val store = ConnectionStore(root)
        val ops = listOf<() -> Unit>(
            { store.saveConnection(info("A")) },
            { store.deleteConnection("x") },
            { store.saveDriver(StoredDriver("d", "D", "a.B", emptyList())) },
        )
        for (op in ops) {
            val e = assertFailsWith<DbException> { op() }
            assertTrue(e.message!!.startsWith("db.json is invalid: "), e.message)
        }
        assertEquals("{\"connections\": [oops", Files.readString(file))
        assertTrue(store.connections().isEmpty())
    }

    @Test
    fun `null options from a client are treated as empty`() {
        val store = ConnectionStore(Files.createTempDirectory("brewery-db-null"))
        // Gson bypasses Kotlin's null checks, exactly as a client sending "options": null would.
        val gson = org.eclipse.lsp4j.jsonrpc.json.MessageJsonHandler(emptyMap()).gson
        val nullOptions = gson.fromJson(
            "{\"name\":\"N\",\"driverId\":\"sqlite\",\"url\":\"jdbc:sqlite:x\",\"options\":null}",
            ConnectionInfo::class.java,
        )
        assertEquals(emptyMap(), store.saveConnection(nullOptions).options)
    }

    @Test
    fun `all three bundled drivers are discoverable through ServiceLoader`() {
        val names = ServiceLoader.load(Driver::class.java).map { it.javaClass.name }.toSet()
        val expected = listOf("org.postgresql.Driver", "org.mariadb.jdbc.Driver", "org.sqlite.JDBC")
        assertTrue(names.containsAll(expected), "$names")
    }

    @Test
    fun `connectionFailure maps SQLState 28 to auth failed and others to connection failed`() {
        assertEquals(DbErrorCodes.AUTH_FAILED, connectionFailure(SQLException("x", "28P01")).code)
        assertEquals(DbErrorCodes.AUTH_FAILED, connectionFailure(SQLException("x", "28000")).code)
        assertEquals(DbErrorCodes.CONNECTION_FAILED, connectionFailure(SQLException("x", "08001")).code)
        assertEquals(DbErrorCodes.CONNECTION_FAILED, connectionFailure(RuntimeException("boom")).code)
    }

    @Test
    fun `removing a password from an unavailable store gives no warning`() {
        val svc = DbService(Files.createTempDirectory("brewery-db-rm"), UnavailableSecretStore("nope"))
        val saved = svc.save(sqliteInfo(), null)
        assertNull(svc.save(saved, "").warning)
    }

    @Test
    fun `test uses the stored password when none is supplied`() {
        val svc = DbService(Files.createTempDirectory("brewery-db-tp"), InMemorySecretStore())
        val saved = svc.save(sqliteInfo(), "stored")
        assertTrue(svc.test(saved, null).ok)
    }

    @Test
    fun `openSaved maps failures and unknown ids`() {
        val svc = DbService(Files.createTempDirectory("brewery-db-os"), InMemorySecretStore())
        val bad = svc.save(info("B", "jdbc:sqlite:/no/such/dir/x.db"), null)
        assertEquals(DbErrorCodes.CONNECTION_FAILED, assertFailsWith<DbException> { svc.openSaved(bad.id) }.code)
        assertEquals(DbErrorCodes.NOT_FOUND, assertFailsWith<DbException> { svc.openSaved("zzz") }.code)
        svc.openSaved(svc.save(sqliteInfo(), null).id).close()
    }
}
