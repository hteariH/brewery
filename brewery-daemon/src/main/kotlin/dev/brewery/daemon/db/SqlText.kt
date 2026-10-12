package dev.brewery.daemon.db

/** Just enough SQL reading to decide how a statement may be run; it never validates SQL. */
object SqlText {
    enum class Kind {
        /** A plain read: SELECT/WITH/VALUES/TABLE/SHOW/EXPLAIN without locking, writing or INTO. */
        READ,

        /** BEGIN / START TRANSACTION: the user takes over transaction control. */
        TX_BEGIN,

        /** COMMIT / END / ROLLBACK / ABORT: the user's transaction is over. */
        TX_END,
        OTHER,
    }

    private val READ_WORDS = setOf("SELECT", "WITH", "VALUES", "TABLE", "SHOW", "EXPLAIN")
    private val NOT_PLAIN = Regex("\\b(FOR\\s+(NO\\s+KEY\\s+)?(UPDATE|SHARE)|FOR\\s+KEY\\s+SHARE|RETURNING|INSERT|UPDATE|DELETE|MERGE|INTO)\\b", RegexOption.IGNORE_CASE)

    fun classify(sql: String): Kind {
        val clean = sanitize(sql)
        val (first, second) = firstWords(clean)
        return when (first) {
            in READ_WORDS -> if (NOT_PLAIN.containsMatchIn(clean)) Kind.OTHER else Kind.READ
            "BEGIN", "START" -> Kind.TX_BEGIN
            "COMMIT", "END", "ROLLBACK", "ABORT" ->
                if (second == "PREPARED" || (first == "ROLLBACK" && second == "TO")) Kind.OTHER else Kind.TX_END
            else -> Kind.OTHER
        }
    }

    /**
     * [sql] with everything that is not code blanked out: string literals ('..' with '' escapes, E'..' with
     * backslash escapes), PostgreSQL dollar-quoted strings, "quoted" and `quoted` identifiers, `--` comments and
     * (nested) block comments. An unterminated literal or comment runs to the end of the text.
     */
    internal fun sanitize(sql: String): String {
        val out = StringBuilder(sql.length)
        var i = 0
        val n = sql.length
        fun blank(to: Int) {
            while (i < to) {
                out.append(' ')
                i++
            }
        }
        while (i < n) {
            val ch = sql[i]
            when {
                ch == '-' && sql.startsWith("--", i) -> {
                    val end = sql.indexOf('\n', i)
                    blank(if (end < 0) n else end)
                }
                ch == '/' && sql.startsWith("/*", i) -> {
                    var depth = 1
                    var j = i + 2
                    while (j < n && depth > 0) {
                        when {
                            sql.startsWith("/*", j) -> { depth++; j += 2 }
                            sql.startsWith("*/", j) -> { depth--; j += 2 }
                            else -> j++
                        }
                    }
                    blank(j)
                }
                ch == '\'' -> {
                    val escapes = i > 0 && (sql[i - 1] == 'E' || sql[i - 1] == 'e') && (i < 2 || !isIdentChar(sql[i - 2]))
                    var j = i + 1
                    while (j < n) {
                        if (escapes && sql[j] == '\\') j += 2
                        else if (sql[j] == '\'') {
                            if (j + 1 < n && sql[j + 1] == '\'') j += 2 else { j++; break }
                        } else j++
                    }
                    blank(minOf(j, n))
                }
                ch == '"' || ch == '`' -> {
                    var j = i + 1
                    while (j < n) {
                        if (sql[j] == ch) {
                            if (j + 1 < n && sql[j + 1] == ch) j += 2 else { j++; break }
                        } else j++
                    }
                    blank(minOf(j, n))
                }
                ch == '$' && (i == 0 || !isIdentChar(sql[i - 1])) -> {
                    val tagEnd = dollarTagEnd(sql, i)
                    if (tagEnd < 0) {
                        out.append(ch)
                        i++
                    } else {
                        val tag = sql.substring(i, tagEnd)
                        val close = sql.indexOf(tag, tagEnd)
                        blank(if (close < 0) n else close + tag.length)
                    }
                }
                else -> {
                    out.append(ch)
                    i++
                }
            }
        }
        return out.toString()
    }

    private fun isIdentChar(c: Char) = c.isLetterOrDigit() || c == '_'

    /** Index just after the closing `$` of a dollar-quote tag starting at [start] (`$$` or `$name$`), or -1. */
    private fun dollarTagEnd(sql: String, start: Int): Int {
        var j = start + 1
        if (j < sql.length && (sql[j].isLetter() || sql[j] == '_')) {
            while (j < sql.length && isIdentChar(sql[j])) j++
        }
        return if (j < sql.length && sql[j] == '$') j + 1 else -1
    }

    /** The first two keywords, upper-cased, after whitespace and opening parentheses of an already [sanitize]d text. */
    internal fun firstWords(clean: String): Pair<String, String> {
        var i = 0
        fun skip(parens: Boolean) {
            while (i < clean.length && (clean[i].isWhitespace() || (parens && clean[i] == '('))) i++
        }
        fun word(): String {
            val start = i
            while (i < clean.length && clean[i].isLetter()) i++
            return clean.substring(start, i).uppercase()
        }
        skip(parens = true)
        val first = word()
        skip(parens = false)
        return first to word()
    }
}
