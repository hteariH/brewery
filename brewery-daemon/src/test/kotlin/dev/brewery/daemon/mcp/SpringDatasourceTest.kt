package dev.brewery.daemon.mcp

import org.junit.jupiter.api.Test
import org.junit.jupiter.api.assertThrows
import java.nio.file.Files
import java.nio.file.Path
import kotlin.test.assertEquals
import kotlin.test.assertNull

class SpringDatasourceTest {
    private fun project(vararg files: Pair<String, String>): Path {
        val root = Files.createTempDirectory("ds")
        for ((path, text) in files) {
            val file = root.resolve(path)
            Files.createDirectories(file.parent)
            Files.writeString(file, text)
        }
        return root
    }

    private val res = "src/main/resources"

    @Test
    fun `reads application yml of the default profile`() {
        val root = project("$res/application.yml" to """
            spring:
              datasource:
                url: "jdbc:postgresql://localhost/shop"   # local db
                username: shop
                password: 'secret'
            ---
            spring:
              config.activate.on-profile: prod
              datasource:
                url: jdbc:postgresql://prod/shop
        """.trimIndent())
        val ds = SpringDatasource.find(root) { null }!!
        assertEquals("jdbc:postgresql://localhost/shop", ds.url)
        assertEquals("shop", ds.username)
        assertEquals("secret", ds.password)
    }

    @Test
    fun `reads application properties and resolves placeholders`() {
        val root = project("$res/application.properties" to """
            spring.datasource.url=jdbc:postgresql://${'$'}{DB_HOST:localhost}:5432/shop
            spring.datasource.username=${'$'}{DB_USER}
        """.trimIndent())
        val ds = SpringDatasource.find(root) { if (it == "DB_USER") "alice" else null }!!
        assertEquals("jdbc:postgresql://localhost:5432/shop", ds.url)
        assertEquals("alice", ds.username)
        assertNull(ds.password)
    }

    @Test
    fun `unresolvable placeholder is a tool error`() {
        val root = project("$res/application.properties" to "spring.datasource.url=jdbc:postgresql://\${DB_HOST}/shop")
        val e = assertThrows<McpToolError> { SpringDatasource.find(root) { null } }
        assertEquals(true, e.message!!.contains("DB_HOST"))
    }

    @Test
    fun `root module wins over submodules and build dirs are skipped`() {
        val root = project(
            "api/$res/application.properties" to "spring.datasource.url=jdbc:sqlite:api.db",
            "$res/application.yml" to "spring.datasource.url: jdbc:sqlite:root.db",
            "build/resources/main/application.properties" to "spring.datasource.url=jdbc:sqlite:stale.db",
        )
        assertEquals("jdbc:sqlite:root.db", SpringDatasource.find(root) { null }!!.url)
        Files.delete(root.resolve("$res/application.yml"))
        assertEquals("jdbc:sqlite:api.db", SpringDatasource.find(root) { null }!!.url)
    }

    @Test
    fun `no datasource gives null`() {
        assertNull(SpringDatasource.find(project("$res/application.yml" to "server.port: 8080")) { null })
    }
}
