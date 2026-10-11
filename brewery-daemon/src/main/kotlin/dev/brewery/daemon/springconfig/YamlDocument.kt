package dev.brewery.daemon.springconfig

/** One `key:` line of a YAML file; [path] is the full dotted path made of the enclosing keys. */
data class YamlEntry(
    val path: List<String>,
    val line: Int,
    val indent: Int,
    val keyStart: Int,
    val keyEnd: Int,
    val colon: Int,
    val value: String,
    val hasChildren: Boolean,
    val document: Int,
) {
    val key: String get() = path.joinToString(".")
}

/**
 * Indentation-based reader for the YAML subset Spring configuration uses: block mappings, `- ` sequences and
 * `---` document separators. Flow style (`{a: 1}`), anchors and multi-line plain scalars are not understood.
 * Keys inside a sequence item continue the path of the key that owns the sequence, which is how relaxed
 * binding sees `list[0].name`.
 */
class YamlDocument(text: String) {
    private val lines = text.split(Regex("\r\n|\n|\r"))
    private val separators = ArrayList<Int>()
    val entries: List<YamlEntry>

    init {
        val found = ArrayList<YamlEntry>()
        // Open ancestors as (indent, entry index); the entry is replaced once it learns it has children.
        val stack = ArrayList<Pair<Int, Int>>()
        var document = 0
        var blockScalarIndent = -1
        val mutable = ArrayList<MutableEntry>()
        for ((index, raw) in lines.withIndex()) {
            if (raw.startsWith("---") && raw.drop(3).isBlank() || raw.startsWith("--- ")) {
                separators.add(index)
                document++
                stack.clear()
                blockScalarIndent = -1
                continue
            }
            val content = contentStart(raw)
            if (content >= raw.length || raw[content] == '#') continue
            // Lines of a `|` or `>` block belong to the scalar, whatever they look like.
            if (blockScalarIndent >= 0) {
                if (content > blockScalarIndent) continue
                blockScalarIndent = -1
            }
            val keyInfo = readKey(raw, content) ?: continue
            while (stack.isNotEmpty() && stack.last().first >= content) stack.removeLast()
            val parent = stack.lastOrNull()?.second?.let { mutable[it] }
            parent?.hasChildren = true
            val path = (parent?.path ?: emptyList()) + keyInfo.name
            val value = stripComment(raw.substring(keyInfo.colon + 1)).trim()
            val entry = MutableEntry(path, index, content, keyInfo.start, keyInfo.end, keyInfo.colon, value, document)
            mutable.add(entry)
            stack.add(content to mutable.lastIndex)
            if (value.startsWith("|") || value.startsWith(">")) blockScalarIndent = content
        }
        entries = mutable.map {
            YamlEntry(it.path, it.line, it.indent, it.keyStart, it.keyEnd, it.colon, it.value, it.hasChildren, it.document)
        }
    }

    private class MutableEntry(
        val path: List<String>, val line: Int, val indent: Int, val keyStart: Int, val keyEnd: Int,
        val colon: Int, val value: String, val document: Int, var hasChildren: Boolean = false,
    )

    private class KeyInfo(val name: String, val start: Int, val end: Int, val colon: Int)

    /** Column where the content of the line starts, after the indentation and any `- ` markers. */
    private fun contentStart(line: String): Int {
        var i = 0
        while (true) {
            while (i < line.length && line[i] == ' ') i++
            if (i < line.length && line[i] == '-' && (i + 1 == line.length || line[i + 1] == ' ')) i++ else return i
        }
    }

    private fun readKey(line: String, start: Int): KeyInfo? {
        if (start >= line.length) return null
        val first = line[start]
        if (first == '"' || first == '\'') {
            val close = line.indexOf(first, start + 1)
            if (close < 0) return null
            var i = close + 1
            while (i < line.length && line[i] == ' ') i++
            if (i >= line.length || line[i] != ':') return null
            return KeyInfo(line.substring(start + 1, close), start, close + 1, i)
        }
        var i = start
        while (i < line.length) {
            val c = line[i]
            if (c == '#' && i > start && line[i - 1] == ' ') return null
            if (c == ':' && (i + 1 == line.length || line[i + 1] == ' ')) {
                val name = line.substring(start, i).trimEnd()
                if (name.isEmpty()) return null
                return KeyInfo(name, start, start + name.length, i)
            }
            i++
        }
        return null
    }

    private fun stripComment(text: String): String {
        var quote = 0.toChar()
        for (i in text.indices) {
            val c = text[i]
            if (quote != 0.toChar()) {
                if (c == quote) quote = 0.toChar()
            } else if (c == '"' || c == '\'') quote = c
            else if (c == '#' && (i == 0 || text[i - 1] == ' ')) return text.substring(0, i)
        }
        return text
    }

    /** Index of the YAML document (separated by `---`) the line belongs to. */
    fun documentOf(line: Int): Int = separators.count { it < line }

    /** The entry whose key covers the position. */
    fun keyAt(line: Int, col: Int): YamlEntry? =
        entries.firstOrNull { it.line == line && col in it.keyStart..it.keyEnd }

    /** Path of the keys enclosing a new key at [indent] on [line], i.e. what is already open above it. */
    fun parentPath(line: Int, indent: Int): List<String> {
        val doc = documentOf(line)
        val open = ArrayList<YamlEntry>()
        for (e in entries) {
            if (e.line >= line) break
            if (e.document != doc) continue
            while (open.isNotEmpty() && open.last().indent >= e.indent) open.removeLast()
            open.add(e)
        }
        while (open.isNotEmpty() && open.last().indent >= indent) open.removeLast()
        // The path of the innermost open entry already contains all of its ancestors.
        return open.lastOrNull()?.path ?: emptyList()
    }

    fun contextAt(line: Int, col: Int): ConfigContext? {
        val raw = lines.getOrNull(line) ?: return null
        if (raw.trimStart().startsWith("#") || raw.startsWith("---")) return null
        val content = contentStart(raw)
        val entry = entries.firstOrNull { it.line == line }
        if (entry != null && col > entry.colon) {
            val valueStart = entry.colon + 1 + raw.substring(entry.colon + 1).takeWhile { it == ' ' }.length
            val start = if (col >= valueStart) valueStart else col
            return ConfigContext.Value(entry.key, start, maxOf(col, valueStart + entry.value.length))
        }
        // A cursor left of the content (in the indentation) types a key at that column.
        val start = if (col >= content) content else col
        var end = if (entry != null) entry.keyEnd else col
        if (entry == null) while (end < raw.length && raw[end] != ' ' && raw[end] != ':') end++
        return ConfigContext.Key(parentPath(line, start), start, maxOf(end, col))
    }
}
