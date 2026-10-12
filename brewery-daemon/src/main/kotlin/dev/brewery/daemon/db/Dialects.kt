package dev.brewery.daemon.db

import java.sql.DatabaseMetaData

/**
 * How the top of the tree looks for a product. `DatabaseMetaData` does the heavy lifting; the dialect only says
 * what the first level is and which names are system noise.
 */
enum class Dialect(
    /** What path `[]` lists: schemas, catalogs (MySQL calls its databases catalogs) or nothing (tables directly). */
    val namespace: Namespace,
    private val systemNamespaces: (String) -> Boolean,
) {
    POSTGRESQL(Namespace.SCHEMA, { it in PG_SYSTEM || it.startsWith("pg_temp_") || it.startsWith("pg_toast_temp_") }),
    MYSQL(Namespace.CATALOG, { it.lowercase() in MYSQL_SYSTEM }),
    SQLITE(Namespace.NONE, { false }),
    GENERIC(Namespace.SCHEMA, { it.lowercase() == "information_schema" });

    enum class Namespace { SCHEMA, CATALOG, NONE }

    fun isSystemNamespace(name: String) = systemNamespaces(name)

    /** Internal tables the user did not create (`sqlite_sequence`...). */
    fun isSystemTable(name: String) = this == SQLITE && name.startsWith("sqlite_")

    companion object {
        private val PG_SYSTEM = setOf("pg_catalog", "information_schema", "pg_toast")
        private val MYSQL_SYSTEM = setOf("information_schema", "mysql", "performance_schema", "sys")

        fun detect(meta: DatabaseMetaData): Dialect {
            val product = meta.databaseProductName.orEmpty().lowercase()
            return when {
                "postgres" in product -> POSTGRESQL
                "mysql" in product || "mariadb" in product -> MYSQL
                "sqlite" in product -> SQLITE
                else -> GENERIC
            }
        }
    }
}
