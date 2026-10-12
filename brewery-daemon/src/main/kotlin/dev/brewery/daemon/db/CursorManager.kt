package dev.brewery.daemon.db

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
}

/**
 * The open cursors of one [Session]: a statement plus its forward-only result set each, up to [MAX_CURSORS]
 * (the oldest is closed to make room). Everything except [closeAll] must be called on the session thread, i.e.
 * inside `Session.run`. [closeAll] is registered as a session close listener and is idempotent, so it may run
 * on an `08*` connection drop and again at final close.
 *
 * Streaming: statements get a fetch size of [FETCH_SIZE], so MariaDB and PostgreSQL pull rows from the server as
 * `fetch` asks for them instead of buffering the whole result in `execute`. PostgreSQL only does that inside a
 * transaction, so when [manageTransactions] says so, `execute` of a query turns autocommit off and the manager
 * owns that transaction until the last streaming cursor is finished, closed, evicted or dropped; then it commits
 * and turns autocommit back on. Consequences, all deliberate:
 *  - A statement that returns no result set while streaming cursors are open is committed at once (so it behaves
 *    as autocommit), which closes the server-side portals: the streaming cursors are dropped (fetch -> -32013).
 *  - A failing statement while cursors are open is rolled back to a savepoint, so the cursors survive.
 *  - A failing or cancelled `fetch` aborts the transaction and drops all streaming cursors.
 *  - Statements that cannot run inside a transaction block (VACUUM, CREATE DATABASE, ...; SQLState 25001) are
 *    re-run once in autocommit mode.
 *  - MariaDB streams without autocommit changes; the driver loads a pending result fully when another statement
 *    runs on the same connection.
 *
 * [index] maps cursor id to this manager so that `fetch`/`close` can find the right session from an id alone.
 */
class CursorManager(
    val session: Session,
    private val index: MutableMap<String, CursorManager>,
    /** Whether the driver needs autocommit off to stream rows (PostgreSQL); see "Streaming" above. */
    private val manageTransactions: (Connection) -> Boolean = { false },
) {
    private val cursors = LinkedHashMap<String, Cursor>()
    private var txOwners = 0
    private var txConnection: Connection? = null
    private var policyConnection: Connection? = null
    private var policy = false

    init {
        session.onClose { closeAll() }
    }

    val openCount: Int get() = synchronized(cursors) { cursors.size }

    /**
     * Runs [sql] and returns after the first result: an update count (no cursor) or the column list of a
     * result set that stays open as a cursor. A driver [java.sql.SQLException] propagates after the statement
     * has been released, so the caller can classify it.
     */
    fun execute(connection: Connection, sql: String): DbExecuteResult = when {
        !managesTransactions(connection) -> runStatement(connection, sql, ownsTx = false)
        synchronized(cursors) { txOwners } > 0 -> executeInOpenTransaction(connection, sql)
        connection.autoCommit -> executeFresh(connection, sql)
        // Somebody else already put the connection in a transaction: leave it alone.
        else -> runStatement(connection, sql, ownsTx = false)
    }

    private fun managesTransactions(connection: Connection): Boolean {
        if (policyConnection !== connection) {
            policy = runCatching { manageTransactions(connection) }.getOrDefault(false)
            policyConnection = connection
        }
        return policy
    }

    private fun executeFresh(c: Connection, sql: String): DbExecuteResult {
        c.autoCommit = false
        try {
            val result = runStatement(c, sql, ownsTx = true)
            if (result.cursorId == null) {
                c.commit()
                c.autoCommit = true
            }
            return result
        } catch (e: SQLException) {
            runCatching { c.rollback() }
            runCatching { c.autoCommit = true }
            if (e.sqlState == "25001") return runStatement(c, sql, ownsTx = false)
            throw e
        }
    }

    private fun executeInOpenTransaction(c: Connection, sql: String): DbExecuteResult {
        val savepoint = c.setSavepoint()
        try {
            val result = runStatement(c, sql, ownsTx = true)
            if (result.cursorId != null) {
                runCatching { c.releaseSavepoint(savepoint) }
                return result
            }
            c.commit()
            endTransaction(c)
            return result
        } catch (e: SQLException) {
            try {
                c.rollback(savepoint)
            } catch (_: SQLException) {
                runCatching { c.rollback() }
                endTransaction(c)
            }
            throw e
        }
    }

    private fun runStatement(connection: Connection, sql: String, ownsTx: Boolean): DbExecuteResult {
        val statement = connection.createStatement()
        session.runningStatement = statement
        var kept = false
        try {
            statement.fetchSize = FETCH_SIZE
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
            synchronized(cursors) {
                // Count this cursor before evicting, so that evicting the last older owner cannot end the transaction.
                if (ownsTx) {
                    cursor.ownsTx = true
                    txOwners++
                    txConnection = connection
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
        val all = synchronized(cursors) { cursors.values.toList().also { cursors.clear() } }
        all.forEach(::closeCursor)
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
                runCatching { c.autoCommit = true }
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
        runCatching { c.autoCommit = true }
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
    }
}
