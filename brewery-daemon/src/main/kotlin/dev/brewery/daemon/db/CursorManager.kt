package dev.brewery.daemon.db

import java.sql.Connection
import java.sql.ResultSet
import java.sql.Statement
import java.util.UUID

private class Cursor(val id: String, val statement: Statement, val resultSet: ResultSet, val columnCount: Int) {
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
 * [index] maps cursor id to this manager so that `fetch`/`close` can find the right session from an id alone.
 */
class CursorManager(val session: Session, private val index: MutableMap<String, CursorManager>) {
    private val cursors = LinkedHashMap<String, Cursor>()

    init {
        session.onClose { closeAll() }
    }

    val openCount: Int get() = synchronized(cursors) { cursors.size }

    /**
     * Runs [sql] and returns after the first result: an update count (no cursor) or the column list of a
     * result set that stays open as a cursor. A driver [java.sql.SQLException] propagates after the statement
     * has been released, so the caller can classify it.
     */
    fun execute(connection: Connection, sql: String): DbExecuteResult {
        val statement = connection.createStatement()
        session.runningStatement = statement
        var kept = false
        try {
            val started = System.nanoTime()
            val hasResultSet = statement.execute(sql)
            val elapsedMs = (System.nanoTime() - started) / 1_000_000
            if (!hasResultSet) {
                return DbExecuteResult(null, emptyList(), statement.updateCount.toLong().coerceAtLeast(0), elapsedMs)
            }
            val rs = statement.resultSet
            val meta = rs.metaData
            val columns = (1..meta.columnCount).map { ColumnInfo(meta.getColumnLabel(it) ?: "", meta.getColumnTypeName(it) ?: "") }
            val cursor = Cursor(UUID.randomUUID().toString(), statement, rs, columns.size)
            synchronized(cursors) {
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
            // A failed or cancelled result set cannot be resumed.
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
            return null
        }
        if (!render) return emptyList()
        return List(cursor.columnCount) { ValueRenderer.read(cursor.resultSet, it + 1) }
    }

    companion object {
        const val MAX_CURSORS = 20
        const val MAX_LIMIT = 1000
    }
}
