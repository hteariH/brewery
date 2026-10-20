package dev.brewery.daemon.mcp

import dev.brewery.daemon.springconfig.PropertiesDocument
import dev.brewery.daemon.springconfig.YamlDocument
import java.nio.file.FileVisitResult
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.SimpleFileVisitor
import java.nio.file.attribute.BasicFileAttributes

/** [source] is the config file the values came from, so errors can point at it. */
data class Datasource(val url: String, val username: String?, val password: String?, val source: Path)

/**
 * Finds the project's `spring.datasource.*` in `src/main/resources/application.{yml,yaml,properties}`.
 * Only the default profile counts: the first YAML document and the plain properties file. The root
 * module's file wins, then modules in path order; within a module `.properties` wins over `.yml`/`.yaml`.
 */
object SpringDatasource {
    private val SKIP = setOf("build", "target", "out", "node_modules", ".gradle", ".git", ".idea")
    // Spring Boot precedence within one location: .properties over .yml over .yaml.
    private val NAMES = listOf("application.properties", "application.yml", "application.yaml")
    private val PLACEHOLDER = Regex("""\$\{([^}:]+)(?::([^}]*))?}""")
    private const val MAX_DEPTH = 6

    fun find(root: Path, env: (String) -> String? = System::getenv): Datasource? {
        for (file in configFiles(root)) {
            val values = read(file)
            val url = values["spring.datasource.url"] ?: continue
            return Datasource(
                resolve(url, env, file),
                values["spring.datasource.username"]?.let { resolve(it, env, file) },
                values["spring.datasource.password"]?.let { resolve(it, env, file) },
                file,
            )
        }
        return null
    }

    private fun configFiles(root: Path): List<Path> {
        val found = ArrayList<Path>()
        Files.walkFileTree(root, emptySet(), MAX_DEPTH + 4, object : SimpleFileVisitor<Path>() {
            override fun preVisitDirectory(dir: Path, attrs: BasicFileAttributes): FileVisitResult =
                if (dir != root && dir.fileName.toString() in SKIP) FileVisitResult.SKIP_SUBTREE else FileVisitResult.CONTINUE

            override fun visitFile(file: Path, attrs: BasicFileAttributes): FileVisitResult {
                val parent = file.parent
                if (file.fileName.toString() in NAMES && parent.endsWith(Path.of("src", "main", "resources"))) found.add(file)
                return FileVisitResult.CONTINUE
            }
        })
        // Root module first (fewest path segments), then path order; properties before yml within a module.
        return found.sortedWith(compareBy<Path>({ root.relativize(it).nameCount }, { it.parent.toString() }, { NAMES.indexOf(it.fileName.toString()) }))
    }

    private fun read(file: Path): Map<String, String> {
        val text = Files.readString(file)
        return if (file.fileName.toString().endsWith(".properties")) {
            PropertiesDocument(text).entries.associate { it.key to it.value.trim() }
        } else {
            // The default profile is the first document with content; a leading `---` opens an empty one.
            val entries = YamlDocument(text).entries
            val first = entries.minOfOrNull { it.document }
            entries.filter { it.document == first && !it.hasChildren }.associate { it.key to cleanYaml(it.value) }
        }
    }

    private fun cleanYaml(raw: String): String {
        var value = raw.trim()
        if (value.startsWith("\"") || value.startsWith("'")) {
            val end = value.indexOf(value[0], 1)
            if (end > 0) return value.substring(1, end)
        }
        val comment = value.indexOf(" #")
        if (comment >= 0) value = value.substring(0, comment).trimEnd()
        return value
    }

    private fun resolve(value: String, env: (String) -> String?, file: Path): String = PLACEHOLDER.replace(value) { m ->
        val name = m.groupValues[1]
        env(name) ?: m.groups[2]?.value
            ?: throw McpToolError("$file: \${$name} is not set in the environment and has no default")
    }
}
