package dev.brewery.daemon

import org.eclipse.lsp4j.jsonrpc.services.JsonNotification
import org.eclipse.lsp4j.jsonrpc.services.JsonRequest
import java.util.concurrent.CompletableFuture

const val PROTOCOL_VERSION = 1
const val DAEMON_VERSION = "0.1.0"

data class InitializeParams(val protocolVersion: Int)
data class InitializeResult(val protocolVersion: Int, val daemonVersion: String)
data class OpenProjectParams(val rootDir: String)
class ProjectModelParams
class ProjectChanged

enum class BuildTool { gradle, maven, none }
data class BuildTask(val name: String, val group: String?, val description: String?)
data class BuildModule(val name: String, val path: String, val tasks: List<BuildTask>)
data class ProjectModel(val buildTool: BuildTool, val rootDir: String, val modules: List<BuildModule>)

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
}

interface BreweryClient {
    @JsonNotification("project/changed") fun projectChanged(params: ProjectChanged)
}
