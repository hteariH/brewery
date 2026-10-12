import java.util.zip.ZipFile

plugins {
    kotlin("jvm") version "2.4.21"
    id("com.gradleup.shadow") version "9.6.1"
    application
}

group = "dev.brewery"
version = "0.1.0"

repositories {
    mavenCentral()
    maven("https://repo.gradle.org/gradle/libs-releases")
}

dependencies {
    implementation("org.eclipse.lsp4j:org.eclipse.lsp4j:1.0.0")
    implementation("org.gradle:gradle-tooling-api:9.8.1")
    implementation("org.apache.maven:maven-model:3.9.16")
    implementation("com.github.javaparser:javaparser-core:3.28.2")
    // Database drivers (bundled) and the OS secret store. MariaDB Connector/J is LGPL-2.1 and stays an
    // unmodified, separate jar inside the fat jar; MySQL Connector/J is GPL and is deliberately not used.
    implementation("org.postgresql:postgresql:42.7.14")
    implementation("org.mariadb.jdbc:mariadb-java-client:3.5.10")
    implementation("org.xerial:sqlite-jdbc:3.53.4.0")
    implementation("com.github.javakeyring:java-keyring:1.0.4")
    runtimeOnly("org.slf4j:slf4j-nop:2.0.17")
    testImplementation(kotlin("test"))
    testImplementation(platform("org.junit:junit-bom:6.1.3"))
    testImplementation("org.junit.jupiter:junit-jupiter")
    // Skipped automatically when Docker is unavailable; CI ubuntu runners have it.
    testImplementation("org.testcontainers:testcontainers-postgresql:2.0.5")
    testImplementation("org.testcontainers:testcontainers-mariadb:2.0.5")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")
}

kotlin { jvmToolchain(21) }

application { mainClass.set("dev.brewery.daemon.MainKt") }

tasks.test {
    useJUnitPlatform()
    // NoticesTest inspects the packaged jar, so it has to exist before the tests run.
    dependsOn(tasks.shadowJar)
}

// Shadow keeps only the first META-INF/LICENSE it meets and drops the rest, which would strip the notices
// that Apache-licensed dependencies require us to ship. This writes one section per runtime jar instead:
// name, version, the licence declared in its POM (following <parent> POMs) and any licence/notice text
// found inside the jar. It also lists the jars in build/notices/runtime-jars.txt so a test can check coverage.
val thirdPartyNotices = tasks.register("thirdPartyNotices") {
    val runtime = configurations.runtimeClasspath
    val output = layout.buildDirectory.file("notices/THIRD-PARTY-NOTICES.txt")
    val jarList = layout.buildDirectory.file("notices/runtime-jars.txt")
    inputs.files(runtime)
    outputs.files(output, jarList)
    doLast {
        val fileName = Regex(
            """(META-INF/(LICENSE|NOTICE|DEPENDENCIES)[^/]*|(LICENSE|NOTICE)[^/]*|about\.html)""",
            RegexOption.IGNORE_CASE,
        )
        val pomCache = HashMap<String, org.w3c.dom.Element?>()
        fun pomOf(group: String, name: String, version: String): org.w3c.dom.Element? =
            pomCache.getOrPut("$group:$name:$version") {
                runCatching {
                    val cfg = project.configurations.detachedConfiguration(
                        project.dependencies.create("$group:$name:$version@pom"),
                    ).apply { isTransitive = false }
                    val factory = javax.xml.parsers.DocumentBuilderFactory.newInstance()
                    factory.isNamespaceAware = false
                    factory.newDocumentBuilder().parse(cfg.singleFile).documentElement
                }.getOrNull()
            }
        fun org.w3c.dom.Element.child(tag: String): org.w3c.dom.Element? =
            (0 until childNodes.length).map { childNodes.item(it) }
                .filterIsInstance<org.w3c.dom.Element>().firstOrNull { it.tagName == tag }
        fun org.w3c.dom.Element.childText(tag: String): String? = child(tag)?.textContent?.trim()
        fun licensesOf(group: String, name: String, version: String, depth: Int = 0): List<Pair<String, String>> {
            val pom = pomOf(group, name, version) ?: return emptyList()
            val found = pom.child("licenses")?.let { l ->
                (0 until l.childNodes.length).map { l.childNodes.item(it) }.filterIsInstance<org.w3c.dom.Element>()
                    .filter { it.tagName == "license" }
                    .map { (it.childText("name") ?: "unknown") to (it.childText("url") ?: "") }
            }.orEmpty()
            if (found.isNotEmpty() || depth > 5) return found
            val parent = pom.child("parent") ?: return emptyList()
            return licensesOf(
                parent.childText("groupId") ?: return emptyList(),
                parent.childText("artifactId") ?: return emptyList(),
                parent.childText("version") ?: return emptyList(),
                depth + 1,
            )
        }

        val artifacts = runtime.get().resolvedConfiguration.resolvedArtifacts
            .filter { it.file.extension == "jar" }
            .sortedBy { it.file.name }
        val text = StringBuilder("Third-party notices for the dependencies bundled in brewery-daemon-all.jar\n")
        for (artifact in artifacts) {
            val id = artifact.moduleVersion.id
            text.append("\n==== ").append(artifact.file.name).append(" (").append(id.group).append(':')
                .append(id.name).append(':').append(id.version).append(") ====\n")
            val licenses = licensesOf(id.group, id.name, id.version)
            if (licenses.isEmpty()) text.append("License: not declared in POM\n")
            for ((n, u) in licenses) text.append("License: ").append(n).append(if (u.isEmpty()) "" else " <$u>").append("\n")
            ZipFile(artifact.file).use { zip ->
                val entries = zip.entries().asSequence().filter { !it.isDirectory && fileName.matches(it.name) }.toList()
                for (entry in entries.sortedBy { it.name }) {
                    text.append("\n--- ").append(entry.name).append(" ---\n")
                    text.append(zip.getInputStream(entry).readBytes().toString(Charsets.UTF_8).replace("\r\n", "\n")).append("\n")
                }
            }
        }
        output.get().asFile.apply { parentFile.mkdirs() }.writeText(text.toString())
        jarList.get().asFile.writeText(artifacts.joinToString("\n", postfix = "\n") { it.file.name })
    }
}

tasks.shadowJar {
    from(thirdPartyNotices) { exclude("runtime-jars.txt") }
    archiveBaseName.set("brewery-daemon")
    archiveClassifier.set("all")
    archiveVersion.set("")
    // INCLUDE lets the service-file merger see every copy (java.sql.Driver lists one driver per jar).
    duplicatesStrategy = DuplicatesStrategy.INCLUDE
    mergeServiceFiles()
}
