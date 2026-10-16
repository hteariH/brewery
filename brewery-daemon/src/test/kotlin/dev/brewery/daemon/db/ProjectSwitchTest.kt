package dev.brewery.daemon.db

import dev.brewery.daemon.BreweryDaemon
import dev.brewery.daemon.OpenProjectParams
import java.nio.file.Files
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/** Opening another project closes the old database service without holding up other requests. */
class ProjectSwitchTest {
    private val services = CopyOnWriteArrayList<DbService>()
    private val daemon = BreweryDaemon(secretStore = InMemorySecretStore(), dbServiceFactory = { root ->
        DbService(root, InMemorySecretStore()).also(services::add)
    })
    private val release = CountDownLatch(1)

    @AfterTest
    fun tearDown() {
        release.countDown()
        daemon.close()
    }

    @Test
    fun `a busy session of the old project does not block the switch`() {
        val first = Files.createTempDirectory("brewery-switch-a")
        val second = Files.createTempDirectory("brewery-switch-b")
        daemon.openProject(OpenProjectParams(first.toString())).get(10, TimeUnit.SECONDS)
        val db = first.resolve("app.db")
        val saved = daemon.dbConnectionsSave(
            DbConnectionsSaveParams(ConnectionInfo("", "lite", "sqlite", "jdbc:sqlite:$db", null, false, emptyMap()), null),
        ).get(10, TimeUnit.SECONDS)
        // Something on the session thread that a cancel cannot stop, like a driver stuck in a network call.
        val busy = CountDownLatch(1)
        val old = services.single()
        Thread {
            runCatching { old.sessions.run(saved.id) { busy.countDown(); release.await(20, TimeUnit.SECONDS) } }
        }.apply { isDaemon = true }.start()
        assertTrue(busy.await(10, TimeUnit.SECONDS))

        val started = System.nanoTime()
        daemon.openProject(OpenProjectParams(second.toString())).get(10, TimeUnit.SECONDS)
        assertEquals(emptyList(), daemon.dbConnectionsList(DbConnectionsListParams()).get(10, TimeUnit.SECONDS))
        val elapsedMs = (System.nanoTime() - started) / 1_000_000
        assertTrue(elapsedMs < 2_000, "switching took $elapsedMs ms")

        // The old service is still closed, in the background.
        release.countDown()
        val deadline = System.nanoTime() + 10_000_000_000L
        while (old.sessions.openCount > 0 && System.nanoTime() < deadline) Thread.sleep(20)
        assertEquals(0, old.sessions.openCount)
    }
}
