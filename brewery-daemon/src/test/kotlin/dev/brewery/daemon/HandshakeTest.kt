package dev.brewery.daemon

import org.eclipse.lsp4j.jsonrpc.Launcher
import org.junit.jupiter.api.Test
import java.io.PipedInputStream
import java.io.PipedOutputStream
import java.util.concurrent.TimeUnit
import kotlin.test.assertEquals

class HandshakeTest {
    @Test
    fun `initialize returns protocol and daemon version`() {
        val clientToServer = PipedOutputStream()
        val serverIn = PipedInputStream(clientToServer)
        val serverToClient = PipedOutputStream()
        val clientIn = PipedInputStream(serverToClient)

        val daemon = BreweryDaemon()
        val serverLauncher = Launcher.createLauncher(daemon, BreweryClient::class.java, serverIn, serverToClient)
        daemon.connect(serverLauncher.remoteProxy)
        serverLauncher.startListening()

        val clientLauncher = Launcher.createLauncher(object : BreweryClient {
            override fun projectChanged(params: ProjectChanged) {}
        }, BreweryServer::class.java, clientIn, clientToServer)
        clientLauncher.startListening()

        val result = clientLauncher.remoteProxy.initialize(InitializeParams(PROTOCOL_VERSION)).get(10, TimeUnit.SECONDS)
        assertEquals(PROTOCOL_VERSION, result.protocolVersion)
        assertEquals("0.1.0", result.daemonVersion)
    }
}
