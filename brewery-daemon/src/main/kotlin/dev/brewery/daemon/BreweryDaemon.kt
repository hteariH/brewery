package dev.brewery.daemon

import java.util.concurrent.CompletableFuture
import kotlin.system.exitProcess

class BreweryDaemon : BreweryServer {
    private var client: BreweryClient? = null

    fun connect(client: BreweryClient) {
        this.client = client
    }

    override fun initialize(params: InitializeParams): CompletableFuture<InitializeResult> =
        CompletableFuture.completedFuture(InitializeResult(PROTOCOL_VERSION, DAEMON_VERSION))

    override fun shutdown(): CompletableFuture<Any?> = CompletableFuture.completedFuture(null)

    override fun exit() {
        exitProcess(0)
    }

    override fun openProject(params: OpenProjectParams): CompletableFuture<Any?> =
        CompletableFuture.failedFuture(UnsupportedOperationException("implemented in Task 2"))

    override fun projectModel(params: ProjectModelParams): CompletableFuture<ProjectModel> =
        CompletableFuture.failedFuture(UnsupportedOperationException("implemented in Task 2"))
}
