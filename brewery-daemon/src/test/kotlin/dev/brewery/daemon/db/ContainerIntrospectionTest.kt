package dev.brewery.daemon.db

import org.junit.jupiter.api.AfterAll
import org.junit.jupiter.api.BeforeAll
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.TestInstance
import org.junit.jupiter.api.condition.EnabledIf
import org.testcontainers.DockerClientFactory
import org.testcontainers.mariadb.MariaDBContainer
import org.testcontainers.postgresql.PostgreSQLContainer
import java.nio.file.Files
import java.sql.DriverManager
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/** True when a Docker daemon answers; the container tests below are skipped otherwise (CI ubuntu runs them). */
object DockerSupport {
    @JvmStatic
    fun available(): Boolean = try {
        DockerClientFactory.instance().isDockerAvailable
    } catch (_: Throwable) {
        false
    }
}

@EnabledIf("dev.brewery.daemon.db.DockerSupport#available", disabledReason = "Docker is not available")
@TestInstance(TestInstance.Lifecycle.PER_CLASS)
class PostgresIntrospectionTest {
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
                st.executeUpdate("CREATE TABLE app.pets (id int PRIMARY KEY, owner_id int NOT NULL, CONSTRAINT pets_owner_fk FOREIGN KEY (owner_id) REFERENCES app.owners (id))")
                st.executeUpdate("CREATE UNIQUE INDEX owners_email_idx ON app.owners (email)")
                st.executeUpdate("CREATE VIEW app.owner_names AS SELECT name FROM app.owners")
                st.executeUpdate("CREATE SEQUENCE app.ticket_seq")
            }
        }
        service = DbService(Files.createTempDirectory("brewery-pg"), InMemorySecretStore())
        id = service.save(ConnectionInfo("", "pg", "postgresql", postgres.jdbcUrl, postgres.username, false, emptyMap()), postgres.password).id
    }

    @AfterAll
    fun stop() {
        if (::service.isInitialized) service.close()
        postgres.stop()
    }

    @Test
    fun `schemas without system ones unless asked`() {
        val nodes = service.introspect(id, emptyList())
        val names = nodes.map { it.name }
        assertTrue("public" in names && "app" in names, names.toString())
        assertFalse("pg_catalog" in names || "information_schema" in names, names.toString())
        assertTrue(nodes.all { it.kind == "schema" && it.hasChildren })
        val all = service.introspect(id, emptyList(), includeSystem = true).map { it.name }
        assertTrue("pg_catalog" in all && "information_schema" in all, all.toString())
    }

    @Test
    fun `tables views and sequences under a schema`() {
        val nodes = service.introspect(id, listOf("app"))
        assertEquals(
            listOf("owners" to "table", "pets" to "table", "owner_names" to "view"),
            nodes.filter { it.kind != "sequence" }.map { it.name to it.kind },
        )
        val sequences = nodes.filter { it.kind == "sequence" }.map { it.name }
        assertTrue("ticket_seq" in sequences && "owners_id_seq" in sequences, sequences.toString())
        assertEquals(listOf("app", "ticket_seq"), nodes.first { it.name == "ticket_seq" }.path)
    }

    @Test
    fun `columns indexes and foreign keys`() {
        val cols = service.introspect(id, listOf("app", "owners", "columns"))
        assertEquals(listOf("id", "name", "email"), cols.map { it.name })
        assertTrue(cols[0].detail!!.endsWith(" NOT NULL PK"), cols[0].detail)
        assertEquals("varchar(40) NOT NULL", cols[1].detail)
        assertEquals("text NULL", cols[2].detail)
        val unique = service.introspect(id, listOf("app", "owners", "indexes")).single { it.name == "owners_email_idx" }
        assertEquals("UNIQUE (email)", unique.detail)
        val fk = service.introspect(id, listOf("app", "pets", "foreignKeys")).single { it.name == "pets_owner_fk" }
        assertEquals("(owner_id) -> owners(id)", fk.detail)
    }
}

@EnabledIf("dev.brewery.daemon.db.DockerSupport#available", disabledReason = "Docker is not available")
@TestInstance(TestInstance.Lifecycle.PER_CLASS)
class MariaDbIntrospectionTest {
    private val maria = MariaDBContainer("mariadb:11.4")
    private lateinit var service: DbService
    private lateinit var id: String

    @BeforeAll
    fun start() {
        maria.start()
        DriverManager.getConnection(maria.jdbcUrl, maria.username, maria.password).use { c ->
            c.createStatement().use { st ->
                st.executeUpdate("CREATE DATABASE IF NOT EXISTS shop")
                st.executeUpdate("CREATE TABLE shop.customers (id INT NOT NULL PRIMARY KEY, name VARCHAR(40) NOT NULL, note TEXT)")
                st.executeUpdate("CREATE TABLE shop.orders (id INT NOT NULL PRIMARY KEY, customer_id INT NOT NULL, CONSTRAINT orders_customer_fk FOREIGN KEY (customer_id) REFERENCES shop.customers (id))")
                st.executeUpdate("CREATE VIEW shop.customer_names AS SELECT name FROM shop.customers")
            }
        }
        service = DbService(Files.createTempDirectory("brewery-maria"), InMemorySecretStore())
        id = service.save(ConnectionInfo("", "maria", "mariadb", maria.jdbcUrl, maria.username, false, emptyMap()), maria.password).id
    }

    @AfterAll
    fun stop() {
        if (::service.isInitialized) service.close()
        maria.stop()
    }

    @Test
    fun `catalogs are the databases without system ones`() {
        val nodes = service.introspect(id, emptyList())
        val names = nodes.map { it.name }
        assertTrue("shop" in names, names.toString())
        assertFalse("information_schema" in names || "mysql" in names || "performance_schema" in names, names.toString())
        assertTrue(nodes.all { it.kind == "catalog" && it.hasChildren })
        assertTrue("mysql" in service.introspect(id, emptyList(), includeSystem = true).map { it.name })
    }

    @Test
    fun `tables columns and foreign keys under a catalog`() {
        assertEquals(
            listOf("customers" to "table", "orders" to "table", "customer_names" to "view"),
            service.introspect(id, listOf("shop")).map { it.name to it.kind },
        )
        val cols = service.introspect(id, listOf("shop", "customers", "columns"))
        assertEquals(listOf("INT NOT NULL PK", "VARCHAR(40) NOT NULL", "TEXT NULL"), cols.map { it.detail })
        val fk = service.introspect(id, listOf("shop", "orders", "foreignKeys")).single()
        assertEquals("orders_customer_fk", fk.name)
        assertEquals("(customer_id) -> customers(id)", fk.detail)
    }
}
