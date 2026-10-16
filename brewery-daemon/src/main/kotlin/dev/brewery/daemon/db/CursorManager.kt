package dev.brewery.daemon.db

import org.postgresql.core.BaseConnection
import org.postgresql.core.TransactionState
import java.sql.Connection
import java.sql.ResultSet
import java.sql.SQLException
import java.sql.Statement
import java.util.UUID

private class Cursor(
    val id: String,
    val statement: Statement,
    val resultSet: ResultSet,
    val kinds: List<ValueRenderer.Kind>,
) {
    val columnCount: Int get() = kinds.size
    /** True while this cursor relies on the autocommit-off transaction [CursorManager] opened for streaming. */
    var ownsTx = false

    /** Rows handed out so far; the next row to deliver has this index. */
    var position = 0L
    /** One row read ahead (to know whether the result set is exhausted) and not yet delivered. */
    var pending: List<String?>? = null
    var done = false
    /** Clock time of the last execute or fetch, for the inactivity TTL of unfinished cursors. */
    @Volatile var lastActivity = 0L
}

/**
 * The open cursors of one [Session]: a statement plus its forward-only result set each, up to [MAX_CURSORS]
 * (the oldest is closed to make room; finished cursors keep their slot until closed). Everything except
 * [closeAll] and [streamingExpired] must be called on the session thread, i.e. inside `Session.run`. [closeAll]
 * is registered as a session close listener and is idempotent, so it may run on an `08*` connection drop and
 * again at final close.
 *
 * Streaming: statements get a fetch size of [FETCH_SIZE], so MariaDB and PostgreSQL pull rows from the server as
 * `fetch` asks for them instead of buffering the whole result in `execute`. PostgreSQL only does that inside a
 * transaction, so when [manageTransactions] says so (see [SqlText] for what counts as a plain read):
 *  - A plain read (SELECT/WITH/VALUES/TABLE/SHOW/EXPLAIN without FOR UPDATE/SHARE, RETURNING, INTO or DML) on a
 *    connection in autocommit mode turns autocommit off. That implicit transaction is owned by the streaming
 *    cursors; when the last one is finished, closed, evicted or dropped, the manager commits and turns autocommit
 *    back on. It is also ended after [streamTtlMs] without a fetch or execute (the owning cursors are dropped,
 *    fetch -> -32013), so an abandoned result tab cannot hold a server transaction open.
 *  - Every other statement runs unchanged in the driver's current mode (DML ... RETURNING is therefore buffered).
 *    Running one while the implicit transaction is open first commits it and drops the streaming cursors
 *    (fetch -> -32013): the clients must treat that as "result expired".
 *  - A failing plain read while cursors stream is rolled back to a savepoint; they survive. A failing or
 *    cancelled `fetch` aborts the transaction and drops all streaming cursors.
 *  - BEGIN / START TRANSACTION switches the session to the user's control: nothing is wrapped, committed or
 *    rolled back by the manager and results are buffered by the driver, until COMMIT / END / ROLLBACK / ABORT.
 *    On pgjdbc the server's own transaction state decides instead of the keywords (see [serverInTransaction]).
 *    (SET autocommit style statements are not tracked.)
 *  - Any unexpected failure restores autocommit; the manager never leaves a transaction without an owner.
 *
 * MariaDB streams without autocommit changes, but its driver loads a pending result fully into memory when another
 * command runs on the same connection; so with [expireUnfinishedBeforeCommands] every execute and every
 * introspection first cancels and closes the unfinished cursors of the session (fetch -> -32013, "result
 * expired"). Finished cursors are not affected.
 *
 * Every other unfinished cursor (SQLite, MariaDB, a buffered result) is closed after [streamTtlMs] without a fetch
 * as well (fetch -> -32013): an abandoned result tab must not hold a SQLite SHARED lock or a MariaDB metadata lock
 * until the console is closed.
 *
 * [index] maps cursor id to this manager so that `fetch`/`close` can find the right session from an id alone.
 */
class CursorManager(
    val session: Session,
    private val index: MutableMap<String, CursorManager>,
    /** Whether the driver needs autocommit off to stream rows (PostgreSQL); see "Streaming" above. */
    private val manageTransactions: (Connection) -> Boolean = { false },
    private val clock: () -> Long = System::currentTimeMillis,
    private val streamTtlMs: Long = DEFAULT_STREAM_TTL_MS,
    /** Whether the driver drains a pending streaming result into memory before the next command (MariaDB). */
    private val expireUnfinishedBeforeCommands: (Connection) -> Boolean = { false },
) {
    private val cursors = LinkedHashMap<String, Cursor>()
    private var txOwners = 0
    private var txConnection: Connection? = null
    private var policyConnection: Connection? = null
    private var policy = false
    /** True from the moment we turned autocommit off until we turned it back on. */
    private var implicitOpen = false
    @Volatile private var lastStreamActivity = 0L
    private var userTx = false

    init {
        session.onClose { closeAll() }
        session.onSweep(::anyExpired) { expireIdle(it) }
    }

    val openCount: Int get() = synchronized(cursors) { cursors.size }

    /**
     * Runs [sql] and returns after the first result: an update count (no cursor) or the column list of a
     * result set that stays open as a cursor. A driver [java.sql.SQLException] propagates after the statement
     * has been released, so the caller can classify it.
     */
    fun execute(connection: Connection, sql: String): DbExecuteResult {
        expireUnfinishedIfNeeded(connection)
        if (!managesTransactions(connection)) return runStatement(connection, sql, ownsTx = false)
        val kind = SqlText.classify(sql)
        serverInTransaction(connection)?.let { userTx = it }
        if (userTx) return runTracking(connection, sql, kind)
        if (synchronized(cursors) { txOwners } > 0) {
            if (kind == SqlText.Kind.READ) return executeInOpenTransaction(connection, sql)
            finishImplicit(connection)
        } else if (implicitOpen && !connection.autoCommit) {
            // Left behind by a failure we did not see: nobody owns it, so end it.
            runCatching { connection.rollback() }
            restoreAutocommit(connection)
        }
        return if (kind == SqlText.Kind.READ && connection.autoCommit) executeFresh(connection, sql) else runTracking(connection, sql, kind)
    }

    /**
     * MariaDB Connector/J reads the rest of a pending streaming result into memory before it sends any other
     * command on the connection. To keep "rows are pulled by fetch", unfinished cursors are cancelled and closed
     * first (fetch -> -32013, "result expired"). Called before every execute and before introspection.
     */
    fun expireUnfinishedIfNeeded(connection: Connection) {
        if (!runCatching { expireUnfinishedBeforeCommands(connection) }.getOrDefault(false)) return
        val victims = synchronized(cursors) {
            val unfinished = cursors.values.filter { !it.done && !it.ownsTx }
            unfinished.forEach { cursors.remove(it.id) }
            unfinished
        }
        for (cursor in victims) {
            index.remove(cursor.id, this)
            runCatching { cursor.statement.cancel() }
            runCatching { cursor.resultSet.close() }
            runCatching { cursor.statement.close() }
        }
    }

    private fun managesTransactions(connection: Connection): Boolean {
        if (policyConnection !== connection) {
            policy = runCatching { manageTransactions(connection) }.getOrDefault(false)
            policyConnection = connection
        }
        return policy
    }

    private fun runTracking(c: Connection, sql: String, kind: SqlText.Kind): DbExecuteResult {
        var ran = false
        try {
            return runStatement(c, sql, ownsTx = false).also { ran = true }
        } finally {
            // Also after a failure: a failed COMMIT ends the transaction, a failed statement after BEGIN aborts it.
            val server = serverInTransaction(c)
            when {
                server != null -> userTx = server
                !ran -> {}
                kind == SqlText.Kind.TX_BEGIN -> userTx = true
                kind == SqlText.Kind.TX_END -> userTx = false
            }
        }
    }

    /**
     * Whether the server has a transaction open, as pgjdbc tracks it from every ReadyForQuery message (open or
     * failed counts as open): COMMIT AND CHAIN, PREPARE TRANSACTION, a failed COMMIT, a CALL that commits or a
     * BEGIN inside a script all show up here, keywords cannot see them. Null when unknown (not pgjdbc) or while
     * autocommit is off (then the driver itself keeps a transaction open).
     */
    private fun serverInTransaction(c: Connection): Boolean? = runCatching {
        if (!c.autoCommit || !c.isWrapperFor(BaseConnection::class.java)) return null
        c.unwrap(BaseConnection::class.java).transactionState != TransactionState.IDLE
    }.getOrNull()

    private fun executeFresh(c: Connection, sql: String): DbExecuteResult {
        c.autoCommit = false
        implicitOpen = true
        try {
            val result = runStatement(c, sql, ownsTx = true)
            if (result.cursorId == null) {
                c.commit()
                restoreAutocommit(c)
            }
            return result
        } catch (t: Throwable) {
            runCatching { c.rollback() }
            endTransaction(c)
            throw t
        }
    }

    private fun executeInOpenTransaction(c: Connection, sql: String): DbExecuteResult {
        val savepoint = c.setSavepoint()
        try {
            val result = runStatement(c, sql, ownsTx = true)
            runCatching { c.releaseSavepoint(savepoint) }
            return result
        } catch (t: Throwable) {
            try {
                c.rollback(savepoint)
            } catch (_: Throwable) {
                runCatching { c.rollback() }
                endTransaction(c)
            }
            throw t
        }
    }

    private fun runStatement(connection: Connection, sql: String, ownsTx: Boolean): DbExecuteResult {
        val statement = connection.createStatement()
        session.runningStatement = statement
        var kept = false
        try {
            // A driver that rejects a fetch size just buffers.
            runCatching { statement.fetchSize = FETCH_SIZE }
            val started = System.nanoTime()
            val hasResultSet = statement.execute(sql)
            val elapsedMs = (System.nanoTime() - started) / 1_000_000
            if (!hasResultSet) {
                return DbExecuteResult(null, emptyList(), statement.updateCount.toLong().coerceAtLeast(0), elapsedMs)
            }
            val rs = statement.resultSet
            val meta = rs.metaData
            val columns = (1..meta.columnCount).map { ColumnInfo(meta.getColumnLabel(it) ?: "", meta.getColumnTypeName(it) ?: "") }
            val cursor = Cursor(UUID.randomUUID().toString(), statement, rs, ValueRenderer.kinds(meta))
            cursor.lastActivity = clock()
            synchronized(cursors) {
                // Count this cursor before evicting, so that evicting the last older owner cannot end the transaction.
                if (ownsTx) {
                    cursor.ownsTx = true
                    txOwners++
                    txConnection = connection
                    lastStreamActivity = clock()
                }
                while (cursors.size >= MAX_CURSORS) {
                    val oldest = cursors.keys.first()
                    closeCursor(cursors.remove(oldest)!!)
                }
                cursors[cursor.id] = cursor
            }
            index[cursor.id] = this
            kept = true
            return DbExecuteResult(cursor.id, columns, null, elapsedMs)
        } finally {
            session.runningStatement = null
            if (!kept) runCatching { statement.close() }
        }
    }

    /**
     * Rows [offset, offset + limit) of the cursor (limit capped at [MAX_LIMIT]). The result set is forward-only:
     * an offset ahead of the current position skips rows, an offset behind it is invalid params.
     */
    fun fetch(cursorId: String, offset: Long, limit: Int): DbFetchResult {
        if (offset < 0) throw DbException(DbErrorCodes.INVALID_PARAMS, "offset must not be negative")
        if (limit < 1) throw DbException(DbErrorCodes.INVALID_PARAMS, "limit must be at least 1")
        val cursor = synchronized(cursors) { cursors[cursorId] } ?: throw DbException(DbErrorCodes.NOT_FOUND, "Unknown cursor: $cursorId")
        if (offset < cursor.position) {
            throw DbException(
                DbErrorCodes.INVALID_PARAMS,
                "Cursor results are forward-only: offset $offset is behind the current position ${cursor.position}",
            )
        }
        val max = minOf(limit, MAX_LIMIT)
        if (cursor.ownsTx) lastStreamActivity = clock()
        if (!cursor.done) session.runningStatement = cursor.statement
        try {
            while (cursor.position < offset) {
                if (step(cursor, render = false) == null) break
                cursor.position++
            }
            val rows = ArrayList<List<String?>>(minOf(max, 256))
            while (rows.size < max) {
                rows += step(cursor, render = true) ?: break
                cursor.position++
            }
            if (!cursor.done && rows.size == max) cursor.pending = step(cursor, render = true)
            return DbFetchResult(rows, cursor.done && cursor.pending == null)
        } catch (e: Throwable) {
            // A failed or cancelled result set cannot be resumed; inside our streaming transaction the failure
            // also aborts every other streaming cursor.
            val c = txConnection
            if (cursor.ownsTx && c != null) {
                runCatching { c.rollback() }
                endTransaction(c)
            }
            forget(cursor)
            throw e
        } finally {
            cursor.lastActivity = clock()
            if (cursor.ownsTx) lastStreamActivity = clock()
            session.runningStatement = null
        }
    }

    /** Closes one cursor; unknown ids are ignored (an evicted cursor can be closed by a slow client). */
    fun close(cursorId: String) {
        val cursor = synchronized(cursors) { cursors.remove(cursorId) } ?: return
        closeCursor(cursor)
    }

    /** Closes and forgets every cursor. Idempotent. */
    fun closeAll() {
        userTx = false
        val all = synchronized(cursors) { cursors.values.toList().also { cursors.clear() } }
        all.forEach(::closeCursor)
    }

    /** True when the implicit streaming transaction has seen no activity for the TTL. Safe from any thread. */
    fun streamingExpired(): Boolean = synchronized(cursors) { txOwners > 0 } && clock() - lastStreamActivity >= streamTtlMs

    /** True when the streaming transaction or any other unfinished cursor has been idle for the TTL. Safe from any thread. */
    fun anyExpired(): Boolean = streamingExpired() || synchronized(cursors) { idleUnfinished().isNotEmpty() }

    /** Unfinished cursors outside the streaming transaction idle for the TTL; call with the [cursors] lock held. */
    private fun idleUnfinished(): List<Cursor> {
        val now = clock()
        return cursors.values.filter { !it.done && !it.ownsTx && now - it.lastActivity >= streamTtlMs }
    }

    /**
     * Ends what has been idle for the TTL. An expired implicit streaming transaction is committed and its cursors
     * dropped. Any other unfinished cursor is closed too: it can hold a SQLite SHARED lock or a MariaDB metadata
     * lock, or a buffered result in the heap (fetch -> -32013, "result expired").
     */
    private fun expireIdle(c: Connection) {
        if (streamingExpired()) {
            runCatching { c.commit() }
            endTransaction(c)
        }
        val victims = synchronized(cursors) { idleUnfinished().onEach { cursors.remove(it.id) } }
        // A MariaDB streaming result is drained by close() unless cancelled; SQLite's cancel is connection-wide.
        val cancel = victims.isNotEmpty() && runCatching { expireUnfinishedBeforeCommands(c) }.getOrDefault(false)
        for (cursor in victims) {
            index.remove(cursor.id, this)
            if (cancel) runCatching { cursor.statement.cancel() }
            runCatching { cursor.resultSet.close() }
            runCatching { cursor.statement.close() }
        }
    }

    private fun finishImplicit(c: Connection) {
        runCatching { c.commit() }
        endTransaction(c)
    }

    private fun forget(cursor: Cursor) {
        synchronized(cursors) { cursors.remove(cursor.id) }
        closeCursor(cursor)
    }

    private fun closeCursor(cursor: Cursor) {
        index.remove(cursor.id, this)
        runCatching { cursor.resultSet.close() }
        runCatching { cursor.statement.close() }
        release(cursor)
    }

    /** The cursor no longer needs the streaming transaction; the last one out commits and restores autocommit. */
    private fun release(cursor: Cursor) {
        val last = synchronized(cursors) {
            if (!cursor.ownsTx) return
            cursor.ownsTx = false
            txOwners--
            txOwners == 0
        }
        if (last) {
            val c = txConnection
            txConnection = null
            if (c != null) {
                runCatching { c.commit() }
                restoreAutocommit(c)
            }
        }
    }

    /** Drops every streaming cursor (their server-side portals are gone) and restores autocommit. */
    private fun endTransaction(c: Connection) {
        val dropped = synchronized(cursors) {
            val owners = cursors.values.filter { it.ownsTx }
            owners.forEach {
                cursors.remove(it.id)
                it.ownsTx = false
            }
            txOwners = 0
            txConnection = null
            owners
        }
        for (cursor in dropped) {
            index.remove(cursor.id, this)
            runCatching { cursor.resultSet.close() }
            runCatching { cursor.statement.close() }
        }
        restoreAutocommit(c)
    }

    private fun restoreAutocommit(c: Connection) {
        // Only forget the implicit transaction once autocommit is really back; otherwise the leak guard needs it.
        if (runCatching { c.autoCommit = true }.isSuccess) implicitOpen = false
    }

    /** The next row (rendered when [render]), or null once the result set is exhausted (which also releases it). */
    private fun step(cursor: Cursor, render: Boolean): List<String?>? {
        cursor.pending?.let {
            cursor.pending = null
            return it
        }
        if (cursor.done) return null
        if (!cursor.resultSet.next()) {
            cursor.done = true
            runCatching { cursor.resultSet.close() }
            runCatching { cursor.statement.close() }
            release(cursor)
            return null
        }
        if (!render) return emptyList()
        return List(cursor.columnCount) { ValueRenderer.read(cursor.resultSet, it + 1, cursor.kinds[it]) }
    }

    companion object {
        const val MAX_CURSORS = 20
        const val MAX_LIMIT = 1000
        const val FETCH_SIZE = 500
        const val DEFAULT_STREAM_TTL_MS = 60_000L
    }
}
