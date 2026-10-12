package dev.brewery.daemon.db

import java.nio.file.Path
import java.sql.Connection
import java.sql.SQLException
import java.util.Properties
import java.util.concurrent.Callable
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ExecutionException
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException

/** Connection management of one project: `db.json`, the OS secret store and the driver registry. */
class DbService(
    root: Path,
    private val secrets: SecretStore,
    clock: () -> Long = System::currentTimeMillis,
    idleTimeoutMs: Long = SessionManager.DEFAULT_IDLE_TIMEOUT_MS,
    /** Whether a connection needs autocommit off to stream result sets; PostgreSQL does. Tests force it on. */
    private val manageTransactions: (Connection) -> Boolean = ::isPostgres,
) : AutoCloseable {
    private val projectKey: String = try {
        root.toRealPath().toString()
    } catch (_: Exception) {
        root.toAbsolutePath().normalize().toString()
    }
    private val store = ConnectionStore(root)
    val registry = DriverRegistry(store)
    /** One thread and one connection per saved connection id; Task 3 runs execute/fetch/cancel through it. */
    val sessions = SessionManager({ store.connection(it) != null }, ::openSaved, clock, idleTimeoutMs)
    private val testThreads = Executors.newCachedThreadPool { r -> Thread(r, "brewery-db-test").apply { isDaemon = true } }

    private fun account(id: String) = "$projectKey#$id"

    fun drivers(): List<DriverInfo> = registry.list()

    fun addJar(driverId: String, name: String, className: String, jarPaths: List<String>): DriverInfo =
        registry.addJar(driverId, name, className, jarPaths)

    fun list(): List<ConnectionInfo> = store.connections().map { it.copy(hasPassword = secrets.get(account(it.id)) != null) }

    fun save(connection: ConnectionInfo, password: String?): ConnectionInfo {
        if (registry.list().none { it.id == connection.driverId }) {
            throw DbException(DbErrorCodes.NOT_FOUND, "Unknown driver: ${connection.driverId}")
        }
        val saved = store.saveConnection(connection)
        // URL, user or options may have changed: the next use opens a fresh session.
        sessions.close(saved.id)
        var warning: String? = null
        try {
            when {
                password == null -> {}
                // Removing from a store that cannot be used is fine: nothing was stored there.
                password.isEmpty() -> try {
                    secrets.remove(account(saved.id))
                } catch (_: SecretStoreUnavailable) {
                }
                else -> secrets.set(account(saved.id), password)
            }
        } catch (e: SecretStoreUnavailable) {
            warning = "Password was not stored: ${e.message}"
        }
        return saved.copy(hasPassword = secrets.get(account(saved.id)) != null, warning = warning)
    }

    fun delete(id: String) {
        if (!store.deleteConnection(id)) throw DbException(DbErrorCodes.NOT_FOUND, "Unknown connection: $id")
        sessions.close(id)
        try {
            secrets.remove(account(id))
        } catch (_: SecretStoreUnavailable) {
            // Nothing could have been stored there.
        }
    }

    /** Never throws for connection problems: they come back as `ok = false` with the driver's message. */
    fun test(connection: ConnectionInfo, password: String?): DbTestResult {
        val effective = password?.takeIf { it.isNotEmpty() }
            ?: connection.id.takeIf { it.isNotBlank() && password == null }?.let { secrets.get(account(it)) }
        val task = testThreads.submit(Callable {
            openConnection(connection.driverId, connection.url, connection.user, effective, connection.options).use { c ->
                val meta = c.metaData
                "${meta.databaseProductName} ${meta.databaseProductVersion}".trim()
            }
        })
        return try {
            val version = task.get(TEST_TIMEOUT_SECONDS, TimeUnit.SECONDS)
            DbTestResult(true, "Connected to $version", version)
        } catch (_: TimeoutException) {
            task.cancel(true)
            DbTestResult(false, "Connection timed out after $TEST_TIMEOUT_SECONDS seconds", null)
        } catch (e: ExecutionException) {
            DbTestResult(false, describe(e.cause ?: e), null)
        }
    }

    /** Opens the saved connection [id] for sessions; failures become `DB_AUTH_FAILED` / `DB_CONNECTION_FAILED`. */
    fun openSaved(id: String): Connection {
        val c = store.connection(id) ?: throw DbException(DbErrorCodes.NOT_FOUND, "Unknown connection: $id")
        return try {
            openConnection(c.driverId, c.url, c.user, secrets.get(account(id)), c.options)
        } catch (e: DbException) {
            throw e
        } catch (e: Exception) {
            throw connectionFailure(e)
        }
    }

    /** Opens a JDBC connection through the driver itself, not `DriverManager`. */
    fun openConnection(
        driverId: String,
        url: String,
        user: String?,
        password: String?,
        options: Map<String, String>,
    ): Connection {
        val driver = registry.driver(driverId)
        val props = Properties()
        (options ?: emptyMap()).forEach { (k, v) -> props.setProperty(k, v) }
        if (!user.isNullOrEmpty()) props.setProperty("user", user)
        if (!password.isNullOrEmpty()) props.setProperty("password", password)
        // MariaDB Connector/J 3 only accepts the jdbc:mysql: scheme with an extra flag; users expect it to work.
        val effectiveUrl = if (driverId == "mysql" && url.startsWith("jdbc:mysql:")) "jdbc:mariadb:" + url.removePrefix("jdbc:mysql:") else url
        return driver.connect(effectiveUrl, props)
            ?: throw SQLException("The ${driverId} driver does not accept this URL", "08001")
    }

    /** Lazily lists the children of [path] in the schema tree of connection [connectionId]. */
    fun introspect(connectionId: String, path: List<String>, includeSystem: Boolean = false): List<DbNode> =
        sessions.run(connectionId) { c ->
            try {
                Introspector(c, includeSystem).children(path)
            } catch (e: SQLException) {
                throw queryFailure(e)
            }
        }

    // cursor id -> the manager (one per live session) that owns it
    private val cursorIndex = ConcurrentHashMap<String, CursorManager>()
    private val cursorManagers = ConcurrentHashMap<String, CursorManager>()

    private fun cursorsOf(connectionId: String, session: Session): CursorManager =
        cursorManagers.compute(connectionId) { _, old ->
            old?.takeIf { it.session === session } ?: CursorManager(session, cursorIndex, manageTransactions).also { manager ->
                // A manager of a closed session would otherwise stay in the map (and keep the session alive).
                session.onClose { if (session.closed) cursorManagers.remove(connectionId, manager) }
            }
        }!!

    /** Runs [sql] on the session of [connectionId] and returns after the first result; see [CursorManager.execute]. */
    fun execute(connectionId: String, sql: String): DbExecuteResult = try {
        sessions.runIn(connectionId) { session, c -> cursorsOf(connectionId, session).execute(c, sql) }
    } catch (e: SQLException) {
        throw queryFailure(e)
    }

    fun fetch(cursorId: String, offset: Int, limit: Int): DbFetchResult {
        val manager = cursorIndex[cursorId] ?: throw DbException(DbErrorCodes.NOT_FOUND, "Unknown cursor: $cursorId")
        return try {
            manager.session.run { manager.fetch(cursorId, offset.toLong(), limit) }
        } catch (_: SessionClosedException) {
            throw DbException(DbErrorCodes.NOT_FOUND, "Unknown cursor: $cursorId")
        } catch (e: SQLException) {
            throw queryFailure(e)
        }
    }

    /** Aborts the statement currently running on the session; a no-op when nothing runs. Works from any thread. */
    fun cancel(connectionId: String?, cursorId: String?) {
        val id = connectionId?.takeIf { it.isNotBlank() }
            ?: cursorId?.let { cursorIndex[it] }?.let { m -> cursorManagers.entries.firstOrNull { it.value === m }?.key }
            ?: throw DbException(DbErrorCodes.NOT_FOUND, "Unknown connection: ${connectionId ?: ""}")
        sessions.session(id).cancelRunning()
    }

    /** Idempotent: unknown or already evicted cursors are ignored. */
    fun closeCursor(cursorId: String) {
        val manager = cursorIndex[cursorId] ?: return
        try {
            manager.session.run { manager.close(cursorId) }
        } catch (_: SessionClosedException) {
            // The session's close listener already released everything.
        }
    }

    override fun close() {
        sessions.close()
        testThreads.shutdownNow()
        registry.close()
    }

    private fun describe(error: Throwable): String {
        val text = error.message?.trim().takeUnless { it.isNullOrEmpty() } ?: error.javaClass.simpleName
        val state = (error as? SQLException)?.sqlState
        return (if (state != null) "$text (SQLState $state)" else text).take(MAX_MESSAGE)
    }

    private companion object {
        const val TEST_TIMEOUT_SECONDS = 20L
        const val MAX_MESSAGE = 2000
    }
}

/** Maps a connection failure to the plan's domain codes: SQLState `28*` is an authentication failure. */
fun connectionFailure(error: Throwable): DbException {
    val state = (error as? SQLException)?.sqlState
    val code = if (state != null && state.startsWith("28")) DbErrorCodes.AUTH_FAILED else DbErrorCodes.CONNECTION_FAILED
    return DbException(code, error.message ?: error.javaClass.simpleName)
}

/** PostgreSQL only streams a result set (fetch size) inside a transaction. */
fun isPostgres(connection: Connection): Boolean = connection.metaData.url?.startsWith("jdbc:postgresql:") == true
