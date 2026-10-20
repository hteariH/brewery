package dev.brewery.daemon.mcp

import dev.brewery.daemon.db.Introspector
import dev.brewery.daemon.db.SchemaSnapshot
import dev.brewery.daemon.db.SqlText
import dev.brewery.daemon.db.ValueRenderer
import java.io.IOException
import java.io.UncheckedIOException
import java.nio.file.Path
import java.sql.Connection
import java.sql.DriverManager
import java.sql.SQLException
import java.util.Properties

const val MAX_ROWS = 200
private const val QUERY_TIMEOUT_SECONDS = 30

data class QueryResult(val columns: List<String>, val rows: List<List<String?>>, val truncated: Boolean)

/** sqlite-jdbc's `SQLITE_OPEN_READONLY`: without CREATE, a missing database file is an error instead of a new file. */
private const val SQLITE_OPEN_READONLY = "1"

/**
 * Opens [ds]. SQLite cannot switch an open connection to read-only, so its file is opened read-only from the start
 * (sqlite-jdbc's `open_mode` property); other drivers are switched by [DbTools] with `setReadOnly`.
 */
internal fun openJdbc(ds: Datasource): Connection {
    val props = Properties()
    ds.username?.let { props.setProperty("user", it) }
    ds.password?.let { props.setProperty("password", it) }
    if (ds.url.startsWith("jdbc:sqlite:", ignoreCase = true)) props.setProperty("open_mode", SQLITE_OPEN_READONLY)
    return DriverManager.getConnection(ds.url, props)
}

/**
 * Read-only database tools on the project's Spring datasource. Read-only is enforced twice:
 * - only a single statement that [SqlText.classify] calls a plain read is run;
 * - it runs on a read-only connection (`setReadOnly(true)`; a connection that refuses it fails the call, it is never
 *   used writable) inside a transaction that is always rolled back.
 * How strictly the driver honours a read-only connection is up to the driver: PostgreSQL runs a READ ONLY
 * transaction, SQLite opens the file read-only (see [openJdbc]).
 */
class DbTools(
    private val root: Path,
    private val env: (String) -> String? = System::getenv,
    private val connect: (Datasource) -> Connection = ::openJdbc,
) {
    private fun datasource(): Datasource {
        val ds = try {
            SpringDatasource.find(root, env)
        } catch (e: IOException) {
            throw McpToolError("Could not read the Spring configuration under $root: ${e.describe()}")
        } catch (e: UncheckedIOException) {
            throw McpToolError("Could not read the Spring configuration under $root: ${e.cause?.describe() ?: e.message}")
        }
        return ds ?: throw McpToolError("No spring.datasource.url in src/main/resources/application.{yml,properties} of $root")
    }

    private fun Throwable.describe(): String = listOfNotNull(javaClass.simpleName, message).joinToString(": ")

    private fun <T> withConnection(block: (Connection) -> T): T {
        val ds = datasource()
        val connection = try {
            connect(ds)
        } catch (e: SQLException) {
            throw McpToolError("Could not connect to ${ds.url} (from ${ds.source}): ${e.message}")
        }
        return connection.use {
            // A connection opened read-only (SQLite, see openJdbc) already is; SQLite would throw on the call.
            if (!it.isReadOnly) it.isReadOnly = true
            it.autoCommit = false
            try {
                block(it)
            } finally {
                runCatching { it.rollback() }
            }
        }
    }

    private fun schema(table: String?): Any = withConnection { c ->
        val snapshot = Introspector(c).snapshot(SchemaSnapshot.MAX_TABLES)
        if (table == null) {
            mapOf(
                "schemas" to snapshot.schemas.map { s ->
                    mapOf("schema" to s.name, "tables" to s.tables.map { mapOf("name" to it.name, "kind" to it.kind, "columns" to it.columns.size) })
                },
                "truncated" to snapshot.truncated,
            )
        } else {
            val matches = snapshot.schemas.flatMap { s -> s.tables.filter { it.name.equals(table, ignoreCase = true) }.map { s.name to it } }
            if (matches.isEmpty()) throw McpToolError("No table or view named $table")
            matches.map { (schema, t) -> mapOf("schema" to schema, "name" to t.name, "kind" to t.kind, "columns" to t.columns) }
        }
    }

    private fun query(sql: String): QueryResult {
        if (SqlText.classify(sql) != SqlText.Kind.READ) {
            throw McpToolError("Only a single read-only statement (SELECT, WITH, VALUES, TABLE, SHOW, EXPLAIN) is allowed")
        }
        return withConnection { c ->
            c.createStatement().use { st ->
                st.maxRows = MAX_ROWS + 1
                st.queryTimeout = QUERY_TIMEOUT_SECONDS
                st.executeQuery(sql).use { rs ->
                    val meta = rs.metaData
                    val kinds = ValueRenderer.kinds(meta)
                    val columns = (1..meta.columnCount).map { meta.getColumnLabel(it) }
                    val rows = ArrayList<List<String?>>()
                    var truncated = false
                    while (rs.next()) {
                        if (rows.size == MAX_ROWS) { truncated = true; break }
                        rows += (1..meta.columnCount).map { ValueRenderer.read(rs, it, kinds[it - 1]) }
                    }
                    QueryResult(columns, rows, truncated)
                }
            }
        }
    }

    fun tools(): List<McpTool> = listOf(
        McpTool(
            "db_schema",
            "Tables and views of the project's Spring datasource (spring.datasource.url), or the columns of one table.",
            objectSchema("table" to stringProp("Table or view name (case-insensitive). Omit to list all tables.")),
        ) { args -> schema(args.get("table")?.takeIf { it.isJsonPrimitive }?.asString) },
        McpTool(
            "db_query",
            "Runs one read-only SQL statement on the project's Spring datasource and returns up to $MAX_ROWS rows.",
            objectSchema("sql" to stringProp("A single SELECT, WITH, VALUES, TABLE, SHOW or EXPLAIN statement."), required = listOf("sql")),
        ) { args ->
            val sql = args.get("sql")?.takeIf { it.isJsonPrimitive }?.asString ?: throw McpToolError("sql is required")
            query(sql)
        },
    )
}
