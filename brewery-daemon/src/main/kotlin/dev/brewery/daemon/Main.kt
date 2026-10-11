package dev.brewery.daemon

import org.eclipse.lsp4j.jsonrpc.Launcher
import java.io.InputStream
import java.io.OutputStream
import kotlin.system.exitProcess

fun main() {
    // JSON-RPC owns the real stdout; anything else that prints (libraries, stray println) goes to stderr
    // so it can never corrupt the framed stream.
    val rpcOut = System.out
    System.setOut(System.err)
    runDaemon(System.`in`, rpcOut) { exitProcess(it) }
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
