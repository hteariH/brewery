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
    val isMap: Boolean get() = type != null && MAP_TYPE.containsMatchIn(type)
    val simpleType: String? get() = type?.substringBefore('<')?.substringAfterLast('.')?.substringAfterLast('$')
}

data class HintValue(val value: String, val description: String?)

data class ConfigHint(val name: String, val values: List<HintValue>)

private val MAP_TYPE = Regex("""^(java\.util\.)?(\w*Map)\b""")

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
    val properties: Collection<ConfigProperty> get() = byKey.values
    val hints: Collection<ConfigHint> get() = hintsByKey.values
    val isEmpty: Boolean get() = byKey.isEmpty()

    fun find(key: String): ConfigProperty? = byKey[canonicalKey(key)]

    fun hint(key: String): ConfigHint? = hintsByKey[canonicalKey(key)]

    /** True for a key strictly below a map-typed property, where any sub key is legitimate. */
    fun isMapKey(key: String): Boolean {
        // `a.b[x.y]` keeps the bracket content as one segment, so cut it off before looking at dots.
        val bracket = key.indexOf('[')
        val canonical = canonicalKey(key)
        var prefix = canonical
        while (true) {
            val dot = prefix.lastIndexOf('.')
            if (dot < 0) break
            prefix = prefix.substring(0, dot)
            if (byKey[prefix]?.isMap == true) return true
        }
        if (bracket > 0 && byKey[canonicalKey(key.substring(0, bracket))]?.isMap == true) return true
        return false
    }

    companion object {
        val EMPTY = SpringMetadata(emptyMap(), emptyMap())

        /** Jars are merged in order, the first definition of a key wins; unreadable jars are skipped. */
        fun fromJars(jars: List<File>): SpringMetadata {
            val props = LinkedHashMap<String, ConfigProperty>()
            val hints = LinkedHashMap<String, ConfigHint>()
            for (jar in jars) {
                val (jarProps, jarHints) = readJar(jar)
                for (p in jarProps) props.putIfAbsent(canonicalKey(p.name), p)
                for (h in jarHints) hints.putIfAbsent(canonicalKey(h.name), h)
            }
            return SpringMetadata(props, hints)
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
                // The additional file is read last so a hand-written entry overrides a generated one.
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
                    deprecation = o.get("deprecation")?.let(::deprecation),
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

        private fun render(el: JsonElement): String = if (el.isJsonPrimitive) el.asString else el.toString()

        private fun JsonElement.objectOrNull(): JsonObject? = if (isJsonObject) asJsonObject else null

        private fun JsonObject.array(name: String): List<JsonElement> =
            get(name)?.takeIf { it.isJsonArray }?.asJsonArray?.toList().orEmpty()

        private fun JsonObject.text(name: String): String? =
            get(name)?.takeIf { it.isJsonPrimitive }?.asString
    }
}
