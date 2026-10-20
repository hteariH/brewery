package dev.brewery.daemon.db

import java.sql.Connection
import java.sql.SQLException
import java.sql.Statement
import java.util.concurrent.Callable
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.ExecutionException
import java.util.concurrent.Executors
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock

/** A query-side SQL failure as the plan's `DB_QUERY_FAILED`: driver message plus SQLState. Never includes SQL text. */
fun queryFailure(error: SQLException): DbException {
    val text = error.message?.trim().takeUnless { it.isNullOrEmpty() } ?: error.javaClass.simpleName
    val state = error.sqlState
    return DbException(DbErrorCodes.QUERY_FAILED, (if (state != null) "$text (SQLState $state)" else text).take(2000))
}

/** Thrown by [Session.run] when the session was closed between lookup and use; [SessionManager.run] retries. */
class SessionClosedException : RuntimeException("Session is closed")

/**
 * One database session: a single dedicated thread and one lazily opened [Connection]. Everything that touches
 * the connection (introspection now, execute/fetch/cancel later) goes through [run], so statements are
 * serialised and always see the same thread. Cancelling a running statement is the one thing that has to come
 * from another thread: use [currentConnection] for that.
 */
class Session(
    val id: String,
    private val opener: () -> Connection,
    private val clock: () -> Long,
) {
    private val executor = Executors.newSingleThreadExecutor { r -> Thread(r, "brewery-db-session").apply { isDaemon = true } }
    private val gate = ReentrantLock()
    private val busy = AtomicInteger()
    private val closeListeners = CopyOnWriteArrayList<() -> Unit>()
    private val sweepHooks = CopyOnWriteArrayList<Pair<() -> Boolean, (Connection) -> Unit>>()
    private val keepOpenConditions = CopyOnWriteArrayList<() -> Boolean>()

    @Volatile private var connection: Connection? = null
    @Volatile private var sessionThread: Thread? = null

    @Volatile private var running: Statement? = null
    private val runGeneration = AtomicLong()

    /**
     * Set around each execute and fetch so that [cancelRunning] (any thread) can reach the statement. Every
     * assignment starts a new "run", so a delayed cancel retry never hits a later operation on the same statement.
     */
    var runningStatement: Statement?
        get() = running
        set(value) {
            runGeneration.incrementAndGet()
            running = value
        }
    @Volatile private var lastUsed: Long = clock()
    @Volatile var closed: Boolean = false
        private set

    /** The open connection, or null before the first use and after close. Safe to read from any thread. */
    val currentConnection: Connection? get() = connection

    /**
     * Runs [block] on the session thread with the (lazily opened) connection and returns its result. Called from
     * the session thread itself (a nested call) it runs inline instead of deadlocking on its own queue. A
     * connection error (SQLState `08*`) drops the connection, so the next call reconnects.
     */
    fun <T> run(block: (Connection) -> T): T {
        if (Thread.currentThread() === sessionThread) return block(connection ?: throw SessionClosedException())
        val future = gate.withLock {
            if (closed) throw SessionClosedException()
            busy.incrementAndGet()
            try {
                executor.submit(Callable {
                    sessionThread = Thread.currentThread()
                    try {
                        val c = connection?.takeUnless { it.isClosed } ?: openNew()
                        try {
                            block(c)
                        } catch (e: SQLException) {
                            if (e.sqlState?.startsWith("08") == true) dropConnection()
                            throw e
                        }
                    } finally {
                        lastUsed = clock()
                        busy.decrementAndGet()
                    }
                })
            } catch (e: RejectedExecutionException) {
                busy.decrementAndGet()
                throw SessionClosedException()
            }
        }
        try {
            return future.get()
        } catch (e: ExecutionException) {
            throw e.cause ?: e
        }
    }

    private fun openNew(): Connection {
        val c = opener()
        if (closed) {
            // Closed while the (slow) open was in flight: nobody will ever use or close this connection.
            runCatching { c.close() }
            throw SessionClosedException()
        }
        connection = c
        return c
    }

    private fun dropConnection() {
        closeListeners.forEach { runCatching(it) }
        runningStatement = null
        runCatching { connection?.close() }
        connection = null
    }

    /**
     * Aborts the statement that [runningStatement] points at, if any. Safe to call from any thread. A cancel that
     * reaches the driver before it started working on the statement is ignored (SQLite, PostgreSQL), so the cancel
     * is re-sent a few times, as long as the same run is still going. There is deliberately no sticky "cancel
     * pending" flag: with nothing running it would kill the next, unrelated statement.
     */
    fun cancelRunning() {
        val generation = runGeneration.get()
        val target = running ?: return
        // The run may have changed between the two reads; SQLite's interrupt is connection-wide.
        if (runGeneration.get() != generation) return
        runCatching { target.cancel() }
        for (delay in CANCEL_RETRY_MS) {
            try {
                retryScheduler.schedule({
                    if (running === target && runGeneration.get() == generation) runCatching { target.cancel() }
                }, delay, TimeUnit.MILLISECONDS)
            } catch (_: RejectedExecutionException) {
                return
            }
        }
    }

    /**
     * Housekeeping for the sweeper: when [due] says so (cheap, any thread), [action] runs on the session thread
     * with the connection, but only if the session is open, connected and not busy. A sweep that finds nothing
     * due does not count as use, so it never keeps an idle session alive.
     */
    fun onSweep(due: () -> Boolean, action: (Connection) -> Unit) {
        sweepHooks.add(due to action)
    }

    fun sweep() {
        if (closed || busy.get() > 0 || connection == null) return
        val hooks = sweepHooks.filter { it.first() }
        if (hooks.isEmpty()) return
        try {
            run { c -> hooks.forEach { (_, action) -> runCatching { action(c) } } }
        } catch (_: Exception) {
            // Closed or dropped meanwhile: nothing to maintain.
        }
    }

    /** [listener] runs on the session thread just before the connection is closed (cursors release their statements here). */
    fun onClose(listener: () -> Unit) {
        closeListeners.add(listener)
    }

    /** While [condition] holds (cheap, any thread), [closeIfIdle] leaves the session open: an open transaction. */
    fun keepOpenWhile(condition: () -> Boolean) {
        keepOpenConditions.add(condition)
    }

    /**
     * Closes the session if nothing is running, nothing keeps it open (see [keepOpenWhile]) and it has been unused
     * for [timeoutMs]; true if it was closed.
     */
    fun closeIfIdle(timeoutMs: Long): Boolean {
        gate.withLock {
            if (closed || busy.get() > 0 || clock() - lastUsed < timeoutMs) return false
            if (keepOpenConditions.any { runCatching(it).getOrDefault(false) }) return false
            closed = true
        }
        shutdown()
        return true
    }

    fun close() {
        gate.withLock {
            if (closed) return
            closed = true
        }
        shutdown()
    }

    private fun shutdown() {
        try {
            executor.execute {
                closeListeners.forEach { runCatching(it) }
                runCatching { connection?.close() }
            }
        } catch (_: RejectedExecutionException) {
        }
        executor.shutdown()
        cancelRunning()
        if (!executor.awaitTermination(CLOSE_WAIT_SECONDS, TimeUnit.SECONDS)) {
            // A statement is still running: closing the connection from here aborts it.
            executor.shutdownNow()
            runCatching { connection?.close() }
        }
    }

    private companion object {
        const val CLOSE_WAIT_SECONDS = 5L
        val CANCEL_RETRY_MS = longArrayOf(50, 250, 1000)
        // A few threads: a cancel that blocks on a dead network must not delay the cancels of other sessions.
        val retryScheduler: ScheduledExecutorService by lazy {
            Executors.newScheduledThreadPool(3) { r -> Thread(r, "brewery-db-cancel").apply { isDaemon = true } }
        }
    }
}

/**
 * The key of a [Session]: the connection id for the connection's shared session (the schema tree, completion), or
 * the connection id plus a client-chosen session id for a session of its own (one per query console).
 */
fun sessionKey(connectionId: String, sessionId: String?): String =
    if (sessionId.isNullOrEmpty()) connectionId else "$connectionId$SESSION_SEPARATOR$sessionId"

private const val SESSION_SEPARATOR = '\u0000'

/**
 * Sessions of saved connections: the shared one of a connection id and any number of named ones (see [sessionKey]),
 * each created on first use and closed after [idleTimeoutMs] without use, when the connection is deleted or saved
 * again (all of its sessions), by [closeSession], and on [close]. [clock] (milliseconds) is injectable for tests.
 */
class SessionManager(
    private val exists: (String) -> Boolean,
    private val opener: (String) -> Connection,
    private val clock: () -> Long = System::currentTimeMillis,
    private val idleTimeoutMs: Long = DEFAULT_IDLE_TIMEOUT_MS,
    private val sweepIntervalMs: Long = DEFAULT_SWEEP_INTERVAL_MS,
) : AutoCloseable {
    private val sessions = ConcurrentHashMap<String, Session>()
    private val sweeperLock = Any()
    private var sweeper: ScheduledExecutorService? = null
    @Volatile private var shut = false

    val openCount: Int get() = sessions.size

    /**
     * The live session [sessionId] (null: the shared one) of connection [id], creating it if needed; unknown ids
     * throw `DB_NOT_FOUND`.
     */
    fun session(id: String, sessionId: String? = null): Session {
        if (shut) throw DbException(DbErrorCodes.CONNECTION_FAILED, "The database service is shutting down")
        val key = sessionKey(id, sessionId)
        sessions[key]?.takeUnless { it.closed }?.let { return it }
        if (!exists(id)) throw DbException(DbErrorCodes.NOT_FOUND, "Unknown connection: $id")
        ensureSweeper()
        val created = sessions.compute(key) { _, old -> old?.takeUnless { it.closed } ?: Session(key, { opener(id) }, clock) }!!
        if (shut) {
            // close() ran between the check above and the compute: do not leave a session behind.
            closeKey(key)
            throw DbException(DbErrorCodes.CONNECTION_FAILED, "The database service is shutting down")
        }
        return created
    }

    /** The live session [sessionId] of connection [id], without creating one. */
    fun existing(id: String, sessionId: String? = null): Session? =
        sessions[sessionKey(id, sessionId)]?.takeUnless { it.closed }

    fun <T> run(id: String, block: (Connection) -> T): T = runIn(id) { _, c -> block(c) }

    /** Like [run], in session [sessionId] (null: the shared one), and the block also gets the [Session] it runs in. */
    fun <T> runIn(id: String, sessionId: String? = null, block: (Session, Connection) -> T): T {
        repeat(2) {
            try {
                val s = session(id, sessionId)
                return s.run { c -> block(s, c) }
            } catch (_: SessionClosedException) {
                // Raced with an idle close or a re-save: take a fresh session.
            }
        }
        throw DbException(DbErrorCodes.CONNECTION_FAILED, "Session for $id kept closing")
    }

    /** Closes sessions that have been idle for the timeout. Called by the sweeper; public so tests can drive it. */
    fun sweep() {
        for ((id, s) in sessions.entries.toList()) {
            s.sweep()
            if (s.closeIfIdle(idleTimeoutMs)) sessions.remove(id, s)
        }
    }

    /** Closes every session of connection [id]: the shared one and the named ones. */
    fun close(id: String) {
        val named = "$id$SESSION_SEPARATOR"
        for (key in sessions.keys.toList()) {
            if (key == id || key.startsWith(named)) closeKey(key)
        }
    }

    /** Closes session [sessionId] of connection [id], rolling back what it has not committed. */
    fun closeSession(id: String, sessionId: String) {
        closeKey(sessionKey(id, sessionId))
    }

    private fun closeKey(key: String) {
        sessions.remove(key)?.close()
    }

    override fun close() {
        shut = true
        synchronized(sweeperLock) {
            sweeper?.shutdownNow()
            sweeper = null
        }
        for (key in sessions.keys.toList()) closeKey(key)
    }

    private fun ensureSweeper() {
        synchronized(sweeperLock) {
            if (sweeper != null || shut) return
            sweeper = Executors.newSingleThreadScheduledExecutor { r -> Thread(r, "brewery-db-sweeper").apply { isDaemon = true } }.also {
                it.scheduleWithFixedDelay({ runCatching { sweep() } }, sweepIntervalMs, sweepIntervalMs, TimeUnit.MILLISECONDS)
            }
        }
    }

    companion object {
        const val DEFAULT_IDLE_TIMEOUT_MS = 10 * 60 * 1000L
        const val DEFAULT_SWEEP_INTERVAL_MS = 30 * 1000L
    }
}
