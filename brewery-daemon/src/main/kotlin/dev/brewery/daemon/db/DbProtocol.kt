package dev.brewery.daemon.db

import org.eclipse.lsp4j.jsonrpc.ResponseErrorException
import org.eclipse.lsp4j.jsonrpc.messages.ResponseError

object DbErrorCodes {
    const val CONNECTION_FAILED = -32010
    const val AUTH_FAILED = -32011
    const val QUERY_FAILED = -32012
    const val NOT_FOUND = -32013
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
