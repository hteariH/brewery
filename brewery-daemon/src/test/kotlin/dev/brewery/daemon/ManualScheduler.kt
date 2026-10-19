package dev.brewery.daemon

import java.util.concurrent.AbstractExecutorService
import java.util.concurrent.Callable
import java.util.concurrent.Delayed
import java.util.concurrent.ExecutorService
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit

/**
 * A scheduler whose clock only moves when a test calls [advance], so debounce tests assert exact
 * behaviour instead of sleeping and hoping the runner kept up. Due tasks run on the calling thread.
 */
class ManualScheduler : AbstractExecutorService(), ScheduledExecutorService {
    private val lock = Any()
    private var now = 0L
    private var sequence = 0L
    private var shutdown = false
    private val tasks = ArrayList<Task<*>>()

    /** Moves the clock forward and runs every task that falls due, including ones they schedule. */
    fun advance(millis: Long) {
        val target = synchronized(lock) { now + millis }
        while (true) {
            val task = synchronized(lock) {
                val next = tasks.filter { it.at <= target }.minWithOrNull(compareBy({ it.at }, { it.order }))
                if (next == null) {
                    now = target
                    return
                }
                tasks.remove(next)
                now = maxOf(now, next.at)
                next
            }
            task.run()
        }
    }

    /** Runs what is already due without moving the clock. */
    fun runDue() = advance(0)

    val pending: Int get() = synchronized(lock) { tasks.size }

    override fun schedule(command: Runnable, delay: Long, unit: TimeUnit): ScheduledFuture<*> =
        add(unit.toMillis(delay)) { command.run() }

    override fun <V> schedule(callable: Callable<V>, delay: Long, unit: TimeUnit): ScheduledFuture<V> =
        add(unit.toMillis(delay)) { callable.call() }

    override fun execute(command: Runnable) {
        add(0) { command.run() }
    }

    private fun <V> add(delayMillis: Long, body: () -> V): ScheduledFuture<V> = synchronized(lock) {
        if (shutdown) throw RejectedExecutionException("shut down")
        Task(now + maxOf(0, delayMillis), sequence++, body).also { tasks += it }
    }

    override fun scheduleAtFixedRate(command: Runnable, initialDelay: Long, period: Long, unit: TimeUnit): ScheduledFuture<*> =
        throw UnsupportedOperationException()

    override fun scheduleWithFixedDelay(command: Runnable, initialDelay: Long, delay: Long, unit: TimeUnit): ScheduledFuture<*> =
        throw UnsupportedOperationException()

    override fun shutdown() {
        synchronized(lock) { shutdown = true }
    }

    override fun shutdownNow(): List<Runnable> = synchronized(lock) {
        shutdown = true
        tasks.toList().also { tasks.clear() }
    }

    override fun isShutdown() = synchronized(lock) { shutdown }

    override fun isTerminated() = isShutdown

    override fun awaitTermination(timeout: Long, unit: TimeUnit) = true

    private inner class Task<V>(val at: Long, val order: Long, private val body: () -> V) : ScheduledFuture<V>, Runnable {
        private var state = 0 // 0 pending, 1 done, 2 cancelled
        private var value: V? = null

        override fun run() {
            if (synchronized(lock) { state != 0 }) return
            val result = body()
            synchronized(lock) {
                value = result
                state = 1
            }
        }

        override fun cancel(mayInterruptIfRunning: Boolean): Boolean = synchronized(lock) {
            if (state != 0) return false
            state = 2
            tasks.remove(this)
            true
        }

        override fun isCancelled() = synchronized(lock) { state == 2 }

        override fun isDone() = synchronized(lock) { state != 0 }

        override fun get(): V = synchronized(lock) {
            check(state == 1) { "the task has not run; advance the clock first" }
            @Suppress("UNCHECKED_CAST")
            value as V
        }

        override fun get(timeout: Long, unit: TimeUnit): V = get()

        override fun getDelay(unit: TimeUnit): Long = unit.convert(at - synchronized(lock) { now }, TimeUnit.MILLISECONDS)

        override fun compareTo(other: Delayed): Int =
            getDelay(TimeUnit.MILLISECONDS).compareTo(other.getDelay(TimeUnit.MILLISECONDS))
    }
}

/** Runs every task on the submitting thread, so its effects are visible when the call returns. */
class InlineExecutor : AbstractExecutorService(), ExecutorService {
    @Volatile
    private var shutdown = false

    override fun execute(command: Runnable) {
        if (shutdown) throw RejectedExecutionException("shut down")
        command.run()
    }

    override fun shutdown() {
        shutdown = true
    }

    override fun shutdownNow(): List<Runnable> {
        shutdown = true
        return emptyList()
    }

    override fun isShutdown() = shutdown

    override fun isTerminated() = shutdown

    override fun awaitTermination(timeout: Long, unit: TimeUnit) = true
}
