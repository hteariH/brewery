package dev.brewery.daemon.db

import dev.brewery.daemon.BreweryClient
import dev.brewery.daemon.BreweryDaemon
import dev.brewery.daemon.BreweryServer
import dev.brewery.daemon.ErrorCodes
import dev.brewery.daemon.OpenProjectParams
import dev.brewery.daemon.ProjectChanged
import dev.brewery.daemon.SpringChanged
import org.eclipse.lsp4j.jsonrpc.Launcher
import org.eclipse.lsp4j.jsonrpc.ResponseErrorException
import org.eclipse.lsp4j.jsonrpc.json.MessageJsonHandler
import java.io.File
import java.io.PipedInputStream
import java.io.PipedOutputStream
import java.nio.file.Files
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ExecutionException
import java.util.concurrent.TimeUnit
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class DbProtocolTest {
    private val secrets = InMemorySecretStore()
    private val daemon = BreweryDaemon(secretStore = secrets)
    private val server: BreweryServer
    private val gson = MessageJsonHandler(emptyMap()).gson

    init {
        val c2s = PipedOutputStream()
        val serverIn = PipedInputStream(c2s)
        val s2c = PipedOutputStream()
        val clientIn = PipedInputStream(s2c)
        val serverLauncher = Launcher.createLauncher(daemon, BreweryClient::class.java, serverIn, s2c)
        daemon.connect(serverLauncher.remoteProxy)
        serverLauncher.startListening()
        val clientLauncher = Launcher.createLauncher(object : BreweryClient {
            override fun springChanged(params: SpringChanged) {}
            override fun projectChanged(params: ProjectChanged) {}
        }, BreweryServer::class.java, clientIn, c2s)
        clientLauncher.startListening()
        server = clientLauncher.remoteProxy
    }

    @AfterTest
    fun tearDown() = daemon.close()

    private fun <T> await(f: CompletableFuture<T>): T = f.get(20, TimeUnit.SECONDS)

    private fun errorCode(f: CompletableFuture<*>): Int {
        val e = assertFailsWith<ExecutionException> { f.get(20, TimeUnit.SECONDS) }
        return (e.cause as ResponseErrorException).responseError.code
    }

    private fun open(): java.nio.file.Path {
        val root = Files.createTempDirectory("brewery-db-proto")
        await(server.openProject(OpenProjectParams(root.toString())))
        return root
    }

    @Test
    fun `db methods need an open project`() {
        assertEquals(ErrorCodes.NOT_OPENED, errorCode(server.dbConnectionsList(DbConnectionsListParams())))
    }

    @Test
    fun `drivers over the wire`() {
        open()
        val drivers = await(server.dbDrivers(DbDriversParams()))
        assertEquals(listOf("postgresql", "mysql", "mariadb", "sqlite"), drivers.filter { it.bundled }.map { it.id })
    }

    @Test
    fun `save list test delete over the wire`() {
        val root = open()
        val file = Files.createTempFile("brewery", ".db")
        val info = ConnectionInfo("", "Lite", "sqlite", "jdbc:sqlite:$file", null, false, emptyMap())
        val saved = await(server.dbConnectionsSave(DbConnectionsSaveParams(info, "pw")))
        assertTrue(saved.id.isNotBlank())
        assertTrue(saved.hasPassword)
        assertEquals(listOf(saved), await(server.dbConnectionsList(DbConnectionsListParams())))
        assertEquals("pw", secrets.get("${root.toRealPath()}#${saved.id}"))

        val result = await(server.dbConnectionsTest(DbConnectionsTestParams(saved, null)))
        assertTrue(result.ok, result.message)

        val bad = await(server.dbConnectionsTest(DbConnectionsTestParams(saved.copy(url = "nonsense"), null)))
        assertFalse(bad.ok)

        await(server.dbConnectionsDelete(DbConnectionsDeleteParams(saved.id)))
        assertTrue(await(server.dbConnectionsList(DbConnectionsListParams())).isEmpty())
        assertEquals(null, secrets.get("${root.toRealPath()}#${saved.id}"))
    }

    @Test
    fun `unknown connection id is DB_NOT_FOUND over the wire`() {
        open()
        assertEquals(-32013, errorCode(server.dbConnectionsDelete(DbConnectionsDeleteParams("missing"))))
        val info = ConnectionInfo("", "X", "nope", "jdbc:x", null, false, emptyMap())
        assertEquals(-32013, errorCode(server.dbConnectionsSave(DbConnectionsSaveParams(info, null))))
    }

    @Test
    fun `addJar failure is DB_CONNECTION_FAILED`() {
        open()
        assertEquals(
            -32010,
            errorCode(server.dbDriversAddJar(DbDriversAddJarParams("u", "U", "a.B", listOf("/missing.jar")))),
        )
    }

    @Test
    fun `introspect over the wire`() {
        open()
        val file = Files.createTempFile("brewery", ".db")
        java.sql.DriverManager.getConnection("jdbc:sqlite:$file").use {
            it.createStatement().executeUpdate("CREATE TABLE t (id INTEGER NOT NULL PRIMARY KEY, v TEXT)")
        }
        val saved = await(server.dbConnectionsSave(DbConnectionsSaveParams(ConnectionInfo("", "L", "sqlite", "jdbc:sqlite:$file", null, false, emptyMap()), null)))
        assertEquals(listOf(DbNode("t", "table", listOf("t"), true)), await(server.dbIntrospect(DbIntrospectParams(saved.id, emptyList()))))
        val columns = await(server.dbIntrospect(DbIntrospectParams(saved.id, listOf("t", "columns"))))
        assertEquals(listOf("INTEGER NOT NULL PK", "TEXT NULL"), columns.map { it.detail })
        // A missing path means the root.
        assertEquals(1, await(server.dbIntrospect(DbIntrospectParams(saved.id, null))).size)
        assertEquals(-32013, errorCode(server.dbIntrospect(DbIntrospectParams("missing", emptyList()))))
        val bad = await(server.dbConnectionsSave(DbConnectionsSaveParams(saved.copy(id = "", url = "jdbc:sqlite:/no/such/dir/x.db"), null)))
        assertEquals(-32010, errorCode(server.dbIntrospect(DbIntrospectParams(bad.id, emptyList()))))
    }

    @Test
    fun `error code constants match the plan`() {
        assertEquals(-32010, DbErrorCodes.CONNECTION_FAILED)
        assertEquals(-32011, DbErrorCodes.AUTH_FAILED)
        assertEquals(-32012, DbErrorCodes.QUERY_FAILED)
        assertEquals(-32013, DbErrorCodes.NOT_FOUND)
    }

    @Test
    fun `fixtures deserialise and match the wire shape`() {
        fun fixture(name: String) = File("protocol/fixtures/$name").readText()
        val conn = gson.fromJson(fixture("db-connection.json"), ConnectionInfo::class.java)
        assertEquals("c1", conn.id)
        assertEquals(mapOf("ssl" to "true"), conn.options)
        val save = gson.fromJson(fixture("db-connections-save.params.json"), DbConnectionsSaveParams::class.java)
        assertEquals(null, save.password)
        val test = gson.fromJson(fixture("db-connections-test.result.json"), DbTestResult::class.java)
        assertTrue(test.ok)
        val drivers = gson.fromJson(fixture("db-drivers.result.json"), Array<DriverInfo>::class.java)
        assertEquals(4, drivers.size)
        val nodes = gson.fromJson(fixture("db-introspect.result.json"), Array<DbNode>::class.java)
        assertEquals(listOf("schema", "table", "folder", "column", "index", "foreignKey", "sequence"), nodes.map { it.kind })
        assertEquals(null, nodes[0].detail)
        assertEquals("serial NOT NULL PK", nodes[3].detail)
        assertEquals(listOf("public", "owners", "columns", "id"), nodes[3].path)
    }
}
