package dev.brewery.daemon.db

/** `db/schema` params; [refresh] (default false) rebuilds the cached snapshot instead of returning it. */
data class DbSchemaParams(val connectionId: String, val refresh: Boolean? = null)

/** [kind] is `table` or `view`. Columns are in table order; [ColumnInfo.type] is the same label introspection shows. */
data class TableInfo(val name: String, val kind: String, val columns: List<ColumnInfo>)

/** [name] is empty for databases without a namespace level (SQLite). MariaDB/MySQL catalogs appear as schemas. */
data class SchemaInfo(val name: String, val tables: List<TableInfo>)

/** Flattened schemas -> tables -> columns of one connection, for SQL completion. [truncated]: more tables existed than the limit. */
data class DbSchema(val schemas: List<SchemaInfo>, val truncated: Boolean)

object SchemaSnapshot {
    /** Upper bound of tables and views in one snapshot. */
    const val MAX_TABLES = 5000
}

/**
 * The snapshot per connection id. [invalidate] also stops a build that is running at that moment from
 * being stored: its result may predate the change that caused the invalidation, so it is only returned.
 */
class SchemaCache {
    private val lock = Any()
    private val entries = HashMap<String, DbSchema>()
    private val epochs = HashMap<String, Long>()

    /** The cached snapshot of [id] unless [refresh]; otherwise [build] runs (outside the lock) and its result is cached. */
    fun get(id: String, refresh: Boolean, build: () -> DbSchema): DbSchema {
        val epoch = synchronized(lock) {
            if (!refresh) entries[id]?.let { return it }
            epochs[id] ?: 0L
        }
        val built = build()
        synchronized(lock) {
            if ((epochs[id] ?: 0L) == epoch) entries[id] = built
        }
        return built
    }

    fun invalidate(id: String) {
        synchronized(lock) {
            entries.remove(id)
            epochs[id] = (epochs[id] ?: 0L) + 1
        }
    }
}
