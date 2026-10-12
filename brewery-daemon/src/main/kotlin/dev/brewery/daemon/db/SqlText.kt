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
        val (first, second) = firstWords(sql)
        return when (first) {
            in READ_WORDS -> if (NOT_PLAIN.containsMatchIn(sql)) Kind.OTHER else Kind.READ
            "BEGIN", "START" -> Kind.TX_BEGIN
            "COMMIT", "END", "ROLLBACK", "ABORT" ->
                if (second == "PREPARED" || (first == "ROLLBACK" && second == "TO")) Kind.OTHER else Kind.TX_END
            else -> Kind.OTHER
        }
    }

    /** The first two keywords, upper-cased, after whitespace, comments and opening parentheses. */
    internal fun firstWords(sql: String): Pair<String, String> {
        var i = 0
        fun skipBlanks(parens: Boolean) {
            while (i < sql.length) {
                val ch = sql[i]
                when {
                    ch.isWhitespace() || (parens && ch == '(') -> i++
                    sql.startsWith("--", i) -> {
                        val end = sql.indexOf('\n', i)
                        i = if (end < 0) sql.length else end + 1
                    }
                    sql.startsWith("/*", i) -> {
                        val end = sql.indexOf("*/", i + 2)
                        i = if (end < 0) sql.length else end + 2
                    }
                    else -> return
                }
            }
        }
        fun word(): String {
            val start = i
            while (i < sql.length && sql[i].isLetter()) i++
            return sql.substring(start, i).uppercase()
        }
        skipBlanks(parens = true)
        val first = word()
        skipBlanks(parens = false)
        return first to word()
    }
}
