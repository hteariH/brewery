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
    private val clock: () -> Long = System::currentTimeMillis,
    idleTimeoutMs: Long = SessionManager.DEFAULT_IDLE_TIMEOUT_MS,
    /** Whether a connection needs autocommit off to stream result sets; PostgreSQL does. Tests force it on. */
    private val manageTransactions: (Connection) -> Boolean = ::isPostgres,
    /** How long an implicit streaming transaction may sit without a fetch before it is ended. */
    private val streamingTtlMs: Long = CursorManager.DEFAULT_STREAM_TTL_MS,
    /** Whether unfinished cursors must be ended before any other command (MariaDB/MySQL driver behaviour). */
    private val expireUnfinishedBeforeCommands: (Connection) -> Boolean = ::isMariaDb,
    /** Most tables and views one `db/schema` snapshot holds; tests inject a small value. */
    private val maxSchemaTables: Int = SchemaSnapshot.MAX_TABLES,
    /** Whether snapshots of a saved connection are built on a separate connection (its driver drains pending results); decided without connecting. */
    private val snapshotOnSeparateConnection: (ConnectionInfo) -> Boolean = { isMariaDbUrl(it.url) },
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
        forgetSchema(saved.id)
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
        forgetSchema(id)
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
        sessions.runIn(connectionId) { session, c ->
            cursorManagers[connectionId]?.takeIf { it.session === session }?.expireUnfinishedIfNeeded(c)
            try {
                Introspector(c, includeSystem).children(path)
            } catch (e: SQLException) {
                throw queryFailure(e)
            }
        }

    private val schemas = SchemaCache()
    /** How many snapshots were built (cache misses and refreshes); tests use it to prove a cache hit. */
    internal val schemaBuilds = java.util.concurrent.atomic.AtomicInteger()

    private val metadataThreads = ConcurrentHashMap<String, java.util.concurrent.ExecutorService>()
    @Volatile private var closing = false

    /** Opens the connection a snapshot is built on when it must not use the session; tests replace it. */
    internal var openMetadataConnection: (String) -> Connection = ::openSaved

    /** Number of per-connection snapshot threads alive; only connections built on a separate connection have one. */
    internal val metadataThreadCount: Int get() = metadataThreads.size

    /**
     * The completion snapshot of connection [connectionId] (see [Introspector.snapshot]), cached until [refresh],
     * DDL through [execute], or save/delete of the connection. A cache hit does not touch any connection, and
     * concurrent misses share one build.
     *
     * Completion calls this automatically, so a build must not end streaming cursors. The path is chosen from
     * the saved connection (its URL), without connecting. Where the driver drains a pending streaming result when
     * another command runs (MariaDB/MySQL) the snapshot is built on a short-lived separate connection (same saved
     * settings, closed afterwards, network timeout [METADATA_TIMEOUT_MS]) on a thread of its own per connection,
     * created only for such connections; the session and its cursors are not touched. If that connection cannot be
     * opened (server connection limit, ...) the build falls back to the session, which then ends the session's
     * unfinished cursors exactly like [introspect] does (fetch -> -32013). Every other database (PostgreSQL,
     * SQLite, generic) builds on the session thread: metadata queries leave PostgreSQL portals and the streaming
     * transaction alone, and a separate connection would not see a private SQLite database such as `:memory:`.
     */
    fun schema(connectionId: String, refresh: Boolean?): DbSchema =
        schemas.get(connectionId, refresh == true) { buildSnapshot(connectionId) }

    private fun buildSnapshot(id: String): DbSchema {
        val saved = store.connection(id) ?: throw DbException(DbErrorCodes.NOT_FOUND, "Unknown connection: $id")
        if (snapshotOnSeparateConnection(saved)) {
            onMetadataThread(id) { snapshotOf(it) }?.let { return it }
        }
        return sessions.runIn(id) { session, c ->
            cursorManagers[id]?.takeIf { it.session === session }?.expireUnfinishedIfNeeded(c)
            snapshotOf(c)
        }
    }

    private fun snapshotOf(c: Connection): DbSchema = try {
        schemaBuilds.incrementAndGet()
        Introspector(c).snapshot(maxSchemaTables)
    } catch (e: SQLException) {
        throw queryFailure(e)
    }

    /**
     * Runs [block] with a fresh connection of saved connection [id] on that id's metadata thread and closes it
     * afterwards. Null when the connection cannot be opened (the caller falls back); other failures propagate.
     */
    private fun onMetadataThread(id: String, block: (Connection) -> DbSchema): DbSchema? {
        val task = Callable<DbSchema?> {
            val c = try {
                openMetadataConnection(id)
            } catch (e: DbException) {
                if (e.code == DbErrorCodes.CONNECTION_FAILED || e.code == DbErrorCodes.AUTH_FAILED) return@Callable null
                throw e
            }
            c.use {
                runCatching { it.setNetworkTimeout({ r -> r.run() }, METADATA_TIMEOUT_MS) }
                block(it)
            }
        }
        val future = try {
            submitMetadata(id, task)
        } catch (_: java.util.concurrent.RejectedExecutionException) {
            // Raced with save/delete shutting this id's thread down: one retry on a fresh one.
            try {
                if (closing) throw java.util.concurrent.RejectedExecutionException()
                submitMetadata(id, task)
            } catch (_: java.util.concurrent.RejectedExecutionException) {
                throw DbException(DbErrorCodes.CONNECTION_FAILED, "The database service is shutting down")
            }
        }
        try {
            return future.get()
        } catch (e: ExecutionException) {
            throw e.cause ?: e
        }
    }

    private fun <T> submitMetadata(id: String, task: Callable<T>) =
        metadataThreads.computeIfAbsent(id) {
            Executors.newSingleThreadExecutor { r -> Thread(r, "brewery-db-metadata").apply { isDaemon = true } }
        }.submit(task)

    private fun forgetSchema(id: String) {
        schemas.invalidate(id)
        metadataThreads.remove(id)?.shutdown()
    }

    // cursor id -> the manager (one per live session) that owns it
    private val cursorIndex = ConcurrentHashMap<String, CursorManager>()
    private val cursorManagers = ConcurrentHashMap<String, CursorManager>()

    private fun cursorsOf(connectionId: String, session: Session): CursorManager =
        cursorManagers.compute(connectionId) { _, old ->
            old?.takeIf { it.session === session } ?: CursorManager(session, cursorIndex, manageTransactions, clock, streamingTtlMs, expireUnfinishedBeforeCommands).also { manager ->
                // A manager of a closed session would otherwise stay in the map (and keep the session alive).
                session.onClose { if (session.closed) cursorManagers.remove(connectionId, manager) }
            }
        }!!

    /** Runs [sql] on the session of [connectionId] and returns after the first result; see [CursorManager.execute]. */
    fun execute(connectionId: String, sql: String): DbExecuteResult = try {
        sessions.runIn(connectionId) { session, c -> cursorsOf(connectionId, session).execute(c, sql) }
    } catch (e: SQLException) {
        throw queryFailure(e)
    } finally {
        // Also after a failure: a script may have changed the schema before it failed.
        if (SqlText.isDdl(sql)) schemas.invalidate(connectionId)
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
        closing = true
        sessions.close()
        metadataThreads.values.forEach { it.shutdownNow() }
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
        const val METADATA_TIMEOUT_MS = 60_000
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

/** True for a MariaDB or MySQL JDBC URL, judged from the URL text alone. */
fun isMariaDbUrl(url: String): Boolean = url.startsWith("jdbc:mariadb:") || url.startsWith("jdbc:mysql:")

/** MariaDB and MySQL URLs (the driver is Connector/J for MariaDB); it drains pending streaming results before the next command. */
fun isMariaDb(connection: Connection): Boolean {
    return connection.metaData.url?.let(::isMariaDbUrl) == true
}
