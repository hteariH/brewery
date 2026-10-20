package dev.brewery.daemon

import dev.brewery.daemon.mcp.DbTools
import dev.brewery.daemon.mcp.McpServer
import dev.brewery.daemon.mcp.McpTool
import dev.brewery.daemon.mcp.ProjectTools
import dev.brewery.daemon.springconfig.SpringConfigServer
import org.eclipse.lsp4j.jsonrpc.Launcher
import org.eclipse.lsp4j.launch.LSPLauncher
import java.io.InputStream
import java.io.OutputStream
import java.nio.file.Path
import kotlin.system.exitProcess

fun main(args: Array<String>) {
    // JSON-RPC owns the real stdout; anything else that prints (libraries, stray println) goes to stderr
    // so it can never corrupt the framed stream.
    val rpcOut = System.out
    System.setOut(System.err)
    if ("--mcp" in args) {
        val root = java.nio.file.Paths.get("").toAbsolutePath()
        runMcp(root, System.`in`, rpcOut, mcpTools(root)) { exitProcess(it) }
    } else if ("--spring-config-lsp" in args) {
        runSpringConfigLsp(System.`in`, rpcOut) { exitProcess(it) }
    } else {
        runDaemon(System.`in`, rpcOut) { exitProcess(it) }
    }
}

/** Serves the Spring configuration language server over [input]/[rpcOut] until EOF or the `exit` notification. */
fun runSpringConfigLsp(input: InputStream, rpcOut: OutputStream, exit: (Int) -> Unit) {
    val server = SpringConfigServer(onExit = { exit(0) })
    val launcher = LSPLauncher.createServerLauncher(server, input, rpcOut)
    server.connect(launcher.remoteProxy)
    launcher.startListening().get()
    // The client may vanish without a `shutdown` request; a Maven child process would then outlive us.
    server.shutdown()
    exit(0)
}

/**
 * Serves MCP over [input]/[rpcOut] for the project in [root] until EOF. Zed starts the context server with the
 * active project folder as its working directory and sends no MCP roots, so the working directory is the project.
 */
fun runMcp(root: Path, input: InputStream, rpcOut: OutputStream, tools: List<McpTool>, exit: (Int) -> Unit) {
    System.err.println("brewery mcp: serving $root")
    McpServer(tools, daemonVersion()).serve(input, rpcOut)
    exit(0)
}

/** Every MCP tool for [root]: the project tools and the read-only tools on its Spring datasource. */
fun mcpTools(root: Path): List<McpTool> = ProjectTools(root).tools() + DbTools(root).tools()

private fun daemonVersion(): String = McpServer::class.java.`package`?.implementationVersion ?: "dev"

/**
 * Serves JSON-RPC until [input] reaches EOF, then closes the daemon and calls [exit]. Exiting explicitly
 * matters: Tooling API threads are non-daemon and would otherwise keep the JVM alive after the IDE is gone.
 */
fun runDaemon(input: InputStream, rpcOut: OutputStream, exit: (Int) -> Unit) {
    val daemon = BreweryDaemon()
    try {
        val launcher = Launcher.createLauncher(daemon, BreweryClient::class.java, input, rpcOut)
        daemon.connect(launcher.remoteProxy)
        launcher.startListening().get()
    } finally {
        daemon.close()
    }
    exit(0)
}
