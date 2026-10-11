package dev.brewery.daemon

import java.io.ByteArrayOutputStream
import java.io.PipedInputStream
import java.io.PipedOutputStream
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import kotlin.test.Test
import kotlin.test.assertEquals

class RunDaemonTest {
    @Test
    fun `closing stdin exits with code zero`() {
        val feed = PipedOutputStream()
        val input = PipedInputStream(feed)
        val exitCode = CompletableFuture<Int>()
        val thread = Thread { runDaemon(input, ByteArrayOutputStream()) { exitCode.complete(it) } }
        thread.isDaemon = true
        thread.start()
        feed.close()
        assertEquals(0, exitCode.get(10, TimeUnit.SECONDS))
    }
}
