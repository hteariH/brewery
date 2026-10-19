package dev.brewery.daemon.db

import org.eclipse.lsp4j.jsonrpc.ResponseErrorException
import org.eclipse.lsp4j.jsonrpc.messages.ResponseError

object DbErrorCodes {
    const val CONNECTION_FAILED = -32010
    const val AUTH_FAILED = -32011
    const val QUERY_FAILED = -32012
    const val NOT_FOUND = -32013
    /** JSON-RPC invalid params, e.g. a fetch offset behind a forward-only cursor. */
    const val INVALID_PARAMS = -32602
}

/** A domain failure of the database methods; the daemon turns it into a JSON-RPC error with [code]. */
class DbException(val code: Int, message: String) : RuntimeException(message) {
    fun toResponseError(): ResponseErrorException = ResponseErrorException(ResponseError(code, message, null))
}

data class DriverInfo(val id: String, val name: String, val bundled: Boolean, val jarPaths: List<String>)

/**
 * [warning] is only ever set in a `db/connections/save` result, when the password could not be handed to
 * the OS secret store; it is never written to `db.json`.
 */
data class ConnectionInfo(
    val id: String,
    val name: String,
    val driverId: String,
    val url: String,
    val user: String?,
    val hasPassword: Boolean,
    val options: Map<String, String>,
    val warning: String? = null,
)

data class DbTestResult(val ok: Boolean, val message: String, val serverVersion: String?)

class DbDriversParams
class DbConnectionsListParams
data class DbConnectionsSaveParams(val connection: ConnectionInfo, val password: String?)
data class DbConnectionsDeleteParams(val id: String)
data class DbConnectionsTestParams(val connection: ConnectionInfo, val password: String?)
data class DbDriversAddJarParams(val driverId: String, val name: String, val className: String, val jarPaths: List<String>)

/** [includeSystem] (optional, default false) also lists `pg_catalog`, `information_schema`, `sqlite_*` and the like. */
data class DbIntrospectParams(val connectionId: String, val path: List<String>?, val includeSystem: Boolean? = null)

data class DbExecuteParams(val connectionId: String, val sql: String)
data class ColumnInfo(val name: String, val type: String)

/**
 * [cursorId] is null for statements that produce no result set; then [updateCount] is set instead. Only the first
 * result of the SQL is returned: [moreResults] says that an update count was followed by further results, which were
 * dropped (for a result set the last page says so, see [DbFetchResult.moreResults]). [inTransaction] says the
 * session has a transaction of the user's open afterwards (BEGIN without COMMIT or ROLLBACK yet).
 */
data class DbExecuteResult(
    val cursorId: String?,
    val columns: List<ColumnInfo>,
    val updateCount: Long?,
    val elapsedMs: Long,
    val moreResults: Boolean = false,
    val inTransaction: Boolean = false,
)

/**
 * Rows [offset, offset + limit) of a cursor; limit is capped at 1000. Results are forward-only: an offset behind
 * what was already fetched is rejected with -32602, an offset ahead skips rows.
 */
data class DbFetchParams(val cursorId: String, val offset: Int, val limit: Int)
/**
 * [truncated] comes with the last page ([done]) of a result that was cut off: one the driver has to hold in memory
 * (PostgreSQL inside a user transaction, DML ... RETURNING, SELECT ... FOR UPDATE) stops after 100 000 rows.
 * [moreResults] comes with the last page when the SQL produced further results after this one; they were dropped.
 */
data class DbFetchResult(
    val rows: List<List<String?>>,
    val done: Boolean,
    val truncated: Boolean = false,
    val moreResults: Boolean = false,
)

/** Cancels the running statement of the session of [connectionId]; [cursorId] may stand in for it when it is blank. */
data class DbCancelParams(val connectionId: String?, val cursorId: String? = null)
data class DbCloseParams(val cursorId: String)
