package dev.brewery.daemon.db

import java.util.concurrent.CompletableFuture
import java.util.concurrent.ExecutionException

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
 * The snapshot per connection id, single-flight: concurrent misses share one in-flight build. [invalidate] (and a
 * `refresh`) bump the connection's generation, so a build that started before it is returned to its own waiters
 * but never cached or joined by later callers; its result may predate the change. The generation map keeps one
 * small entry per connection id for the life of the service (it cannot be dropped without losing the guard).
 */
class SchemaCache {
    private class Flight(val generation: Long, val result: CompletableFuture<DbSchema> = CompletableFuture())

    private val lock = Any()
    private val entries = HashMap<String, DbSchema>()
    private val generations = HashMap<String, Long>()
    private val inFlight = HashMap<String, Flight>()

    /** The cached snapshot of [id] unless [refresh]; otherwise one [build] (outside the lock) runs and is shared. */
    fun get(id: String, refresh: Boolean, build: () -> DbSchema): DbSchema {
        var mine: Flight? = null
        val flight = synchronized(lock) {
            if (refresh) bump(id) else entries[id]?.let { return it }
            val generation = generations[id] ?: 0L
            inFlight[id]?.takeIf { it.generation == generation }
                ?: Flight(generation).also {
                    inFlight[id] = it
                    mine = it
                }
        }
        if (mine == null) {
            try {
                return flight.result.get()
            } catch (e: ExecutionException) {
                throw e.cause ?: e
            }
        }
        try {
            val built = build()
            synchronized(lock) {
                if ((generations[id] ?: 0L) == flight.generation) entries[id] = built
                if (inFlight[id] === flight) inFlight.remove(id)
            }
            flight.result.complete(built)
            return built
        } catch (t: Throwable) {
            synchronized(lock) { if (inFlight[id] === flight) inFlight.remove(id) }
            flight.result.completeExceptionally(t)
            throw t
        }
    }

    fun invalidate(id: String) {
        synchronized(lock) { bump(id) }
    }

    // Caller holds [lock].
    private fun bump(id: String) {
        entries.remove(id)
        generations[id] = (generations[id] ?: 0L) + 1
    }
}
