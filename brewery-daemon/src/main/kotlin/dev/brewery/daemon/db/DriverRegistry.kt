package dev.brewery.daemon.db

import java.io.File
import java.net.URLClassLoader
import java.sql.Driver

/**
 * Bundled drivers (PostgreSQL, MySQL via MariaDB Connector/J, MariaDB, SQLite) plus the project's user
 * drivers from `db.json`. A user driver gets its own [URLClassLoader] so its dependencies cannot clash
 * with the daemon's, and is never registered with `DriverManager`.
 */
class DriverRegistry(private val store: ConnectionStore) {
    private class Bundled(val id: String, val name: String, val className: String)

    private val bundled = listOf(
        Bundled("postgresql", "PostgreSQL", "org.postgresql.Driver"),
        // MySQL Connector/J is GPL; MariaDB Connector/J speaks the MySQL protocol and is LGPL.
        Bundled("mysql", "MySQL", "org.mariadb.jdbc.Driver"),
        Bundled("mariadb", "MariaDB", "org.mariadb.jdbc.Driver"),
        Bundled("sqlite", "SQLite", "org.sqlite.JDBC"),
    )

    private class Loaded(val key: String, val loader: URLClassLoader, val driver: Driver)

    private val loaded = HashMap<String, Loaded>()

    fun list(): List<DriverInfo> =
        bundled.map { DriverInfo(it.id, it.name, true, emptyList()) } +
            store.drivers().map { DriverInfo(it.id, it.name, false, it.jarPaths) }

    /** The JDBC driver for [id]; throws [DbException] (`DB_NOT_FOUND` / `DB_CONNECTION_FAILED`). */
    @Synchronized
    fun driver(id: String): Driver {
        bundled.firstOrNull { it.id == id }?.let { b ->
            return try {
                instantiate(Class.forName(b.className))
            } catch (e: Throwable) {
                throw DbException(DbErrorCodes.CONNECTION_FAILED, "Bundled driver ${b.name} failed to load: ${e.javaClass.simpleName}")
            }
        }
        val user = store.drivers().firstOrNull { it.id == id }
            ?: throw DbException(DbErrorCodes.NOT_FOUND, "Unknown driver: $id")
        val key = user.className + "|" + user.jarPaths.joinToString("|")
        loaded[id]?.takeIf { it.key == key }?.let { return it.driver }
        loaded.remove(id)?.loader?.close()
        val entry = load(user.className, user.jarPaths, key)
        loaded[id] = entry
        return entry.driver
    }

    /** Validates that [className] loads from [jarPaths], then persists the driver in `db.json`. */
    @Synchronized
    fun addJar(id: String, name: String, className: String, jarPaths: List<String>): DriverInfo {
        if (id.isBlank()) throw DbException(DbErrorCodes.CONNECTION_FAILED, "Driver id must not be empty")
        if (bundled.any { it.id == id }) throw DbException(DbErrorCodes.CONNECTION_FAILED, "Driver id '$id' is reserved for a bundled driver")
        if (jarPaths.isEmpty()) throw DbException(DbErrorCodes.CONNECTION_FAILED, "No driver jar given")
        val entry = load(className, jarPaths, "")
        loaded.remove(id)?.loader?.close()
        loaded[id] = Loaded(className + "|" + jarPaths.joinToString("|"), entry.loader, entry.driver)
        store.saveDriver(StoredDriver(id, name.ifBlank { id }, className, jarPaths))
        return DriverInfo(id, name.ifBlank { id }, false, jarPaths)
    }

    private fun load(className: String, jarPaths: List<String>, key: String): Loaded {
        val urls = jarPaths.map { path ->
            val f = File(path)
            if (!f.isFile) throw DbException(DbErrorCodes.CONNECTION_FAILED, "Driver jar not found: $path")
            f.toURI().toURL()
        }
        // The platform loader as parent exposes java.sql but none of the daemon's own classes.
        val loader = URLClassLoader(urls.toTypedArray(), ClassLoader.getPlatformClassLoader())
        try {
            return Loaded(key, loader, instantiate(Class.forName(className, true, loader)))
        } catch (e: DbException) {
            loader.close()
            throw e
        } catch (e: Throwable) {
            loader.close()
            throw DbException(DbErrorCodes.CONNECTION_FAILED, "Cannot load driver class $className: ${e.javaClass.simpleName}")
        }
    }

    private fun instantiate(cls: Class<*>): Driver {
        if (!Driver::class.java.isAssignableFrom(cls)) {
            throw DbException(DbErrorCodes.CONNECTION_FAILED, "${cls.name} is not a java.sql.Driver")
        }
        return cls.getDeclaredConstructor().newInstance() as Driver
    }

    fun close() {
        synchronized(this) {
            loaded.values.forEach { runCatching { it.loader.close() } }
            loaded.clear()
        }
    }
}
