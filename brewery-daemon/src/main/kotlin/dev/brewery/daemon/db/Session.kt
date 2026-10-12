package dev.brewery.daemon.db

import java.sql.Connection
import java.sql.SQLException
import java.util.concurrent.Callable
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.ExecutionException
import java.util.concurrent.Executors
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
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

    @Volatile private var connection: Connection? = null
    @Volatile private var lastUsed: Long = clock()
    @Volatile var closed: Boolean = false
        private set

    /** The open connection, or null before the first use and after close. Safe to read from any thread. */
    val currentConnection: Connection? get() = connection

    /** Runs [block] on the session thread with the (lazily opened) connection and returns its result. */
    fun <T> run(block: (Connection) -> T): T {
        val future = gate.withLock {
            if (closed) throw SessionClosedException()
            busy.incrementAndGet()
            try {
                executor.submit(Callable {
                    try {
                        val c = connection?.takeUnless { it.isClosed } ?: opener().also { connection = it }
                        block(c)
                    } finally {
                        lastUsed = clock()
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
        } finally {
            busy.decrementAndGet()
            lastUsed = clock()
        }
    }

    /** [listener] runs on the session thread just before the connection is closed (cursors release their statements here). */
    fun onClose(listener: () -> Unit) {
        closeListeners.add(listener)
    }

    /** Closes the session if nothing is running and it has been unused for [timeoutMs]; true if it was closed. */
    fun closeIfIdle(timeoutMs: Long): Boolean {
        gate.withLock {
            if (closed || busy.get() > 0 || clock() - lastUsed < timeoutMs) return false
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
        if (!executor.awaitTermination(CLOSE_WAIT_SECONDS, TimeUnit.SECONDS)) {
            // A statement is still running: closing the connection from here aborts it.
            executor.shutdownNow()
            runCatching { connection?.close() }
        }
    }

    private companion object {
        const val CLOSE_WAIT_SECONDS = 5L
    }
}

/**
 * One [Session] per connection id, created on first use and closed after [idleTimeoutMs] without use, when the
 * connection is deleted or saved again, and on [close]. [clock] (milliseconds) is injectable for tests.
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

    /** The live session for [id], creating it if needed; unknown ids throw `DB_NOT_FOUND`. */
    fun session(id: String): Session {
        if (shut) throw DbException(DbErrorCodes.CONNECTION_FAILED, "The database service is shutting down")
        sessions[id]?.takeUnless { it.closed }?.let { return it }
        if (!exists(id)) throw DbException(DbErrorCodes.NOT_FOUND, "Unknown connection: $id")
        ensureSweeper()
        return sessions.compute(id) { _, old -> old?.takeUnless { it.closed } ?: Session(id, { opener(id) }, clock) }!!
    }

    fun <T> run(id: String, block: (Connection) -> T): T {
        repeat(2) {
            try {
                return session(id).run(block)
            } catch (_: SessionClosedException) {
                // Raced with an idle close or a re-save: take a fresh session.
            }
        }
        throw DbException(DbErrorCodes.CONNECTION_FAILED, "Session for $id kept closing")
    }

    /** Closes sessions that have been idle for the timeout. Called by the sweeper; public so tests can drive it. */
    fun sweep() {
        for ((id, s) in sessions.entries.toList()) {
            if (s.closeIfIdle(idleTimeoutMs)) sessions.remove(id, s)
        }
    }

    fun close(id: String) {
        sessions.remove(id)?.close()
    }

    override fun close() {
        shut = true
        synchronized(sweeperLock) {
            sweeper?.shutdownNow()
            sweeper = null
        }
        for (id in sessions.keys.toList()) close(id)
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
