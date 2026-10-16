package dev.brewery.daemon.db

import java.net.URLDecoder
import java.nio.charset.StandardCharsets

/**
 * A connection with the passwords taken out of its URL and options. [url], [user] and [options] are safe to
 * write to `db.json`; [urlPassword] and [optionsPassword] are what was removed (null when there was none).
 */
internal data class WithoutSecrets(
    val url: String,
    val user: String?,
    val options: Map<String, String>,
    val urlPassword: String?,
    val optionsPassword: String?,
) {
    val found: Boolean get() = urlPassword != null || optionsPassword != null
}

/**
 * Finds passwords users type into a JDBC URL or the driver options, so they go to the secret store instead of
 * `db.json`. Recognised: userinfo (`//user:password@host`, the user moves to the user field; neither bundled
 * driver accepts userinfo), query parameters (`?password=`, `&pwd=`), MySQL host descriptions
 * (`address=(host=h)(password=p)`, `(host=h,password=p)`), `;password=` parameters of other drivers, and the
 * option keys `password` / `pwd`, all ignoring case. PostgreSQL percent-decodes query values, so its values are
 * decoded; MariaDB Connector/J does not, so other drivers' values are kept as typed.
 */
internal object EmbeddedSecrets {
    private val SECRET_KEYS = setOf("password", "pwd")
    private val PREFIX = Regex("^jdbc:[A-Za-z0-9+._-]+(?::[A-Za-z0-9+._-]+)*:(?=//)")
    private val GROUP = Regex("""\(([^()]*)\)""")

    fun isSecretKey(key: String): Boolean = key.trim().lowercase() in SECRET_KEYS

    /** Throws `INVALID_PARAMS` when the URL holds two different passwords or a user that contradicts [user]. */
    fun strip(url: String, user: String?, options: Map<String, String>?): WithoutSecrets {
        val passwords = mutableListOf<String>()
        var urlUser: String? = null
        var rest = url
        val decodeQuery = url.startsWith("jdbc:postgresql:")
        val noAuthority = url.startsWith("jdbc:sqlite:")

        // Query string first: `?` cannot appear in an authority.
        val q = rest.indexOf('?')
        var query: String? = null
        if (q >= 0 && !noAuthority) {
            query = rest.substring(q + 1)
            rest = rest.substring(0, q)
        }

        PREFIX.find(rest)?.takeUnless { noAuthority }?.let { match ->
            val start = match.range.last + 3
            val end = rest.indexOfAny(charArrayOf('/', ';'), start).let { if (it < 0) rest.length else it }
            var authority = rest.substring(start, end)
            val at = authority.lastIndexOf('@')
            if (at >= 0 && ':' in authority.substring(0, at)) {
                val info = authority.substring(0, at)
                urlUser = percentDecode(info.substringBefore(':'))
                info.substringAfter(':').takeIf { it.isNotEmpty() }?.let { passwords += percentDecode(it) }
                authority = authority.substring(at + 1)
            }
            authority = GROUP.replace(authority) { group ->
                val kept = group.groupValues[1].split(',').filter { part -> !takeSecret(part, passwords, decode = false) }
                if (kept.isEmpty()) "" else kept.joinToString(",", "(", ")")
            }
            rest = rest.substring(0, start) + authority + rest.substring(end)
        }

        if (!decodeQuery && !noAuthority && !url.startsWith("jdbc:mariadb:") && !url.startsWith("jdbc:mysql:") && ';' in rest) {
            val parts = rest.split(';')
            rest = (listOf(parts.first()) + parts.drop(1).filter { !takeSecret(it, passwords, decode = false) }).joinToString(";")
        }

        if (query != null) {
            val kept = query.split('&').filter { !takeSecret(it, passwords, decodeQuery) }
            rest = if (kept.isEmpty()) rest else rest + "?" + kept.joinToString("&")
        }

        if (passwords.distinct().size > 1) {
            throw DbException(DbErrorCodes.INVALID_PARAMS, "The URL contains two different passwords. Remove them from the URL and enter the password in the Password field.")
        }
        val fieldUser = user?.takeIf { it.isNotBlank() }
        if (urlUser != null && fieldUser != null && urlUser != fieldUser) {
            throw DbException(
                DbErrorCodes.INVALID_PARAMS,
                "The URL names user \"$urlUser\" but the User field says \"$fieldUser\". Remove the user and password from the URL.",
            )
        }

        val optionPasswords = mutableListOf<String>()
        val keptOptions = (options ?: emptyMap()).filter { (k, v) ->
            if (isSecretKey(k)) {
                if (v.isNotEmpty()) optionPasswords += v
                false
            } else {
                true
            }
        }
        return WithoutSecrets(
            url = rest,
            user = fieldUser ?: urlUser?.takeIf { it.isNotEmpty() } ?: user,
            options = keptOptions,
            urlPassword = passwords.firstOrNull(),
            optionsPassword = optionPasswords.firstOrNull(),
        )
    }

    /** True (and the value recorded) when [param] is `password=...` or `pwd=...`; an empty value is dropped too. */
    private fun takeSecret(param: String, into: MutableList<String>, decode: Boolean): Boolean {
        val eq = param.indexOf('=')
        if (eq < 0 || !isSecretKey(param.substring(0, eq))) return false
        val raw = param.substring(eq + 1).trim()
        val value = if (decode) {
            try {
                URLDecoder.decode(raw, StandardCharsets.UTF_8)
            } catch (_: IllegalArgumentException) {
                raw
            }
        } else {
            raw
        }
        if (value.isNotEmpty()) into += value
        return true
    }

    /** `%XX` escapes only (userinfo does not turn `+` into a space); malformed escapes stay as typed. */
    private fun percentDecode(text: String): String {
        if ('%' !in text) return text
        val out = java.io.ByteArrayOutputStream()
        var i = 0
        while (i < text.length) {
            val c = text[i]
            val hex = if (c == '%' && i + 2 < text.length && text[i + 1].isHex() && text[i + 2].isHex()) {
                text.substring(i + 1, i + 3).toInt(16)
            } else {
                null
            }
            if (hex != null) {
                out.write(hex)
                i += 3
            } else {
                out.write(c.toString().toByteArray(StandardCharsets.UTF_8))
                i += 1
            }
        }
        return out.toString(StandardCharsets.UTF_8)
    }

    private fun Char.isHex() = this in '0'..'9' || this in 'a'..'f' || this in 'A'..'F'
}
