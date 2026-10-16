package dev.brewery.daemon.db

import org.junit.jupiter.api.AfterAll
import org.junit.jupiter.api.BeforeAll
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.TestInstance
import org.junit.jupiter.api.condition.EnabledIf
import org.testcontainers.mariadb.MariaDBContainer
import org.testcontainers.postgresql.PostgreSQLContainer
import java.nio.file.Files
import java.sql.DriverManager
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

@EnabledIf("dev.brewery.daemon.db.DockerSupport#available", disabledReason = "Docker is not available")
@TestInstance(TestInstance.Lifecycle.PER_CLASS)
class PostgresSchemaTest {
    private val postgres = PostgreSQLContainer("postgres:16-alpine")
    private lateinit var service: DbService
    private lateinit var id: String

    @BeforeAll
    fun start() {
        postgres.start()
        DriverManager.getConnection(postgres.jdbcUrl, postgres.username, postgres.password).use { c ->
            c.createStatement().use { st ->
                st.executeUpdate("CREATE SCHEMA app")
                st.executeUpdate("CREATE TABLE app.owners (id serial PRIMARY KEY, name varchar(40) NOT NULL, email text)")
                st.executeUpdate("CREATE VIEW app.owner_names AS SELECT name FROM app.owners")
                st.executeUpdate("CREATE TABLE public.pets (id int PRIMARY KEY, nick text)")
                st.executeUpdate("CREATE SCHEMA a_b")
                st.executeUpdate("CREATE SCHEMA axb")
                st.executeUpdate("CREATE TABLE a_b.t (only_in_a_b int)")
                st.executeUpdate("CREATE TABLE axb.t (only_in_axb int)")
            }
        }
        service = DbService(Files.createTempDirectory("brewery-pg-schema"), InMemorySecretStore())
        id = service.save(ConnectionInfo("", "pg", "postgresql", postgres.jdbcUrl, postgres.username, false, emptyMap()), postgres.password).id
    }

    @AfterAll
    fun stop() {
        if (::service.isInitialized) service.close()
        postgres.stop()
    }

    @Test
    fun `two schemas with their tables and no system schemas`() {
        val snapshot = service.schema(id, true)
        val names = snapshot.schemas.map { it.name }
        assertTrue("public" in names && "app" in names, names.toString())
        assertFalse("pg_catalog" in names || "information_schema" in names, names.toString())
        val app = snapshot.schemas.first { it.name == "app" }
        assertEquals(listOf("owners" to "table", "owner_names" to "view"), app.tables.map { it.name to it.kind })
        assertEquals(listOf("id" to "serial", "name" to "varchar(40)", "email" to "text"), app.tables[0].columns.map { it.name to it.type })
        assertEquals(listOf("pets"), snapshot.schemas.first { it.name == "public" }.tables.map { it.name })
        assertEquals(listOf("only_in_a_b"), snapshot.schemas.first { it.name == "a_b" }.tables.single().columns.map { it.name })
        assertEquals(listOf("only_in_axb"), snapshot.schemas.first { it.name == "axb" }.tables.single().columns.map { it.name })
    }

    @Test
    fun `a small limit is shared between schemas instead of starving later ones`() {
        val small = DbService(Files.createTempDirectory("brewery-pg-schema-small"), InMemorySecretStore(), maxSchemaTables = 4)
        try {
            val sid = small.save(ConnectionInfo("", "pg", "postgresql", postgres.jdbcUrl, postgres.username, false, emptyMap()), postgres.password).id
            val snapshot = small.schema(sid, false)
            assertTrue(snapshot.truncated)
            assertEquals(4, snapshot.schemas.sumOf { it.tables.size })
            assertEquals(listOf("pets"), snapshot.schemas.first { it.name == "public" }.tables.map { it.name })
            assertEquals(1, snapshot.schemas.first { it.name == "a_b" }.tables.size)
            assertEquals(1, snapshot.schemas.first { it.name == "app" }.tables.size)
            assertTrue(snapshot.schemas.first { it.name == "app" }.tables.single().columns.isNotEmpty())
        } finally {
            small.close()
        }
    }

    @Test
    fun `building the snapshot leaves an open streaming cursor alive`() {
        service.schema(id, true)
        val cursor = service.execute(id, "SELECT g FROM generate_series(1, 5000) g").cursorId!!
        service.schema(id, true)
        assertEquals(10, service.fetch(cursor, 0, 10).rows.size)
        service.closeCursor(cursor)
    }
}

@EnabledIf("dev.brewery.daemon.db.DockerSupport#available", disabledReason = "Docker is not available")
@TestInstance(TestInstance.Lifecycle.PER_CLASS)
class MariaDbSchemaTest {
    private val maria = MariaDBContainer("mariadb:11.4").withUsername("root")
    private lateinit var service: DbService
    private lateinit var id: String

    @BeforeAll
    fun start() {
        maria.start()
        DriverManager.getConnection(maria.jdbcUrl, maria.username, maria.password).use { c ->
            c.createStatement().use { st ->
                st.executeUpdate("CREATE DATABASE IF NOT EXISTS shop")
                st.executeUpdate("CREATE TABLE shop.customers (id INT NOT NULL PRIMARY KEY, name VARCHAR(40) NOT NULL, note TEXT)")
                st.executeUpdate("CREATE VIEW shop.customer_names AS SELECT name FROM shop.customers")
            }
        }
        service = DbService(Files.createTempDirectory("brewery-maria-schema"), InMemorySecretStore())
        id = service.save(ConnectionInfo("", "maria", "mariadb", maria.jdbcUrl, maria.username, false, emptyMap()), maria.password).id
    }

    @AfterAll
    fun stop() {
        if (::service.isInitialized) service.close()
        maria.stop()
    }

    @Test
    fun `building the snapshot leaves an unfinished streaming cursor alive`() {
        val cursor = service.execute(id, "SELECT seq FROM seq_1_to_5000").cursorId!!
        assertEquals(10, service.fetch(cursor, 0, 10).rows.size)
        service.schema(id, true)
        service.schema(id, true)
        assertEquals(10, service.fetch(cursor, 10, 10).rows.size)
        service.closeCursor(cursor)
    }

    @Test
    fun `expanding the schema tree leaves an unfinished streaming cursor alive`() {
        val cursor = service.execute(id, "SELECT seq FROM seq_1_to_5000").cursorId!!
        assertEquals(10, service.fetch(cursor, 0, 10).rows.size)
        assertTrue("shop" in service.introspect(id, emptyList()).map { it.name })
        assertEquals(listOf("id", "name", "note"), service.introspect(id, listOf("shop", "customers", "columns")).map { it.name })
        assertEquals(10, service.fetch(cursor, 10, 10).rows.size)
        service.closeCursor(cursor)
    }

    @Test
    fun `catalogs are the schemas without system databases`() {
        val snapshot = service.schema(id, true)
        val names = snapshot.schemas.map { it.name }
        assertTrue("shop" in names, names.toString())
        assertFalse("information_schema" in names || "mysql" in names || "performance_schema" in names, names.toString())
        val shop = snapshot.schemas.first { it.name == "shop" }
        assertEquals(listOf("customers" to "table", "customer_names" to "view"), shop.tables.map { it.name to it.kind })
        assertEquals(listOf("INT", "VARCHAR(40)", "TEXT"), shop.tables[0].columns.map { it.type.uppercase() })
    }
}
