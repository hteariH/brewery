package dev.brewery.daemon.mcp

import com.google.gson.JsonObject
import org.junit.jupiter.api.Test
import java.nio.file.Files
import java.nio.file.Path
import java.sql.DriverManager
import java.sql.SQLException
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class DbToolsTest {
    private fun project(): Path {
        val root = Files.createTempDirectory("dbtools")
        val db = root.resolve("shop.db")
        DriverManager.getConnection("jdbc:sqlite:$db").use { c ->
            c.createStatement().use { s ->
                s.execute("create table pet(id integer primary key, name text not null)")
                for (i in 1..250) s.execute("insert into pet(name) values ('pet$i')")
            }
        }
        val res = root.resolve("src/main/resources")
        Files.createDirectories(res)
        Files.writeString(res.resolve("application.properties"), "spring.datasource.url=jdbc:sqlite:${db.toString().replace('\\', '/')}\n")
        return root
    }

    private val root = project()
    private val tools = DbTools(root, env = { null }).tools().associateBy { it.name }
    private fun args(vararg pairs: Pair<String, String>) = JsonObject().apply { pairs.forEach { (k, v) -> addProperty(k, v) } }

    @Test
    fun `schema lists tables and one table's columns`() {
        val all = tools.getValue("db_schema").call(args()).toString()
        assertTrue(all.contains("pet"))
        val pet = tools.getValue("db_schema").call(args("table" to "PET")).toString()
        assertTrue(pet.contains("name"), pet)
    }

    @Test
    fun `query returns at most 200 rows and says it truncated`() {
        val r = tools.getValue("db_query").call(args("sql" to "select id, name from pet order by id")) as QueryResult
        assertEquals(listOf("id", "name"), r.columns)
        assertEquals(MAX_ROWS, r.rows.size)
        assertEquals(listOf("1", "pet1"), r.rows[0])
        assertTrue(r.truncated)
    }

    @Test
    fun `writes and multi statements are rejected`() {
        for (sql in listOf("delete from pet", "select 1; delete from pet", "insert into pet(name) values ('x')", "select * into x from pet")) {
            assertFailsWith<McpToolError>(sql) { tools.getValue("db_query").call(args("sql" to sql)) }
        }
        val count = tools.getValue("db_query").call(args("sql" to "select count(*) from pet")) as QueryResult
        assertEquals("250", count.rows[0][0])
    }

    @Test
    fun `missing datasource is a tool error`() {
        val empty = DbTools(Files.createTempDirectory("nods"), env = { null }).tools().associateBy { it.name }
        val e = assertFailsWith<McpToolError> { empty.getValue("db_schema").call(args()) }
        assertTrue(e.message!!.contains("spring.datasource.url"))
    }

    @Test
    fun `a missing sqlite file is not created`() {
        val dir = Files.createTempDirectory("nodb")
        val missing = dir.resolve("missing.db")
        val res = dir.resolve("src/main/resources")
        Files.createDirectories(res)
        Files.writeString(res.resolve("application.properties"), "spring.datasource.url=jdbc:sqlite:${missing.toString().replace('\\', '/')}\n")
        val missingTools = DbTools(dir, env = { null }).tools().associateBy { it.name }
        assertFailsWith<McpToolError> { missingTools.getValue("db_schema").call(args()) }
        assertFailsWith<McpToolError> { missingTools.getValue("db_query").call(args("sql" to "select 1")) }
        assertFalse(Files.exists(missing))
    }

    @Test
    fun `the sqlite connection itself refuses writes`() {
        val url = Files.readString(root.resolve("src/main/resources/application.properties")).substringAfter("=").trim()
        openJdbc(Datasource(url, null, null, root)).use { c ->
            assertTrue(c.isReadOnly)
            assertFailsWith<SQLException> { c.createStatement().use { it.executeUpdate("delete from pet") } }
        }
        val count = tools.getValue("db_query").call(args("sql" to "select count(*) from pet")) as QueryResult
        assertEquals("250", count.rows[0][0])
    }

    @Test
    fun `a connection that cannot be made read-only is a failure, not a silent downgrade`() {
        // A writable SQLite connection: the driver refuses to switch it to read-only once open.
        val writable = DbTools(root, env = { null }, connect = { DriverManager.getConnection(it.url) }).tools().associateBy { it.name }
        assertFailsWith<SQLException> { writable.getValue("db_query").call(args("sql" to "select 1")) }
    }

    @Test
    fun `an unreadable config file is a tool error`() {
        val bad = Files.createTempDirectory("badcfg")
        val res = bad.resolve("src/main/resources")
        Files.createDirectories(res)
        // Latin-1 bytes are not valid UTF-8, so reading the file fails with an IOException.
        Files.write(res.resolve("application.properties"), "spring.datasource.url=jdbc:sqlite:café.db\n".toByteArray(Charsets.ISO_8859_1))
        val badTools = DbTools(bad, env = { null }).tools().associateBy { it.name }
        for (call in listOf({ badTools.getValue("db_schema").call(args()) }, { badTools.getValue("db_query").call(args("sql" to "select 1")) })) {
            val e = assertFailsWith<McpToolError> { call() }
            assertTrue(e.message!!.contains("Could not read the Spring configuration"), e.message)
        }
    }

    @Test
    fun `a jdbc mysql url is handed to the driver as jdbc mariadb`() {
        var seen: String? = null
        assertFailsWith<SQLException> {
            openJdbc(Datasource("jdbc:mysql://db.local:3306/shop", "u", "p", root)) { url, _ -> seen = url; throw SQLException("stop") }
        }
        assertEquals("jdbc:mariadb://db.local:3306/shop", seen)
        assertFailsWith<SQLException> {
            openJdbc(Datasource("jdbc:postgresql://db.local/shop", null, null, root)) { url, _ -> seen = url; throw SQLException("stop") }
        }
        assertEquals("jdbc:postgresql://db.local/shop", seen)
    }

    @Test
    fun `a connect error does not echo the url parameters`() {
        val dir = Files.createTempDirectory("secreturl")
        val res = dir.resolve("src/main/resources")
        Files.createDirectories(res)
        Files.writeString(
            res.resolve("application.properties"),
            "spring.datasource.url=jdbc:postgresql://db.local/shop?user=app&password=s3cret\n",
        )
        val failing = DbTools(dir, env = { null }, connect = { throw SQLException("refused") }).tools().associateBy { it.name }
        val e = assertFailsWith<McpToolError> { failing.getValue("db_query").call(args("sql" to "select 1")) }
        assertTrue(e.message!!.contains("jdbc:postgresql://db.local/shop"), e.message)
        assertFalse(e.message!!.contains("s3cret"), e.message)
        assertFalse(e.message!!.contains("password"), e.message)
    }

    @Test
    fun `redacted urls drop query and semicolon parameters`() {
        assertEquals("jdbc:postgresql://h/db", redactJdbcUrl("jdbc:postgresql://h/db?password=x"))
        assertEquals("jdbc:sqlserver://h:1433", redactJdbcUrl("jdbc:sqlserver://h:1433;password=x;user=y"))
        assertEquals("jdbc:sqlite:/tmp/a.db", redactJdbcUrl("jdbc:sqlite:/tmp/a.db"))
    }
}
