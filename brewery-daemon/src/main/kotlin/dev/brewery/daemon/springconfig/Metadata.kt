package dev.brewery.daemon.springconfig

import com.google.gson.JsonElement
import com.google.gson.JsonObject
import com.google.gson.JsonParser
import java.io.File
import java.util.zip.ZipFile

data class Deprecation(val level: String, val replacement: String?, val reason: String?)

data class ConfigProperty(
    val name: String,
    val type: String?,
    val description: String?,
    val defaultValue: String?,
    val deprecation: Deprecation?,
) {
    val isMap: Boolean get() = simpleType?.let(MAP_TYPE::matches) == true
    val simpleType: String? get() = type?.substringBefore('<')?.substringAfterLast('.')?.substringAfterLast('$')

    /**
     * True when Spring binds this property to something without sub keys of its own: a scalar, or a list, set or
     * array of scalars. Any other type may be an object whose fields the metadata does not describe.
     */
    val isLeafType: Boolean
        get() {
            val t = type?.replace(" ", "") ?: return false
            if (t.endsWith("[]")) return t.removeSuffix("[]") in SCALAR_TYPES
            val element = COLLECTION_TYPE.matchEntire(t)?.groupValues?.get(1)
            return (element ?: t) in SCALAR_TYPES
        }
}

data class HintValue(val value: String, val description: String?)

data class ConfigHint(val name: String, val values: List<HintValue>)

// Matched against the simple type name, so any package and any generic arguments are accepted.
private val MAP_TYPE = Regex("""\w*Map|Properties""")
private val COLLECTION_TYPE = Regex("""java\.util\.(?:List|Set|SortedSet|Collection)<([^<>]+)>""")

private val SCALAR_TYPES = setOf(
    "boolean", "byte", "char", "short", "int", "long", "float", "double",
    "java.lang.Boolean", "java.lang.Byte", "java.lang.Character", "java.lang.Short", "java.lang.Integer",
    "java.lang.Long", "java.lang.Float", "java.lang.Double", "java.lang.String", "java.lang.Class",
    "java.lang.Class<?>", "java.math.BigDecimal", "java.math.BigInteger", "java.nio.charset.Charset",
    "java.time.Duration", "java.time.Period", "java.util.Locale", "java.util.TimeZone", "java.util.UUID",
    "java.util.regex.Pattern", "java.io.File", "java.nio.file.Path", "java.net.URI", "java.net.URL",
    "java.net.InetAddress", "org.springframework.util.unit.DataSize", "org.springframework.core.io.Resource",
    "org.springframework.util.MimeType", "org.springframework.http.MediaType",
)

private const val MAIN_FILE = "META-INF/spring-configuration-metadata.json"
private const val ADDITIONAL_FILE = "META-INF/additional-spring-configuration-metadata.json"

/**
 * Spring Boot's relaxed binding compares keys without case, `-` and `_`, and ignores list indices; an
 * environment-variable name (no dots, upper case) uses `_` where a property uses `.`.
 */
fun canonicalKey(key: String): String {
    val dotted = if ('.' !in key && '_' in key && key == key.uppercase()) key.replace('_', '.') else key
    return dotted.replace(Regex("""\[[^\]]*]"""), "").replace("-", "").replace("_", "").lowercase()
}

class SpringMetadata private constructor(
    private val byKey: Map<String, ConfigProperty>,
    private val hintsByKey: Map<String, ConfigHint>,
) {
    /** Canonical dotted prefixes of the known properties: `server` and `server.servlet` for `server.servlet.path`. */
    private val groupPrefixes: Set<String> = buildSet {
        for (key in byKey.keys) {
            var dot = key.indexOf('.')
            while (dot > 0) {
                add(key.substring(0, dot))
                dot = key.indexOf('.', dot + 1)
            }
        }
    }

    val properties: Collection<ConfigProperty> get() = byKey.values
    val hints: Collection<ConfigHint> get() = hintsByKey.values
    val isEmpty: Boolean get() = byKey.isEmpty()

    fun find(key: String): ConfigProperty? = byKey[canonicalKey(key)]

    fun hint(key: String): ConfigHint? = hintsByKey[canonicalKey(key)]

    /** True for a key strictly below a map-typed property, where any sub key is legitimate. */
    fun isMapKey(key: String): Boolean = ancestors(key).any { it.isMap }

    /**
     * True only for a key the metadata proves wrong. Keys below a map, or below an object type whose fields the
     * metadata does not list (`List<RouteDefinition>`, a POJO), cannot be checked; a key that only groups known
     * properties (`server`) is not a mistake either.
     */
    fun isUnknown(key: String): Boolean {
        if (find(key) != null || canonicalKey(key) in groupPrefixes) return false
        return ancestors(key).none { it.isMap || !it.isLeafType && canonicalKey(it.name) !in groupPrefixes }
    }

    /** Known properties that [key] lies strictly below, nearest first. */
    fun ancestors(key: String): List<ConfigProperty> {
        val found = ArrayList<ConfigProperty>()
        var prefix = canonicalKey(key)
        while (true) {
            val dot = prefix.lastIndexOf('.')
            if (dot < 0) break
            prefix = prefix.substring(0, dot)
            byKey[prefix]?.let(found::add)
        }
        // `a.b[x.y]` keeps the bracket content as one segment, so the part before the bracket is a parent too.
        val bracket = key.indexOf('[')
        if (bracket > 0) byKey[canonicalKey(key.substring(0, bracket))]?.takeUnless { it in found }?.let(found::add)
        return found
    }

    companion object {
        val EMPTY = SpringMetadata(emptyMap(), emptyMap())

        /**
         * Merges metadata from classpath entries in order, the first definition of a key winning: jars, and
         * directories holding `META-INF/` (a module's resources or build output). Unreadable entries are skipped.
         * Without any jar metadata the classpath was not resolved, and project metadata on its own would make
         * every Spring Boot key look unknown, so the result is then empty.
         */
        fun fromClasspath(entries: List<File>): SpringMetadata {
            val props = LinkedHashMap<String, ConfigProperty>()
            val hints = LinkedHashMap<String, ConfigHint>()
            var fromJars = false
            for (entry in entries) {
                val directory = entry.isDirectory
                val (entryProps, entryHints) = if (directory) readDirectory(entry) else readJar(entry)
                if (!directory && entryProps.isNotEmpty()) fromJars = true
                for (p in entryProps) props.putIfAbsent(canonicalKey(p.name), p)
                for (h in entryHints) hints.putIfAbsent(canonicalKey(h.name), h)
            }
            if (!fromJars) return EMPTY
            return SpringMetadata(props, hints)
        }

        private fun readDirectory(dir: File): Pair<Collection<ConfigProperty>, Collection<ConfigHint>> {
            val props = LinkedHashMap<String, ConfigProperty>()
            val hints = LinkedHashMap<String, ConfigHint>()
            // The additional file is read last so a hand-written entry overrides a generated one.
            for (name in listOf(MAIN_FILE, ADDITIONAL_FILE)) {
                val file = File(dir, name)
                if (!file.isFile) continue
                val root = try {
                    JsonParser.parseString(file.readText())
                } catch (_: Exception) {
                    continue
                }
                if (root.isJsonObject) parse(root.asJsonObject, props, hints)
            }
            return props.values to hints.values
        }

        private fun readJar(jar: File): Pair<Collection<ConfigProperty>, Collection<ConfigHint>> {
            val props = LinkedHashMap<String, ConfigProperty>()
            val hints = LinkedHashMap<String, ConfigHint>()
            val zip = try {
                ZipFile(jar)
            } catch (_: Exception) {
                return props.values to hints.values
            }
            zip.use {
                for (name in listOf(MAIN_FILE, ADDITIONAL_FILE)) {
                    val entry = zip.getEntry(name) ?: continue
                    val root = try {
                        JsonParser.parseString(zip.getInputStream(entry).readBytes().toString(Charsets.UTF_8))
                    } catch (_: Exception) {
                        continue
                    }
                    if (!root.isJsonObject) continue
                    parse(root.asJsonObject, props, hints)
                }
            }
            return props.values to hints.values
        }

        private fun parse(root: JsonObject, props: MutableMap<String, ConfigProperty>, hints: MutableMap<String, ConfigHint>) {
            for (el in root.array("properties")) {
                val o = el.objectOrNull() ?: continue
                val name = o.text("name") ?: continue
                props[canonicalKey(name)] = ConfigProperty(
                    name = name,
                    type = o.text("type"),
                    description = o.text("description"),
                    defaultValue = o.get("defaultValue")?.takeUnless { it.isJsonNull }?.let(::render),
                    // Old metadata only had a top-level `"deprecated": true`.
                    deprecation = o.get("deprecation")?.let(::deprecation)
                        ?: Deprecation("warning", null, null).takeIf { o.get("deprecated")?.let(::isTrue) == true },
                )
            }
            for (el in root.array("hints")) {
                val o = el.objectOrNull() ?: continue
                val name = o.text("name") ?: continue
                val values = o.array("values").mapNotNull { v ->
                    val vo = v.objectOrNull() ?: return@mapNotNull null
                    val value = vo.get("value")?.takeUnless { it.isJsonNull }?.let(::render) ?: return@mapNotNull null
                    HintValue(value, vo.text("description"))
                }
                hints[canonicalKey(name)] = ConfigHint(name, values)
            }
        }

        private fun deprecation(el: JsonElement): Deprecation? {
            if (el.isJsonPrimitive) return if (el.asString == "true") Deprecation("warning", null, null) else null
            val o = el.objectOrNull() ?: return null
            return Deprecation(o.text("level") ?: "warning", o.text("replacement"), o.text("reason"))
        }

        private fun isTrue(el: JsonElement): Boolean = el.isJsonPrimitive && el.asString == "true"

        private fun render(el: JsonElement): String = if (el.isJsonPrimitive) el.asString else el.toString()

        private fun JsonElement.objectOrNull(): JsonObject? = if (isJsonObject) asJsonObject else null

        private fun JsonObject.array(name: String): List<JsonElement> =
            get(name)?.takeIf { it.isJsonArray }?.asJsonArray?.toList().orEmpty()

        private fun JsonObject.text(name: String): String? =
            get(name)?.takeIf { it.isJsonPrimitive }?.asString
    }
}
