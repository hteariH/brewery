package dev.brewery.daemon.springconfig

/** One `key=value` line of a `.properties` file; columns are 0-based UTF-16 offsets in the line. */
data class PropertyEntry(
    val key: String,
    val line: Int,
    val keyStart: Int,
    val keyEnd: Int,
    /** Column of the first separator character (`=`, `:` or the whitespace after the key); -1 when the line has none. */
    val separatorStart: Int,
    /** Column where the value begins; equals the line length when the value is empty, -1 without separator. */
    val valueStart: Int,
    val value: String,
    val lineLength: Int,
)

/** What is being typed at a cursor position in a properties file. */
sealed interface ConfigContext {
    /** A key (or, in YAML, a key segment) being typed; [startCol]..[endCol] is the token to replace. */
    data class Key(val parentPath: List<String>, val startCol: Int, val endCol: Int) : ConfigContext

    data class Value(val key: String, val startCol: Int, val endCol: Int) : ConfigContext
}

/**
 * Line-based reader for `.properties` files. It only needs keys and value positions, so continuation lines are
 * skipped rather than joined and escapes are honoured just enough to find where the key ends.
 */
class PropertiesDocument(text: String) {
    private val lines = text.split(Regex("\r\n|\n|\r"))
    val entries: List<PropertyEntry>

    init {
        val found = ArrayList<PropertyEntry>()
        var continued = false
        for ((index, line) in lines.withIndex()) {
            val wasContinued = continued
            continued = line.takeLastWhile { it == BACKSLASH }.length % 2 == 1
            if (wasContinued) continue
            parseLine(index, line)?.let(found::add)
        }
        entries = found
    }

    private fun parseLine(index: Int, line: String): PropertyEntry? {
        var i = 0
        while (i < line.length && line[i].isWhitespace()) i++
        if (i >= line.length || line[i] == '#' || line[i] == '!') return null
        val keyStart = i
        while (i < line.length) {
            val c = line[i]
            if (c == BACKSLASH) { i += 2; continue }
            if (c == '=' || c == ':' || c.isWhitespace()) break
            i++
        }
        i = i.coerceAtMost(line.length)
        val keyEnd = i
        val key = line.substring(keyStart, keyEnd)
        if (i >= line.length) return PropertyEntry(key, index, keyStart, keyEnd, -1, -1, "", line.length)
        val separatorStart = i
        while (i < line.length && line[i].isWhitespace()) i++
        if (i < line.length && (line[i] == '=' || line[i] == ':')) {
            i++
            while (i < line.length && line[i].isWhitespace()) i++
        }
        val value = line.substring(i).trimEnd()
        return PropertyEntry(key, index, keyStart, keyEnd, separatorStart, i, value, line.length)
    }

    fun entryOnLine(line: Int): PropertyEntry? = entries.firstOrNull { it.line == line }

    /** The entry whose key covers the position (the end column counts, as for a caret right after the key). */
    fun keyAt(line: Int, col: Int): PropertyEntry? =
        entryOnLine(line)?.takeIf { col in it.keyStart..it.keyEnd }

    fun contextAt(line: Int, col: Int): ConfigContext? {
        val entry = entryOnLine(line)
        if (entry == null) {
            val text = lines.getOrNull(line) ?: return null
            val trimmed = text.trimStart()
            if (trimmed.startsWith("#") || trimmed.startsWith("!")) return null
            return ConfigContext.Key(emptyList(), col, col)
        }
        if (col <= entry.keyEnd) {
            return if (col >= entry.keyStart) ConfigContext.Key(emptyList(), entry.keyStart, entry.keyEnd)
            else ConfigContext.Key(emptyList(), col, col)
        }
        if (entry.separatorStart < 0 || col <= entry.separatorStart) return null
        val start = if (col >= entry.valueStart) entry.valueStart else col
        return ConfigContext.Value(entry.key, start, maxOf(col, entry.valueStart + entry.value.length))
    }
}

private val BACKSLASH = 92.toChar()
