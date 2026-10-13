package dev.brewery.daemon.db

import java.sql.Connection
import java.sql.DatabaseMetaData
import java.sql.Types

/**
 * One node of the schema tree; [kind] is `catalog | schema | table | view | column | index | foreignKey |
 * sequence | function | folder`. [path] addresses the node: pass it back to list its children.
 */
data class DbNode(
    val name: String,
    val kind: String,
    val path: List<String>,
    val hasChildren: Boolean,
    val detail: String? = null,
)

/**
 * Lazy schema browser over `DatabaseMetaData`. Only the level asked for is queried. Tree shape:
 * `[]` -> schemas (PostgreSQL, generic) or catalogs (MySQL/MariaDB) -> tables, views, sequences ->
 * folders `columns`, `indexes`, `foreignKeys` -> leaves. SQLite has no namespace level, so `[]` lists its
 * tables and views directly. Must run on the session thread.
 */
class Introspector(private val connection: Connection, private val includeSystem: Boolean = false) {
    private val meta: DatabaseMetaData = connection.metaData
    private val dialect = Dialect.detect(meta)
    private val depth = if (dialect.namespace == Dialect.Namespace.NONE) 0 else 1

    fun children(path: List<String>): List<DbNode> {
        if (path.isEmpty() && depth == 1) return namespaces()
        val ns = path.take(depth)
        val rest = path.drop(depth)
        return when (rest.size) {
            0 -> objects(ns)
            1 -> folders(ns, rest[0])
            2 -> when (rest[1]) {
                FOLDER_COLUMNS -> columns(ns, rest[0], path)
                FOLDER_INDEXES -> indexes(ns, rest[0], path)
                FOLDER_FOREIGN_KEYS -> foreignKeys(ns, rest[0], path)
                else -> emptyList()
            }
            else -> emptyList()
        }
    }

    /**
     * Every namespace with its tables, views and columns, for SQL completion. Metadata calls only: `getTables`
     * and one `getColumns` per schema/catalog (not per table). At most [maxTables] tables and views are
     * included, namespaces in name order, tables before views; the rest sets `truncated`.
     */
    fun snapshot(maxTables: Int): DbSchema {
        val namespaces = if (depth == 0) listOf(emptyList()) else namespaces().map { it.path }
        var remaining = maxTables
        var truncated = false
        val schemas = namespaces.map { ns ->
            val found = tableList(ns)
            val kept = found.take(remaining.coerceAtLeast(0))
            if (kept.size < found.size) truncated = true
            remaining -= kept.size
            val columns = if (kept.isEmpty()) emptyMap() else allColumns(ns, kept.mapTo(HashSet()) { it.first })
            SchemaInfo(ns.firstOrNull().orEmpty(), kept.map { (name, view) -> TableInfo(name, if (view) "view" else "table", columns[name].orEmpty()) })
        }
        return DbSchema(schemas, truncated)
    }

    /** Tables then views of [ns] by name, as (name, isView). */
    private fun tableList(ns: List<String>): List<Pair<String, Boolean>> {
        val tables = mutableListOf<String>()
        val views = mutableListOf<String>()
        meta.getTables(catalogOf(ns), schemaPatternOf(ns), null, tableTypes()).use { rs ->
            while (rs.next()) {
                if (!rs.inNamespace(ns)) continue
                val name = rs.getString("TABLE_NAME") ?: continue
                if (!includeSystem && dialect.isSystemTable(name)) continue
                (if (rs.getString("TABLE_TYPE").orEmpty().endsWith("VIEW", ignoreCase = true)) views else tables) += name
            }
        }
        return tables.sortedBy { it.lowercase() }.map { it to false } + views.sortedBy { it.lowercase() }.map { it to true }
    }

    /** The columns of all tables of [ns] named in [wanted] with a single `getColumns` call. */
    private fun allColumns(ns: List<String>, wanted: Set<String>): Map<String, List<ColumnInfo>> {
        val byTable = HashMap<String, MutableList<Pair<Int, ColumnInfo>>>()
        meta.getColumns(catalogOf(ns), schemaPatternOf(ns), "%", "%").use { rs ->
            while (rs.next()) {
                if (!rs.inNamespace(ns)) continue
                val table = rs.getString("TABLE_NAME") ?: continue
                if (table !in wanted) continue
                val name = rs.getString("COLUMN_NAME") ?: continue
                val type = typeLabel(rs.getString("TYPE_NAME").orEmpty(), rs.getInt("DATA_TYPE"), rs.getInt("COLUMN_SIZE"), rs.getInt("DECIMAL_DIGITS"), rs.wasNull())
                byTable.getOrPut(table) { mutableListOf() } += rs.getInt("ORDINAL_POSITION") to ColumnInfo(name, type)
            }
        }
        return byTable.mapValues { (_, cols) -> cols.sortedBy { it.first }.map { it.second } }
    }

    private fun namespaces(): List<DbNode> {
        val catalogs = dialect.namespace == Dialect.Namespace.CATALOG
        val names = mutableListOf<String>()
        (if (catalogs) meta.catalogs else meta.schemas).use { rs ->
            while (rs.next()) rs.getString(1)?.let(names::add)
        }
        return names.filter { includeSystem || !dialect.isSystemNamespace(it) }
            .sortedBy { it.lowercase() }
            .map { DbNode(it, if (catalogs) "catalog" else "schema", listOf(it), true) }
    }

    private fun catalogOf(ns: List<String>): String? = if (dialect.namespace == Dialect.Namespace.CATALOG) ns.firstOrNull() else null
    private fun schemaOf(ns: List<String>): String? = if (dialect.namespace == Dialect.Namespace.SCHEMA) ns.firstOrNull() else null

    /** `schemaPattern` of getTables/getColumns is a LIKE pattern (`my_app` would also match `myXapp`). */
    private fun schemaPatternOf(ns: List<String>): String? = schemaOf(ns)?.let(::escape)

    /** Rows of a pattern-matched call that belong to another schema/catalog than [ns] are dropped. */
    private fun java.sql.ResultSet.inNamespace(ns: List<String>): Boolean {
        schemaOf(ns)?.let { if (getString("TABLE_SCHEM") != it) return false }
        catalogOf(ns)?.let { if (getString("TABLE_CAT") != it) return false }
        return true
    }

    private fun objects(ns: List<String>): List<DbNode> {
        val tables = mutableListOf<DbNode>()
        val views = mutableListOf<DbNode>()
        meta.getTables(catalogOf(ns), schemaPatternOf(ns), null, tableTypes()).use { rs ->
            while (rs.next()) {
                if (!rs.inNamespace(ns)) continue
                val name = rs.getString("TABLE_NAME") ?: continue
                if (!includeSystem && dialect.isSystemTable(name)) continue
                val remarks = rs.getString("REMARKS")?.takeIf { it.isNotBlank() }
                val isView = rs.getString("TABLE_TYPE").orEmpty().endsWith("VIEW", ignoreCase = true)
                (if (isView) views else tables) += DbNode(name, if (isView) "view" else "table", ns + name, true, remarks)
            }
        }
        val out = mutableListOf<DbNode>()
        out += tables.sortedBy { it.name.lowercase() }
        out += views.sortedBy { it.name.lowercase() }
        // pg_sequences needs PostgreSQL 10; losing the sequences must not lose the tables.
        if (dialect == Dialect.POSTGRESQL && ns.isNotEmpty()) out += runCatching { sequences(ns[0]) }.getOrDefault(emptyList())
        return out
    }

    private fun tableTypes(): Array<String> {
        val types = mutableListOf("TABLE", "VIEW")
        // pgjdbc reports these as types of their own.
        if (dialect == Dialect.POSTGRESQL) types += listOf("PARTITIONED TABLE", "MATERIALIZED VIEW", "FOREIGN TABLE")
        if (includeSystem) types += listOf("SYSTEM TABLE", "SYSTEM VIEW")
        return types.toTypedArray()
    }

    private fun sequences(schema: String): List<DbNode> {
        val out = mutableListOf<DbNode>()
        connection.prepareStatement("SELECT sequencename, data_type::text FROM pg_sequences WHERE schemaname = ? ORDER BY sequencename").use { st ->
            st.setString(1, schema)
            st.executeQuery().use { rs ->
                while (rs.next()) out += DbNode(rs.getString(1), "sequence", listOf(schema, rs.getString(1)), false, rs.getString(2))
            }
        }
        return out
    }

    private fun isView(ns: List<String>, table: String): Boolean? {
        meta.getTables(catalogOf(ns), schemaPatternOf(ns), escape(table), tableTypes()).use { rs ->
            while (rs.next()) {
                if (!rs.inNamespace(ns)) continue
                if (rs.getString("TABLE_NAME") == table) return rs.getString("TABLE_TYPE").orEmpty().endsWith("VIEW", ignoreCase = true)
            }
        }
        return null
    }

    private fun folders(ns: List<String>, table: String): List<DbNode> {
        val view = isView(ns, table) ?: return emptyList()
        val base = ns + table
        return buildList {
            add(DbNode(FOLDER_COLUMNS, "folder", base + FOLDER_COLUMNS, true))
            if (!view) {
                add(DbNode(FOLDER_INDEXES, "folder", base + FOLDER_INDEXES, true))
                add(DbNode(FOLDER_FOREIGN_KEYS, "folder", base + FOLDER_FOREIGN_KEYS, true))
            }
        }
    }

    private fun columns(ns: List<String>, table: String, path: List<String>): List<DbNode> {
        val primaryKey = mutableSetOf<String>()
        runCatching {
            meta.getPrimaryKeys(catalogOf(ns), schemaOf(ns), table).use { rs ->
                while (rs.next()) rs.getString("COLUMN_NAME")?.let(primaryKey::add)
            }
        }
        val cols = mutableListOf<Pair<Int, DbNode>>()
        meta.getColumns(catalogOf(ns), schemaPatternOf(ns), escape(table), null).use { rs ->
            while (rs.next()) {
                if (!rs.inNamespace(ns)) continue
                if (rs.getString("TABLE_NAME") != table) continue
                val name = rs.getString("COLUMN_NAME") ?: continue
                val nullable = rs.getInt("NULLABLE") != DatabaseMetaData.columnNoNulls
                val type = typeLabel(rs.getString("TYPE_NAME").orEmpty(), rs.getInt("DATA_TYPE"), rs.getInt("COLUMN_SIZE"), rs.getInt("DECIMAL_DIGITS"), rs.wasNull())
                val detail = type + (if (nullable) " NULL" else " NOT NULL") + (if (name in primaryKey) " PK" else "")
                cols += rs.getInt("ORDINAL_POSITION") to DbNode(name, "column", path + name, false, detail)
            }
        }
        return cols.sortedBy { it.first }.map { it.second }
    }

    private fun typeLabel(typeName: String, sqlType: Int, size: Int, digits: Int, digitsNull: Boolean): String {
        if (typeName.contains('(') || size <= 0) return typeName
        return when (sqlType) {
            Types.CHAR, Types.VARCHAR, Types.NCHAR, Types.NVARCHAR, Types.BINARY, Types.VARBINARY ->
                if (size < UNBOUNDED) "$typeName($size)" else typeName
            Types.DECIMAL, Types.NUMERIC ->
                if (size < MAX_PRECISION) (if (digitsNull) "$typeName($size)" else "$typeName($size,$digits)") else typeName
            else -> typeName
        }
    }

    private fun indexes(ns: List<String>, table: String, path: List<String>): List<DbNode> {
        class Idx(val unique: Boolean, val columns: MutableList<Pair<Int, String>> = mutableListOf())
        val byName = linkedMapOf<String, Idx>()
        meta.getIndexInfo(catalogOf(ns), schemaOf(ns), table, false, true).use { rs ->
            while (rs.next()) {
                if (rs.getShort("TYPE") == DatabaseMetaData.tableIndexStatistic) continue
                val name = rs.getString("INDEX_NAME") ?: continue
                val col = rs.getString("COLUMN_NAME") ?: "expr"
                byName.getOrPut(name) { Idx(!rs.getBoolean("NON_UNIQUE")) }.columns += rs.getInt("ORDINAL_POSITION") to col
            }
        }
        return byName.entries.sortedBy { it.key.lowercase() }.map { (name, idx) ->
            val cols = idx.columns.sortedBy { it.first }.joinToString(", ") { it.second }
            DbNode(name, "index", path + name, false, (if (idx.unique) "UNIQUE " else "") + "($cols)")
        }
    }

    private fun foreignKeys(ns: List<String>, table: String, path: List<String>): List<DbNode> {
        class Fk(val name: String?, val target: String, val from: MutableList<String> = mutableListOf(), val to: MutableList<String> = mutableListOf())
        val keys = mutableListOf<Fk>()
        var lastSeq = Int.MAX_VALUE
        meta.getImportedKeys(catalogOf(ns), schemaOf(ns), table).use { rs ->
            while (rs.next()) {
                val name = rs.getString("FK_NAME")?.takeIf { it.isNotBlank() }
                val pkSchema = rs.getString("PKTABLE_SCHEM") ?: rs.getString("PKTABLE_CAT")
                val pkTable = rs.getString("PKTABLE_NAME") ?: continue
                val target = if (pkSchema != null && pkSchema != ns.firstOrNull()) "$pkSchema.$pkTable" else pkTable
                val seq = rs.getInt("KEY_SEQ")
                val current = keys.lastOrNull()
                // Constraints without a name (SQLite) are told apart by the sequence number starting over.
                val sameKey = current != null && current.target == target && current.name == name && seq > lastSeq
                lastSeq = seq
                val fk = if (sameKey) current!! else Fk(name, target).also { keys += it }
                fk.from += rs.getString("FKCOLUMN_NAME").orEmpty()
                fk.to += rs.getString("PKCOLUMN_NAME").orEmpty()
            }
        }
        return keys.map { fk ->
            val name = fk.name ?: "fk_${fk.from.joinToString("_")}_${fk.target}"
            DbNode(name, "foreignKey", path + name, false, "(${fk.from.joinToString(", ")}) -> ${fk.target}(${fk.to.joinToString(", ")})")
        }.sortedBy { it.name.lowercase() }
    }

    /** `getTables`/`getColumns` take LIKE patterns; a table called `a_b` must not match `axb`. */
    private fun escape(name: String): String {
        val esc = meta.searchStringEscape.orEmpty()
        if (esc.isEmpty()) return name
        return name.replace(esc, esc + esc).replace("_", esc + "_").replace("%", esc + "%")
    }

    private companion object {
        const val FOLDER_COLUMNS = "columns"
        const val FOLDER_INDEXES = "indexes"
        const val FOLDER_FOREIGN_KEYS = "foreignKeys"
        const val UNBOUNDED = 1_000_000_000
        const val MAX_PRECISION = 1000
    }
}
