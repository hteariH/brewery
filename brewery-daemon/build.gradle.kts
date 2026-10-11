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
    implementation("org.eclipse.lsp4j:org.eclipse.lsp4j.jsonrpc:1.0.0")
    implementation("org.gradle:gradle-tooling-api:9.8.1")
    implementation("org.apache.maven:maven-model:3.9.16")
    runtimeOnly("org.slf4j:slf4j-nop:2.0.17")
    testImplementation(kotlin("test"))
    testImplementation(platform("org.junit:junit-bom:6.1.3"))
    testImplementation("org.junit.jupiter:junit-jupiter")
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
// that Apache-licensed dependencies require us to ship. This collects every dependency's own license and
// notice files into one file inside the jar instead.
val thirdPartyNotices = tasks.register("thirdPartyNotices") {
    val jars = configurations.runtimeClasspath
    val output = layout.buildDirectory.file("notices/THIRD-PARTY-NOTICES.txt")
    inputs.files(jars)
    outputs.file(output)
    doLast {
        val noticeName = Regex("""META-INF/(LICENSE|NOTICE|DEPENDENCIES)[^/]*""", RegexOption.IGNORE_CASE)
        val text = StringBuilder("Third-party notices for the dependencies bundled in brewery-daemon-all.jar\n")
        for (jar in jars.get().files.sortedBy { it.name }.filter { it.extension == "jar" }) {
            ZipFile(jar).use { zip ->
                val entries = zip.entries().asSequence().filter { !it.isDirectory && noticeName.matches(it.name) }.toList()
                if (entries.isEmpty()) return@use
                text.append("\n==== ").append(jar.name).append(" ====\n")
                for (entry in entries.sortedBy { it.name }) {
                    text.append("\n--- ").append(entry.name).append(" ---\n")
                    text.append(zip.getInputStream(entry).readBytes().toString(Charsets.UTF_8).replace("\r\n", "\n")).append("\n")
                }
            }
        }
        output.get().asFile.apply { parentFile.mkdirs() }.writeText(text.toString())
    }
}

tasks.shadowJar {
    from(thirdPartyNotices)
    archiveBaseName.set("brewery-daemon")
    archiveClassifier.set("all")
    archiveVersion.set("")
    mergeServiceFiles()
}
