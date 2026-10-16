package dev.brewery.daemon.project

import dev.brewery.daemon.BuildModule
import dev.brewery.daemon.BuildTask
import dev.brewery.daemon.BuildTool
import dev.brewery.daemon.ProjectModel
import org.gradle.tooling.GradleConnector
import org.gradle.tooling.model.GradleProject
import java.io.OutputStream
import java.nio.file.Files
import java.nio.file.Path

class GradleModelReader(
    private val fallbackGradleVersion: String = "9.8.1",
    private val jvm: GradleJvm = GradleJvm(),
) : ProjectModelReader {
    override fun read(root: Path): ProjectModel {
        val javaHome = jvm.javaHome(root, fallbackGradleVersion)
        jvm.check(root, fallbackGradleVersion, javaHome)
        val connector = GradleConnector.newConnector().forProjectDirectory(root.toFile())
        // A project without a wrapper still needs some distribution to answer the model query.
        if (!Files.exists(root.resolve("gradle/wrapper/gradle-wrapper.properties"))) {
            connector.useGradleVersion(fallbackGradleVersion)
        }
        val connection = connector.connect()
        try {
            // stdout carries the JSON-RPC stream, so Gradle's own output must never reach it.
            val project = connection.model(GradleProject::class.java)
                .setJavaHome(javaHome)
                .setStandardOutput(OutputStream.nullOutputStream())
                .setStandardError(OutputStream.nullOutputStream())
                .get()
            return ProjectModel(BuildTool.gradle, root.toString(), flatten(project))
        } finally {
            connection.close()
        }
    }

    private fun flatten(project: GradleProject): List<BuildModule> {
        val tasks = project.tasks.sortedBy { it.name }.map { BuildTask(it.name, it.group, it.description) }
        return listOf(BuildModule(project.name, project.path, tasks)) + project.children.flatMap { flatten(it) }
    }
}
