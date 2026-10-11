package dev.brewery.daemon.spring

import com.github.javaparser.JavaParser
import com.github.javaparser.ParserConfiguration
import com.github.javaparser.ast.Node
import com.github.javaparser.ast.body.ClassOrInterfaceDeclaration
import com.github.javaparser.ast.body.MethodDeclaration
import com.github.javaparser.ast.expr.AnnotationExpr
import com.github.javaparser.ast.expr.ArrayInitializerExpr
import com.github.javaparser.ast.expr.Expression
import com.github.javaparser.ast.expr.NormalAnnotationExpr
import com.github.javaparser.ast.expr.SingleMemberAnnotationExpr
import com.github.javaparser.ast.expr.StringLiteralExpr
import com.github.javaparser.ast.nodeTypes.NodeWithAnnotations
import dev.brewery.daemon.BuildTool
import dev.brewery.daemon.ProjectModel
import java.beans.Introspector
import java.io.IOException
import java.nio.file.FileVisitResult
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.SimpleFileVisitor
import java.nio.file.attribute.BasicFileAttributes

/**
 * Finds Spring Boot applications, beans and HTTP endpoints by parsing `src/main/java` of every module.
 * Syntax only, no symbol resolution: annotations are matched by simple name, so it works without a
 * classpath and stays fast, at the price of missing meta-annotations and constants in mapping paths.
 */
class SpringScanner {
    // JavaParser keeps per-instance state, so each scan builds its own.
    private fun newParser() = JavaParser(
        ParserConfiguration()
            .setLanguageLevel(ParserConfiguration.LanguageLevel.JAVA_21)
            .setAttributeComments(false),
    )

    /** Parsed files so far; tests read it to prove unchanged files are not parsed again. */
    @Volatile
    var parsedFiles = 0
        private set

    private class Stamp(val modifiedMillis: Long, val size: Long, val modulePath: String)
    private class FileResult(val stamp: Stamp, val apps: List<SpringApp>, val beans: List<SpringBean>, val endpoints: List<SpringEndpoint>)

    // Keyed by absolute path. Rebuilt on every scan, so files that disappeared drop out.
    private var cache = mapOf<String, FileResult>()

    @Synchronized
    fun scan(model: ProjectModel): SpringModel {
        val root = Path.of(model.rootDir)
        val parser = newParser()
        val apps = mutableListOf<SpringApp>()
        val beans = mutableListOf<SpringBean>()
        val endpoints = mutableListOf<SpringEndpoint>()
        val next = HashMap<String, FileResult>()
        for (module in model.modules) {
            for (file in javaFiles(sourceRoot(root, model.buildTool, module.path))) {
                val path = file.toAbsolutePath().normalize().toString()
                val stamp = try {
                    val size = Files.size(file)
                    // Generated or vendored giants are not worth the parse time; real Spring classes are tiny.
                    if (size > MAX_FILE_BYTES) continue
                    Stamp(Files.getLastModifiedTime(file).toMillis(), size, module.path)
                } catch (e: IOException) {
                    continue
                }
                val known = cache[path]
                val result = if (known != null && known.stamp.same(stamp)) {
                    known
                } else {
                    parseFile(parser, file, path, stamp) ?: continue
                }
                next[path] = result
                apps += result.apps
                beans += result.beans
                endpoints += result.endpoints
            }
        }
        cache = next
        return SpringModel(apps, beans, endpoints)
    }

    private fun Stamp.same(other: Stamp) =
        modifiedMillis == other.modifiedMillis && size == other.size && modulePath == other.modulePath

    private fun parseFile(parser: JavaParser, file: Path, path: String, stamp: Stamp): FileResult? {
        parsedFiles++
        val unit = try {
            val result = parser.parse(file)
            // A partial tree from a file with syntax errors would yield wrong beans, so skip it whole.
            if (!result.isSuccessful) return null
            result.result.orElse(null) ?: return null
        } catch (e: IOException) {
            return null
        } catch (e: RuntimeException) {
            return null
        }
        val apps = mutableListOf<SpringApp>()
        val beans = mutableListOf<SpringBean>()
        val endpoints = mutableListOf<SpringEndpoint>()
        for (type in unit.findAll(ClassOrInterfaceDeclaration::class.java)) {
            scanType(type, stamp.modulePath, path, apps, beans, endpoints)
        }
        return FileResult(stamp, apps, beans, endpoints)
    }

    private fun scanType(
        type: ClassOrInterfaceDeclaration,
        modulePath: String,
        file: String,
        apps: MutableList<SpringApp>,
        beans: MutableList<SpringBean>,
        endpoints: MutableList<SpringEndpoint>,
    ) {
        val fqcn = type.fullyQualifiedName.orElse(type.nameAsString)
        val simple = type.nameAsString
        val line = type.line()
        val isApp = type.annotation("SpringBootApplication") != null
        if (isApp) apps += SpringApp(simple, fqcn, modulePath, file, line)

        // The application class is a configuration too, but listing it among beans would just duplicate the app row.
        val stereotype = if (isApp) null else STEREOTYPES.firstNotNullOfOrNull { (name, kind) ->
            type.annotation(name)?.let { it to kind }
        }
        if (stereotype != null) {
            val (annotation, kind) = stereotype
            // Same default as Spring's bean name generator (Introspector rules, so `URLService` stays as is).
            val name = annotation.stringAttribute("value") ?: Introspector.decapitalize(simple)
            beans += SpringBean(name, fqcn, kind, file, line)
        }

        for (method in type.methods) {
            val bean = method.annotation("Bean") ?: continue
            val name = bean.stringAttribute("name", "value") ?: method.nameAsString
            beans += SpringBean(name, method.type.asString(), SpringBeanKind.bean, file, method.line())
        }

        val classMapping = type.annotation("RequestMapping")
        if (classMapping != null || type.annotation("RestController") != null || type.annotation("Controller") != null) {
            val prefix = classMapping?.stringAttribute("value", "path").orEmpty()
            for (method in type.methods) {
                val (http, path) = endpointOf(method, prefix) ?: continue
                endpoints += SpringEndpoint(http, path, "$simple#${method.nameAsString}", file, method.line())
            }
        }
    }

    private fun endpointOf(method: MethodDeclaration, prefix: String): Pair<HttpMethod, String>? {
        for ((name, http) in METHOD_MAPPINGS) {
            val annotation = method.annotation(name) ?: continue
            return http to joinPath(prefix, annotation.stringAttribute("value", "path").orEmpty())
        }
        val request = method.annotation("RequestMapping") ?: return null
        val http = request.attribute("method")?.let(::firstEnumName)
            ?.let { runCatching { HttpMethod.valueOf(it) }.getOrNull() } ?: HttpMethod.ANY
        return http to joinPath(prefix, request.stringAttribute("value", "path").orEmpty())
    }

    private fun sourceRoot(root: Path, tool: BuildTool, modulePath: String): Path {
        val dir = when (tool) {
            // ":" is the root project, ":a:b" lives in a/b under the default layout.
            BuildTool.gradle -> modulePath.split(':').filter { it.isNotEmpty() }.fold(root) { acc, part -> acc.resolve(part) }
            BuildTool.maven -> if (modulePath == ".") root else root.resolve(modulePath)
            BuildTool.none -> root
        }
        return dir.resolve("src").resolve("main").resolve("java")
    }

    private fun javaFiles(src: Path): List<Path> {
        if (!Files.isDirectory(src)) return emptyList()
        val found = mutableListOf<Path>()
        Files.walkFileTree(
            src,
            object : SimpleFileVisitor<Path>() {
                override fun preVisitDirectory(dir: Path, attrs: BasicFileAttributes): FileVisitResult =
                    if (dir != src && dir.fileName.toString() in SKIPPED_DIRS) {
                        FileVisitResult.SKIP_SUBTREE
                    } else {
                        FileVisitResult.CONTINUE
                    }

                override fun visitFile(file: Path, attrs: BasicFileAttributes): FileVisitResult {
                    if (file.fileName.toString().endsWith(".java")) found.add(file)
                    return FileVisitResult.CONTINUE
                }

                override fun visitFileFailed(file: Path, exc: IOException) = FileVisitResult.CONTINUE
            },
        )
        return found.sorted()
    }

    private companion object {
        const val MAX_FILE_BYTES = 1L shl 20
        val SKIPPED_DIRS = setOf("build", "target", ".gradle")

        // Order matters: a class carrying several stereotypes is reported as the most specific one.
        val STEREOTYPES = listOf(
            "RestController" to SpringBeanKind.restController,
            "Controller" to SpringBeanKind.controller,
            "Service" to SpringBeanKind.service,
            "Repository" to SpringBeanKind.repository,
            "Configuration" to SpringBeanKind.configuration,
            "Component" to SpringBeanKind.component,
        )

        val METHOD_MAPPINGS = listOf(
            "GetMapping" to HttpMethod.GET,
            "PostMapping" to HttpMethod.POST,
            "PutMapping" to HttpMethod.PUT,
            "DeleteMapping" to HttpMethod.DELETE,
            "PatchMapping" to HttpMethod.PATCH,
        )
    }
}

private fun Node.line(): Int = range.map { it.begin.line }.orElse(1)

// Simple-name match also accepts the fully-qualified spelling (`@org.springframework...Service`).
private fun NodeWithAnnotations<*>.annotation(simpleName: String): AnnotationExpr? =
    annotations.firstOrNull { it.nameAsString.substringAfterLast('.') == simpleName }

private fun AnnotationExpr.attribute(vararg names: String): Expression? = when (this) {
    is SingleMemberAnnotationExpr -> if ("value" in names) memberValue else null
    is NormalAnnotationExpr -> pairs.firstOrNull { it.nameAsString in names }?.value
    else -> null
}

private fun AnnotationExpr.stringAttribute(vararg names: String): String? = attribute(*names)?.let(::firstString)

private fun firstString(expr: Expression): String? = when (expr) {
    is StringLiteralExpr -> expr.value
    is ArrayInitializerExpr -> expr.values.firstOrNull()?.let(::firstString)
    else -> null
}

// `RequestMethod.GET`, a static import `GET`, or `{GET, POST}`: the first one wins.
private fun firstEnumName(expr: Expression): String? = when (expr) {
    is ArrayInitializerExpr -> expr.values.firstOrNull()?.let(::firstEnumName)
    else -> expr.toString().substringAfterLast('.')
}

internal fun joinPath(prefix: String, suffix: String): String {
    val joined = "/$prefix/$suffix".replace(Regex("/+"), "/")
    return if (joined.length > 1) joined.trimEnd('/') else joined
}
