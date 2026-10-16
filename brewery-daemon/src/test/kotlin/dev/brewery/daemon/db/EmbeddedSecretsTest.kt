package dev.brewery.daemon.db

import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** Issue #3: a password typed into the URL or the options must not end up in `db.json`. */
class EmbeddedSecretsTest {
    private val root: Path = Files.createTempDirectory("brewery-db-embedded")
    private val secrets = InMemorySecretStore()
    private val service = DbService(root, secrets)
    private val dbJson: Path get() = root.resolve(".brewery/db.json")

    private fun account(id: String) = "${root.toRealPath()}#$id"

    private fun conn(url: String, user: String? = null, options: Map<String, String> = emptyMap(), driverId: String = "postgresql") =
        ConnectionInfo("", "C", driverId, url, user, false, options)

    private fun assertOnDiskWithout(secret: String) {
        val json = Files.readString(dbJson)
        assertFalse(secret in json, json)
    }

    @Test
    fun `postgres userinfo password moves to the secret store and the user to the user field`() {
        val saved = service.save(conn("jdbc:postgresql://u:secret@host:5432/db?ssl=true"), null)
        assertEquals("jdbc:postgresql://host:5432/db?ssl=true", saved.url)
        assertEquals("u", saved.user)
        assertTrue(saved.hasPassword)
        assertEquals("secret", secrets.get(account(saved.id)))
        assertNotNull(saved.warning, "the dialog tells the user where the password went")
        assertOnDiskWithout("secret")
    }

    @Test
    fun `postgres query password is decoded like the driver does and stripped`() {
        val saved = service.save(conn("jdbc:postgresql://host/db?user=app&password=s%40cret&ssl=true"), null)
        assertEquals("jdbc:postgresql://host/db?user=app&ssl=true", saved.url)
        assertEquals("s@cret", secrets.get(account(saved.id)))
        assertOnDiskWithout("cret")
    }

    @Test
    fun `mariadb and mysql urls keep the raw value and any key case`() {
        val maria = service.save(conn("jdbc:mariadb://h/db?PASSWORD=a%40b", driverId = "mariadb"), null)
        assertEquals("jdbc:mariadb://h/db", maria.url)
        assertEquals("a%40b", secrets.get(account(maria.id)))

        val mysql = service.save(conn("jdbc:mysql://root:pw1@h:3306/db?useSSL=false&pwd=pw1", driverId = "mysql"), null)
        assertEquals("jdbc:mysql://h:3306/db?useSSL=false", mysql.url)
        assertEquals("root", mysql.user)
        assertEquals("pw1", secrets.get(account(mysql.id)))

        val address = service.save(conn("jdbc:mysql://address=(host=h)(port=3306)(password=pw2)/db", driverId = "mysql"), null)
        assertEquals("jdbc:mysql://address=(host=h)(port=3306)/db", address.url)
        assertEquals("pw2", secrets.get(account(address.id)))

        val kv = service.save(conn("jdbc:mysql://(host=h,password=pw3,port=3306)/db", driverId = "mysql"), null)
        assertEquals("jdbc:mysql://(host=h,port=3306)/db", kv.url)
        assertEquals("pw3", secrets.get(account(kv.id)))
        assertOnDiskWithout("pw")
    }

    @Test
    fun `password driver options move to the secret store`() {
        val saved = service.save(conn("jdbc:postgresql://h/db", options = mapOf("Password" to "osecret1", "ssl" to "true")), null)
        assertEquals(mapOf("ssl" to "true"), saved.options)
        assertEquals("osecret1", secrets.get(account(saved.id)))
        val pwd = service.save(conn("jdbc:postgresql://h/db", options = mapOf("pwd" to "osecret2")), null)
        assertEquals(emptyMap(), pwd.options)
        assertEquals("osecret2", secrets.get(account(pwd.id)))
        assertOnDiskWithout("osecret")
    }

    @Test
    fun `a typed password wins over a leftover option and an equal url password is fine`() {
        val saved = service.save(conn("jdbc:postgresql://h/db?password=same", options = mapOf("password" to "old")), "same")
        assertEquals("same", secrets.get(account(saved.id)))
        assertEquals(emptyMap(), saved.options)
        assertOnDiskWithout("old")
    }

    @Test
    fun `different passwords in the url and the password field are refused`() {
        val e = assertFailsWith<DbException> { service.save(conn("jdbc:postgresql://h/db?password=one"), "two") }
        assertEquals(DbErrorCodes.INVALID_PARAMS, e.code)
        assertTrue("Password field" in e.message!!, e.message)
        assertFalse(Files.exists(dbJson))
    }

    @Test
    fun `a url user that differs from the user field is refused`() {
        val e = assertFailsWith<DbException> { service.save(conn("jdbc:postgresql://a:pw@h/db", user = "b"), null) }
        assertEquals(DbErrorCodes.INVALID_PARAMS, e.code)
        assertFalse(Files.exists(dbJson))
    }

    @Test
    fun `without a secret store the save is refused instead of writing the password`() {
        val unavailable = DbService(root, UnavailableSecretStore("no keyring"))
        val e = assertFailsWith<DbException> { unavailable.save(conn("jdbc:postgresql://h/db?password=leak"), null) }
        assertEquals(DbErrorCodes.INVALID_PARAMS, e.code)
        assertTrue("no keyring" in e.message!!, e.message)
        assertFalse(Files.exists(dbJson))
        // Without an embedded password the store is not needed: saving still works.
        unavailable.save(conn("jdbc:postgresql://h/db"), null)
    }

    @Test
    fun `forgetting the password drops a leftover option and a typed one only warns without a store`() {
        val forgot = service.save(conn("jdbc:postgresql://h/db", options = mapOf("password" to "leftover")), "")
        assertFalse(forgot.hasPassword)
        assertOnDiskWithout("leftover")

        val unavailable = DbService(root, UnavailableSecretStore("no keyring"))
        val typed = unavailable.save(conn("jdbc:postgresql://h/db", options = mapOf("pwd" to "leftover2")), "typed")
        assertEquals(emptyMap(), typed.options)
        assertTrue("not stored" in typed.warning!!, typed.warning)
        assertOnDiskWithout("leftover")
    }

    @Test
    fun `existing db json entries are cleaned on the next save`() {
        Files.createDirectories(dbJson.parent)
        Files.writeString(
            dbJson,
            """
            {"connections": [
              {"id": "url", "name": "U", "driverId": "postgresql", "url": "jdbc:postgresql://h/db?password=legacy1", "user": "u", "options": {}},
              {"id": "opt", "name": "O", "driverId": "postgresql", "url": "jdbc:postgresql://h/db", "user": "u", "options": {"password": "legacy2"}},
              {"id": "kept", "name": "K", "driverId": "postgresql", "url": "jdbc:postgresql://h/db", "user": "u", "options": {"password": "legacy3"}}
            ]}
            """.trimIndent(),
        )
        // The stored secret is what the driver used over an option (properties set after the options).
        secrets.set(account("kept"), "stored")

        service.save(conn("jdbc:sqlite::memory:", driverId = "sqlite"), null)

        assertOnDiskWithout("legacy")
        assertEquals("legacy1", secrets.get(account("url")))
        assertEquals("legacy2", secrets.get(account("opt")))
        assertEquals("stored", secrets.get(account("kept")))
        assertEquals(listOf("jdbc:postgresql://h/db"), service.list().filter { it.id == "url" }.map { it.url })
        assertTrue(service.list().filter { it.driverId == "postgresql" }.all { it.hasPassword && it.options.isEmpty() })
    }

    @Test
    fun `legacy entries stay as they are when the secret store is unavailable`() {
        Files.createDirectories(dbJson.parent)
        Files.writeString(
            dbJson,
            """{"connections": [{"id": "url", "name": "U", "driverId": "postgresql", "url": "jdbc:postgresql://h/db?password=legacy", "user": "u", "options": {}}]}""",
        )
        DbService(root, UnavailableSecretStore("no keyring")).save(conn("jdbc:sqlite::memory:", driverId = "sqlite"), null)
        // Dropping it would lose the only copy of the password.
        assertTrue("legacy" in Files.readString(dbJson))
    }

    @Test
    fun `urls without credentials are stored verbatim`() {
        for (url in listOf(
            "jdbc:postgresql://u@h/db?ssl=true",
            "jdbc:sqlite:C:/data/a@b;c.db",
            "jdbc:mariadb://h1,h2/db?passwordCharacterEncoding=utf8",
        )) {
            val saved = service.save(conn(url, driverId = if (url.startsWith("jdbc:sqlite")) "sqlite" else "postgresql"), null)
            assertEquals(url, saved.url)
            assertNull(saved.warning)
            assertFalse(saved.hasPassword)
        }
    }

    @Test
    fun `test connects with the password from the url`() {
        val file = Files.createTempFile("brewery", ".db")
        // SQLite ignores credentials; the point is that test() accepts the URL and does not throw.
        val result = service.test(conn("jdbc:sqlite:$file", driverId = "sqlite"), null)
        assertTrue(result.ok, result.message)
        val conflict = service.test(conn("jdbc:postgresql://h/db?password=one"), "two")
        assertFalse(conflict.ok)
        assertTrue("Password field" in conflict.message, conflict.message)
    }
}
