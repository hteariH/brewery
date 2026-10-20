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

const val MAX_ROWS = 200
private const val QUERY_TIMEOUT_SECONDS = 30

data class QueryResult(val columns: List<String>, val rows: List<List<String?>>, val truncated: Boolean)

private fun openJdbc(ds: Datasource): Connection = DriverManager.getConnection(ds.url, ds.username, ds.password)

/**
 * Read-only database tools on the project's Spring datasource. Read-only is enforced twice: only SQL that
 * [SqlText.classify] calls a plain read is run, on a read-only connection whose transaction is rolled back.
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
            // SQLite's driver refuses to change the read-only flag once the connection is open; there the
            // SqlText guard and the rollback below still keep the database unchanged.
            runCatching { it.isReadOnly = true }
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
