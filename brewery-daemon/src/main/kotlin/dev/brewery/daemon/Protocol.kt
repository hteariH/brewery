package dev.brewery.daemon

import com.google.gson.TypeAdapter
import com.google.gson.annotations.JsonAdapter
import com.google.gson.stream.JsonReader
import com.google.gson.stream.JsonWriter
import org.eclipse.lsp4j.jsonrpc.services.JsonNotification
import org.eclipse.lsp4j.jsonrpc.services.JsonRequest
import dev.brewery.daemon.db.ConnectionInfo
import dev.brewery.daemon.db.DbConnectionsDeleteParams
import dev.brewery.daemon.db.DbConnectionsListParams
import dev.brewery.daemon.db.DbConnectionsSaveParams
import dev.brewery.daemon.db.DbConnectionsTestParams
import dev.brewery.daemon.db.DbDriversAddJarParams
import dev.brewery.daemon.db.DbDriversParams
import dev.brewery.daemon.db.DbIntrospectParams
import dev.brewery.daemon.db.DbNode
import dev.brewery.daemon.db.DbTestResult
import dev.brewery.daemon.db.DriverInfo
import dev.brewery.daemon.run.RunConfiguration
import dev.brewery.daemon.run.RunConfigurationsParams
import dev.brewery.daemon.spring.SpringModel
import java.util.concurrent.CompletableFuture

const val PROTOCOL_VERSION = 1
const val DAEMON_VERSION = "0.1.0"

data class InitializeParams(val protocolVersion: Int)
data class InitializeResult(val protocolVersion: Int, val daemonVersion: String)
data class OpenProjectParams(val rootDir: String)
class ProjectModelParams
class ProjectChanged
class SpringModelParams
class SpringChanged

enum class BuildTool { gradle, maven, none }
data class BuildTask(val name: String, val group: String?, val description: String?)
data class BuildModule(val name: String, val path: String, val tasks: List<BuildTask>)

// lsp4j's Gson registers a factory that writes enums as their ordinal; the wire format wants the name.
// A field-level adapter takes precedence over that factory.
class BuildToolAdapter : TypeAdapter<BuildTool>() {
    override fun write(out: JsonWriter, value: BuildTool?) {
        if (value == null) out.nullValue() else out.value(value.name)
    }

    override fun read(reader: JsonReader): BuildTool? =
        if (reader.peek() == com.google.gson.stream.JsonToken.NULL) {
            reader.nextNull()
            null
        } else {
            BuildTool.valueOf(reader.nextString())
        }
}

data class ProjectModel(
    @field:JsonAdapter(BuildToolAdapter::class) val buildTool: BuildTool,
    val rootDir: String,
    val modules: List<BuildModule>,
)

object ErrorCodes {
    const val BUILD_IMPORT_FAILED = -32001
    const val NOT_OPENED = -32002
}

interface BreweryServer {
    @JsonRequest("initialize") fun initialize(params: InitializeParams): CompletableFuture<InitializeResult>
    @JsonRequest("shutdown") fun shutdown(): CompletableFuture<Any?>
    @JsonNotification("exit") fun exit()
    @JsonRequest("project/open") fun openProject(params: OpenProjectParams): CompletableFuture<Any?>
    @JsonRequest("project/model") fun projectModel(params: ProjectModelParams): CompletableFuture<ProjectModel>
    @JsonRequest("spring/model") fun springModel(params: SpringModelParams): CompletableFuture<SpringModel>
    @JsonRequest("run/configurations") fun runConfigurations(params: RunConfigurationsParams): CompletableFuture<List<RunConfiguration>>
    @JsonRequest("db/drivers") fun dbDrivers(params: DbDriversParams): CompletableFuture<List<DriverInfo>>
    @JsonRequest("db/drivers/addJar") fun dbDriversAddJar(params: DbDriversAddJarParams): CompletableFuture<DriverInfo>
    @JsonRequest("db/connections/list") fun dbConnectionsList(params: DbConnectionsListParams): CompletableFuture<List<ConnectionInfo>>
    @JsonRequest("db/connections/save") fun dbConnectionsSave(params: DbConnectionsSaveParams): CompletableFuture<ConnectionInfo>
    @JsonRequest("db/connections/delete") fun dbConnectionsDelete(params: DbConnectionsDeleteParams): CompletableFuture<Any?>
    @JsonRequest("db/connections/test") fun dbConnectionsTest(params: DbConnectionsTestParams): CompletableFuture<DbTestResult>
    @JsonRequest("db/introspect") fun dbIntrospect(params: DbIntrospectParams): CompletableFuture<List<DbNode>>
}

interface BreweryClient {
    @JsonNotification("project/changed") fun projectChanged(params: ProjectChanged)
    @JsonNotification("spring/changed") fun springChanged(params: SpringChanged)
}
