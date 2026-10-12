package dev.brewery.daemon.db

import com.google.gson.Gson
import com.google.gson.GsonBuilder
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardCopyOption
import java.util.TreeMap
import java.util.UUID

/** What `db.json` keeps of a connection: everything except the password state. */
internal data class StoredConnection(
    val id: String,
    val name: String,
    val driverId: String,
    val url: String,
    val user: String?,
    val options: Map<String, String>,
)

/** A user-added driver. [className] is persisted alongside the public [DriverInfo] fields. */
data class StoredDriver(val id: String, val name: String, val className: String, val jarPaths: List<String>)

private class DbFile(
    val connections: List<StoredConnection>? = null,
    val drivers: List<StoredDriver>? = null,
)

/** `.brewery/db.json` of one project. Holds no secrets. All access is serialised on the instance. */
class ConnectionStore(root: Path) {
    private val file: Path = root.resolve(".brewery").resolve("db.json")
    private val gson: Gson = GsonBuilder().setPrettyPrinting().disableHtmlEscaping().create()

    @Synchronized
    fun connections(): List<ConnectionInfo> = read(forWrite = false).first.map { it.toInfo() }

    @Synchronized
    fun connection(id: String): ConnectionInfo? = read(forWrite = false).first.firstOrNull { it.id == id }?.toInfo()

    /** Stores [info] (assigning a UUID when its id is blank) and returns it as saved; `hasPassword` is not stored. */
    @Synchronized
    fun saveConnection(info: ConnectionInfo): ConnectionInfo {
        val id = (info.id as String?).orEmpty().ifBlank { UUID.randomUUID().toString() }
        val stored = StoredConnection(id, info.name, info.driverId, info.url, info.user, TreeMap(info.options ?: emptyMap()))
        val (connections, drivers) = read(forWrite = true)
        val index = connections.indexOfFirst { it.id == id }
        write(if (index < 0) connections + stored else connections.toMutableList().also { it[index] = stored }, drivers)
        return stored.toInfo()
    }

    @Synchronized
    fun deleteConnection(id: String): Boolean {
        val (connections, drivers) = read(forWrite = true)
        if (connections.none { it.id == id }) return false
        write(connections.filterNot { it.id == id }, drivers)
        return true
    }

    @Synchronized
    fun drivers(): List<StoredDriver> = read(forWrite = false).second

    @Synchronized
    fun saveDriver(driver: StoredDriver) {
        val (connections, drivers) = read(forWrite = true)
        val index = drivers.indexOfFirst { it.id == driver.id }
        write(connections, if (index < 0) drivers + driver else drivers.toMutableList().also { it[index] = driver })
    }

    private fun StoredConnection.toInfo() = ConnectionInfo(id, name, driverId, url, user, false, options ?: emptyMap())

    // A missing or unreadable file is an empty store: a hand-damaged db.json must not take the daemon down.
    private fun read(forWrite: Boolean): Pair<List<StoredConnection>, List<StoredDriver>> {
        if (!Files.isRegularFile(file)) return emptyList<StoredConnection>() to emptyList()
        return try {
            val parsed = gson.fromJson(Files.readString(file), DbFile::class.java)
            (parsed?.connections ?: emptyList()) to (parsed?.drivers ?: emptyList())
        } catch (e: Exception) {
            // Saving would replace the user's hand-edited file with just the new item, so writes refuse.
            val reason = (e.cause?.message ?: e.message ?: e.javaClass.simpleName).lineSequence().first().take(200)
            if (forWrite) throw DbException(-32603, "db.json is invalid: $reason")
            System.err.println("brewery: ignoring unreadable ${file.fileName}: ${e.javaClass.simpleName}")
            emptyList<StoredConnection>() to emptyList()
        }
    }

    private fun write(connections: List<StoredConnection>, drivers: List<StoredDriver>) {
        Files.createDirectories(file.parent)
        val text = gson.toJson(DbFile(connections, drivers)).replace("\r\n", "\n") + "\n"
        val tmp = file.resolveSibling("db.json.tmp")
        Files.writeString(tmp, text)
        try {
            Files.move(tmp, file, StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE)
        } catch (_: java.nio.file.AtomicMoveNotSupportedException) {
            Files.move(tmp, file, StandardCopyOption.REPLACE_EXISTING)
        }
    }
}
