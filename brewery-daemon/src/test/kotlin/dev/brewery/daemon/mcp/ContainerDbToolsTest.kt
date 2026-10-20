package dev.brewery.daemon.mcp

import com.google.gson.JsonObject
import org.junit.jupiter.api.AfterAll
import org.junit.jupiter.api.BeforeAll
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.TestInstance
import org.junit.jupiter.api.condition.EnabledIf
import org.testcontainers.postgresql.PostgreSQLContainer
import java.nio.file.Files
import java.sql.DriverManager
import java.sql.SQLException
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

@EnabledIf("dev.brewery.daemon.db.DockerSupport#available", disabledReason = "Docker is not available")
@TestInstance(TestInstance.Lifecycle.PER_CLASS)
class ContainerDbToolsTest {
    private val postgres = PostgreSQLContainer("postgres:16-alpine")
    private lateinit var tools: Map<String, McpTool>

    @BeforeAll
    fun start() {
        postgres.start()
        DriverManager.getConnection(postgres.jdbcUrl, postgres.username, postgres.password).use { c ->
            c.createStatement().use { st ->
                st.executeUpdate("create table pet(id serial primary key, name text)")
                st.executeUpdate("insert into pet(name) values ('rex'), ('tom'), ('kesha')")
            }
        }
        val root = Files.createTempDirectory("dbtools-pg")
        val res = root.resolve("src/main/resources")
        Files.createDirectories(res)
        Files.writeString(res.resolve("application.yml"), """
            spring:
              datasource:
                url: ${'$'}{PG_URL}
                username: ${'$'}{PG_USER}
                password: ${'$'}{PG_PASSWORD}
        """.trimIndent() + "\n")
        val env = mapOf("PG_URL" to postgres.jdbcUrl, "PG_USER" to postgres.username, "PG_PASSWORD" to postgres.password)
        tools = DbTools(root, env = env::get).tools().associateBy { it.name }
    }

    @AfterAll
    fun stop() {
        postgres.stop()
    }

    private fun query(sql: String): QueryResult =
        tools.getValue("db_query").call(JsonObject().apply { addProperty("sql", sql) }) as QueryResult

    @Test
    fun `reads rows and rejects every write`() {
        assertEquals(3, query("select name from pet order by id").rows.size)
        assertFailsWith<McpToolError> { query("delete from pet") }
        query("select pg_catalog.set_config('x.y','1',false)")
        assertFailsWith<McpToolError> { query("with d as (delete from pet returning 1) select * from d") }
        // SqlText calls this a read, but it writes the sequence: the READ ONLY transaction must refuse it.
        val nextval = assertFailsWith<SQLException> { query("select nextval('pet_id_seq')") }
        assertTrue(nextval.message!!.contains("read-only", ignoreCase = true), nextval.message)
        assertEquals("3", query("select count(*) from pet").rows[0][0])
    }
}
