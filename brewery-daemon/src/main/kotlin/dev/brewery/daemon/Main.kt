package dev.brewery.daemon

import org.eclipse.lsp4j.jsonrpc.Launcher

fun main() {
    val daemon = BreweryDaemon()
    val launcher = Launcher.createLauncher(daemon, BreweryClient::class.java, System.`in`, System.out)
    daemon.connect(launcher.remoteProxy)
    launcher.startListening().get()
}
