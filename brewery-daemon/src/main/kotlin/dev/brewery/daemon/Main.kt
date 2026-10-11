package dev.brewery.daemon

import dev.brewery.daemon.springconfig.SpringConfigServer
import org.eclipse.lsp4j.jsonrpc.Launcher
import org.eclipse.lsp4j.launch.LSPLauncher
import java.io.InputStream
import java.io.OutputStream
import kotlin.system.exitProcess

fun main(args: Array<String>) {
    // JSON-RPC owns the real stdout; anything else that prints (libraries, stray println) goes to stderr
    // so it can never corrupt the framed stream.
    val rpcOut = System.out
    System.setOut(System.err)
    if ("--spring-config-lsp" in args) {
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
    exit(0)
}

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
