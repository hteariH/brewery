package dev.brewery.daemon.project

import dev.brewery.daemon.BuildModule
import dev.brewery.daemon.BuildTask
import dev.brewery.daemon.BuildTool
import dev.brewery.daemon.ProjectModel
import org.apache.maven.model.io.xpp3.MavenXpp3Reader
import java.nio.file.Files
import java.nio.file.Path

class MavenModelReader : ProjectModelReader {
    // Plugin goals need a real Maven run, so only the lifecycle phases are offered for now.
    private val lifecycle = listOf("clean", "validate", "compile", "test", "package", "verify", "install", "deploy")
        .map { BuildTask(it, "lifecycle", null) }

    override fun read(root: Path): ProjectModel =
        ProjectModel(BuildTool.maven, root.toString(), collect(root.toAbsolutePath().normalize(), root, mutableSetOf()))

    private fun collect(root: Path, dir: Path, seen: MutableSet<Path>): List<BuildModule> {
        val normalized = dir.toAbsolutePath().normalize()
        if (!seen.add(normalized)) return emptyList()
        val model = Files.newBufferedReader(normalized.resolve(MAVEN_BUILD_FILE)).use { MavenXpp3Reader().read(it) }
        val relative = root.relativize(normalized).toString().replace("\\", "/").ifEmpty { "." }
        val self = BuildModule(model.artifactId ?: normalized.fileName.toString(), relative, lifecycle)
        return listOf(self) + model.modules.flatMap { collect(root, normalized.resolve(it), seen) }
    }
}
