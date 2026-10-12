package dev.brewery.daemon.db

import com.github.javakeyring.Keyring

/** The platform could not provide a secret store (no keyring backend, locked, denied). */
class SecretStoreUnavailable(message: String) : Exception(message)

/**
 * Passwords live in the OS secret store under service [SERVICE] and an account built by [DbService]
 * (`<project root>#<connectionId>`). [get] returns null when nothing is stored; the other calls throw
 * [SecretStoreUnavailable] when the store cannot be used.
 */
interface SecretStore {
    fun get(account: String): String?

    @Throws(SecretStoreUnavailable::class)
    fun set(account: String, password: String)

    @Throws(SecretStoreUnavailable::class)
    fun remove(account: String)

    companion object {
        const val SERVICE = "brewery-db"
    }
}

/** Test fake, also usable where no keyring exists. */
class InMemorySecretStore : SecretStore {
    private val values = java.util.concurrent.ConcurrentHashMap<String, String>()
    override fun get(account: String): String? = values[account]
    override fun set(account: String, password: String) {
        values[account] = password
    }

    override fun remove(account: String) {
        values.remove(account)
    }
}

/** A store that never works; used to exercise the "not stored" fallback. */
class UnavailableSecretStore(private val reason: String) : SecretStore {
    override fun get(account: String): String? = null
    override fun set(account: String, password: String): Unit = throw SecretStoreUnavailable(reason)
    override fun remove(account: String): Unit = throw SecretStoreUnavailable(reason)
}

/** java-keyring: Windows Credential Manager, macOS Keychain, libsecret/KWallet. Opened on first use. */
class KeyringSecretStore : SecretStore {
    private var keyring: Keyring? = null
    private var failure: String? = null

    @Synchronized
    private fun open(): Keyring {
        keyring?.let { return it }
        failure?.let { throw SecretStoreUnavailable(it) }
        return try {
            Keyring.create().also { keyring = it }
        } catch (e: Throwable) {
            val reason = "OS secret store unavailable (${e.javaClass.simpleName}: ${e.message ?: "no details"})"
            failure = reason
            throw SecretStoreUnavailable(reason)
        }
    }

    override fun get(account: String): String? = try {
        // java-keyring reports a missing entry as an exception too, so any failure reads as "nothing stored".
        open().getPassword(SecretStore.SERVICE, account)
    } catch (_: Throwable) {
        null
    }

    override fun set(account: String, password: String) {
        val k = open()
        try {
            k.setPassword(SecretStore.SERVICE, account, password)
        } catch (e: Throwable) {
            throw SecretStoreUnavailable("Could not store the password in the OS secret store (${e.javaClass.simpleName})")
        }
    }

    override fun remove(account: String) {
        val k = open()
        try {
            k.deletePassword(SecretStore.SERVICE, account)
        } catch (_: Throwable) {
            // Deleting an entry that does not exist is a failure in java-keyring and a success for us.
        }
    }
}
