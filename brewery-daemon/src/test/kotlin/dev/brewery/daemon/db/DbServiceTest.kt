package dev.brewery.daemon.db

import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

class DbServiceTest {
    private val root: Path = Files.createTempDirectory("brewery-db-svc")
    private val secrets = InMemorySecretStore()
    private val service = DbService(root, secrets)

    private fun sqlite(name: String = "S"): ConnectionInfo {
        val file = Files.createTempFile("brewery", ".db")
        return ConnectionInfo("", name, "sqlite", "jdbc:sqlite:$file", null, false, emptyMap())
    }

    private fun account(id: String) = "${root.toRealPath()}#$id"

    @Test
    fun `password semantics null keeps, value saves, empty removes`() {
        val saved = service.save(sqlite(), "s3cret")
        assertTrue(saved.hasPassword)
        assertEquals("s3cret", secrets.get(account(saved.id)))
        assertEquals("brewery-db", SecretStore.SERVICE)

        val kept = service.save(saved.copy(name = "S2"), null)
        assertTrue(kept.hasPassword)
        assertEquals("s3cret", secrets.get(account(saved.id)))

        val replaced = service.save(kept, "other")
        assertEquals("other", secrets.get(account(saved.id)))
        assertTrue(replaced.hasPassword)

        val removed = service.save(replaced, "")
        assertFalse(removed.hasPassword)
        assertNull(secrets.get(account(saved.id)))
    }

    @Test
    fun `list reports hasPassword from the secret store and nothing hits disk`() {
        val a = service.save(sqlite("A"), "topsecretvalue")
        service.save(sqlite("B"), null)
        assertEquals(listOf(true, false), service.list().map { it.hasPassword })
        val json = Files.readString(root.resolve(".brewery/db.json"))
        assertFalse("topsecretvalue" in json, json)
        assertTrue(a.id in json)
    }

    @Test
    fun `delete removes the secret and unknown ids are not found`() {
        val a = service.save(sqlite(), "pw")
        service.delete(a.id)
        assertNull(secrets.get(account(a.id)))
        assertTrue(service.list().isEmpty())
        assertEquals(DbErrorCodes.NOT_FOUND, assertFailsWith<DbException> { service.delete(a.id) }.code)
    }

    @Test
    fun `save with an unknown driver is not found`() {
        val bad = sqlite().copy(driverId = "nope")
        assertEquals(DbErrorCodes.NOT_FOUND, assertFailsWith<DbException> { service.save(bad, null) }.code)
    }

    @Test
    fun `test against an sqlite file is ok with a server version`() {
        val result = service.test(sqlite(), null)
        assertTrue(result.ok, result.message)
        assertTrue(result.serverVersion!!.startsWith("SQLite"), result.serverVersion)
    }

    @Test
    fun `test with a bad url reports ok=false instead of throwing`() {
        val result = service.test(sqlite().copy(url = "not-a-jdbc-url"), null)
        assertFalse(result.ok)
        assertTrue(result.message.isNotBlank())
        assertNull(result.serverVersion)
    }

    @Test
    fun `test with an unreachable sqlite path is ok=false`() {
        val result = service.test(sqlite().copy(url = "jdbc:sqlite:/no/such/dir/x.db"), null)
        assertFalse(result.ok)
    }

    @Test
    fun `test with unknown driver is ok=false`() {
        val result = service.test(sqlite().copy(driverId = "nope"), null)
        assertFalse(result.ok)
    }

    @Test
    fun `unavailable keyring degrades to not stored with a warning`() {
        val svc = DbService(root, UnavailableSecretStore("no keyring here"))
        val saved = svc.save(sqlite(), "pw")
        assertFalse(saved.hasPassword)
        assertTrue(saved.warning!!.contains("no keyring here"), saved.warning)
        val result = svc.test(sqlite(), "pw")
        assertTrue(result.ok, result.message)
        assertFalse(svc.list().single().hasPassword)
        svc.delete(saved.id)
    }

    @Test
    fun `adding a bad user driver fails with DB_CONNECTION_FAILED`() {
        val jar = Files.createTempFile("x", ".jar")
        assertEquals(DbErrorCodes.CONNECTION_FAILED, assertFailsWith<DbException> {
            service.addJar("x", "X", "a.B", listOf(jar.toString()))
        }.code)
        assertTrue(service.drivers().any { it.id == "sqlite" && it.bundled })
    }
}
