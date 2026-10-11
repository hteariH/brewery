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

tasks.test { useJUnitPlatform() }

tasks.shadowJar {
    archiveBaseName.set("brewery-daemon")
    archiveClassifier.set("all")
    archiveVersion.set("")
    mergeServiceFiles()
}
