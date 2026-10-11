# Brewery M2b — Daemon and Build Model Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A `brewery-daemon` (Kotlin/JVM) runs next to each project window, speaks Brewery's JSON-RPC protocol over stdio, and reports the project's Gradle or Maven model (modules and tasks); a "Gradle/Maven" tool window on the right stripe shows that model and runs a task with a double click.

**Architecture:** Spec sections 5–8. The daemon is a Gradle Kotlin project in `brewery-daemon/` (own Apache 2.0 licence). Rust side: crate `brewery_daemon_client` finds a JDK 21+, starts the daemon jar through Zed's `lsp::LanguageServer` process/JSON-RPC machinery (Content-Length framing, request/notification handling) and offers typed requests; crate `brewery_build` is the tool window. Protocol types are hand-written on both sides and pinned by shared JSON fixtures in `brewery-daemon/protocol/fixtures/` that tests on both sides read.

**Rulings (spec deviations, recorded):**
1. Monorepo: `brewery-daemon/` lives in this repository with its own `LICENSE` (Apache 2.0) instead of a separate `brewery-daemon` repository — creating another public repository is an outward action not yet authorised; `git subtree split` can extract it later.
2. Shared JSON fixtures instead of JSON-Schema code generation — same guarantee (both sides agree on the wire format) with far less machinery; revisit if the protocol grows past ~20 methods.

**Tech Stack:** Kotlin 2.4.21, JVM 21 toolchain, Gradle 9.8.1 (wrapper), `org.eclipse.lsp4j:org.eclipse.lsp4j.jsonrpc:1.0.0`, `org.gradle:gradle-tooling-api:9.8.1` (repository `https://repo.gradle.org/gradle/libs-releases`), `org.apache.maven:maven-model:3.9.16`, JUnit 6.1.3, `com.gradleup.shadow` 9.6.1; Rust: Zed `lsp`, `gpui`, `workspace`, `task`, `ui`.

**Spec:** `docs/superpowers/specs/2026-10-10-brewery-design.md`

## Global Constraints

- Daemon code: Apache 2.0, `brewery-daemon/LICENSE` with the full Apache 2.0 text; package `dev.brewery.daemon`.
- Protocol: JSON-RPC 2.0 over stdio, `Content-Length` framing. Handshake `initialize { protocolVersion }` → `{ protocolVersion, daemonVersion }`; `PROTOCOL_VERSION = 1`. On mismatch the UI logs an error and does not use the daemon.
- Methods in this milestone: `initialize`, `shutdown`, `exit` (notification), `project/open { rootDir }`, `project/model { }` → `ProjectModel`, notification `project/changed { }` (daemon → UI, after build files change).
- `ProjectModel` JSON: `{ "buildTool": "gradle" | "maven" | "none", "rootDir": "<abs path>", "modules": [ { "name": "...", "path": ":sub" | "sub", "tasks": [ { "name": "...", "group": "...", "description": "..." } ] } ] }` (`group`/`description` may be null).
- Domain errors: JSON-RPC error `code` -32001 `BUILD_IMPORT_FAILED`, -32002 `NOT_OPENED`, with `message` for users.
- JDK for the daemon: 21 or newer, searched in order `BREWERY_DAEMON_JAVA` env, `JAVA_HOME`, `java` on PATH, `~/.jdks/*`, Windows `C:\Program Files\Eclipse Adoptium\*`, `C:\Program Files\Java\*`, Linux `/usr/lib/jvm/*`.
- Daemon jar: env `BREWERY_DAEMON_JAR`, else `<exe dir>/brewery-daemon/brewery-daemon.jar`, else (dev) `<repo>/brewery-daemon/build/libs/brewery-daemon-all.jar` found by walking up from the exe dir.
- Rust edits in Zed files carry `brewery:` markers; LF only everywhere (Kotlin and Gradle files too).

## File map

| Path | Responsibility |
|---|---|
| `brewery-daemon/settings.gradle.kts`, `build.gradle.kts`, `gradle/wrapper/*`, `gradlew`, `gradlew.bat`, `LICENSE`, `README.md` | daemon build |
| `brewery-daemon/src/main/kotlin/dev/brewery/daemon/Main.kt` | stdio entry point |
| `.../Protocol.kt` | protocol data classes + `PROTOCOL_VERSION` |
| `.../BreweryDaemon.kt` | JSON-RPC service (`@JsonRequest` methods) |
| `.../project/ProjectModelReader.kt`, `GradleModelReader.kt`, `MavenModelReader.kt` | build model |
| `brewery-daemon/protocol/fixtures/*.json` | wire-format fixtures shared with Rust |
| `brewery-daemon/src/test/...` | JUnit tests, fixture projects under `src/test/resources/projects/` |
| `crates/brewery_daemon_client/` | JDK/jar discovery, process start, typed requests |
| `crates/brewery_build/` | Gradle/Maven tool window panel |
| `crates/zed/src/zed.rs`, `Cargo.toml`s | register panel (hook) |
| `.github/workflows/brewery_ci.yml`, `docs/brewery/FORK.md` | CI job for the daemon, test lists |

---

### Task 1: Daemon project skeleton with `initialize`/`shutdown`

**Files:** everything under `brewery-daemon/` except the project readers.

- [ ] **Step 1: Gradle wrapper without a system Gradle** — download `https://services.gradle.org/distributions/gradle-9.8.1-bin.zip` into a scratch dir, unzip, and run `<scratch>/gradle-9.8.1/bin/gradle wrapper --gradle-version 9.8.1 --distribution-type bin` inside `brewery-daemon/` (it needs a `settings.gradle.kts` first). Use JDK 21 (`~/.jdks/temurin-21.0.11`) as `JAVA_HOME`. Commit the wrapper files (`gradlew` with mode 100755 via `git update-index --chmod=+x`).

- [ ] **Step 2: Build files**

`brewery-daemon/settings.gradle.kts`:
```kotlin
rootProject.name = "brewery-daemon"
```

`brewery-daemon/build.gradle.kts`:
```kotlin
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
```
(If `slf4j-nop` 2.0.17 does not resolve, use the newest 2.0.x; it silences the Tooling API's slf4j warning on stderr.)

- [ ] **Step 3: Failing test** — `brewery-daemon/src/test/kotlin/dev/brewery/daemon/HandshakeTest.kt`:
```kotlin
package dev.brewery.daemon

import org.eclipse.lsp4j.jsonrpc.Launcher
import org.junit.jupiter.api.Test
import java.io.PipedInputStream
import java.io.PipedOutputStream
import java.util.concurrent.TimeUnit
import kotlin.test.assertEquals

class HandshakeTest {
    @Test
    fun `initialize returns protocol and daemon version`() {
        val clientToServer = PipedOutputStream()
        val serverIn = PipedInputStream(clientToServer)
        val serverToClient = PipedOutputStream()
        val clientIn = PipedInputStream(serverToClient)

        val daemon = BreweryDaemon()
        val serverLauncher = Launcher.createLauncher(daemon, BreweryClient::class.java, serverIn, serverToClient)
        daemon.connect(serverLauncher.remoteProxy)
        serverLauncher.startListening()

        val clientLauncher = Launcher.createLauncher(object : BreweryClient {
            override fun projectChanged(params: ProjectChanged) {}
        }, BreweryServer::class.java, clientIn, clientToServer)
        clientLauncher.startListening()

        val result = clientLauncher.remoteProxy.initialize(InitializeParams(PROTOCOL_VERSION)).get(10, TimeUnit.SECONDS)
        assertEquals(PROTOCOL_VERSION, result.protocolVersion)
        assertEquals("0.1.0", result.daemonVersion)
    }
}
```
Add `testImplementation(kotlin("test"))` to dependencies for `kotlin.test`. Run `./gradlew test` (JAVA_HOME = JDK 21) → FAIL to compile.

- [ ] **Step 4: Implement**

`Protocol.kt`:
```kotlin
package dev.brewery.daemon

import org.eclipse.lsp4j.jsonrpc.services.JsonNotification
import org.eclipse.lsp4j.jsonrpc.services.JsonRequest
import java.util.concurrent.CompletableFuture

const val PROTOCOL_VERSION = 1
const val DAEMON_VERSION = "0.1.0"

data class InitializeParams(val protocolVersion: Int)
data class InitializeResult(val protocolVersion: Int, val daemonVersion: String)
data class OpenProjectParams(val rootDir: String)
class ProjectModelParams
class ProjectChanged

enum class BuildTool { gradle, maven, none }
data class BuildTask(val name: String, val group: String?, val description: String?)
data class BuildModule(val name: String, val path: String, val tasks: List<BuildTask>)
data class ProjectModel(val buildTool: BuildTool, val rootDir: String, val modules: List<BuildModule>)

object ErrorCodes {
    const val BUILD_IMPORT_FAILED = -32001
    const val NOT_OPENED = -32002
}

interface BreweryServer {
    @JsonRequest("initialize") fun initialize(params: InitializeParams): CompletableFuture<InitializeResult>
    @JsonRequest("shutdown") fun shutdown(): CompletableFuture<Any?>
    @JsonNotification("exit") fun exit()
    @JsonRequest("project/open") fun openProject(params: OpenProjectParams): CompletableFuture<Any?>
    @JsonRequest("project/model") fun projectModel(params: ProjectModelParams): CompletableFuture<ProjectModel>
}

interface BreweryClient {
    @JsonNotification("project/changed") fun projectChanged(params: ProjectChanged)
}
```

`BreweryDaemon.kt`:
```kotlin
package dev.brewery.daemon

import java.util.concurrent.CompletableFuture
import kotlin.system.exitProcess

class BreweryDaemon : BreweryServer {
    private var client: BreweryClient? = null

    fun connect(client: BreweryClient) {
        this.client = client
    }

    override fun initialize(params: InitializeParams): CompletableFuture<InitializeResult> =
        CompletableFuture.completedFuture(InitializeResult(PROTOCOL_VERSION, DAEMON_VERSION))

    override fun shutdown(): CompletableFuture<Any?> = CompletableFuture.completedFuture(null)

    override fun exit() {
        exitProcess(0)
    }

    override fun openProject(params: OpenProjectParams): CompletableFuture<Any?> =
        CompletableFuture.failedFuture(UnsupportedOperationException("implemented in Task 2"))

    override fun projectModel(params: ProjectModelParams): CompletableFuture<ProjectModel> =
        CompletableFuture.failedFuture(UnsupportedOperationException("implemented in Task 2"))
}
```
(The two `failedFuture` placeholders are replaced in Task 2 of this same plan; they must not survive the milestone.)

`Main.kt`:
```kotlin
package dev.brewery.daemon

import org.eclipse.lsp4j.jsonrpc.Launcher

fun main() {
    val daemon = BreweryDaemon()
    val launcher = Launcher.createLauncher(daemon, BreweryClient::class.java, System.`in`, System.out)
    daemon.connect(launcher.remoteProxy)
    launcher.startListening().get()
}
```

`brewery-daemon/LICENSE`: full Apache License 2.0 text (copy from the repo root `LICENSE-APACHE`). `brewery-daemon/README.md`: what it is, how to build (`./gradlew shadowJar` → `build/libs/brewery-daemon-all.jar`), protocol pointer to the spec, licence.

- [ ] **Step 5: Fixtures** — `brewery-daemon/protocol/fixtures/initialize.params.json` = `{"protocolVersion":1}`, `initialize.result.json` = `{"protocolVersion":1,"daemonVersion":"0.1.0"}`. Add a JUnit test `FixturesTest` that deserialises both with lsp4j's Gson (`org.eclipse.lsp4j.jsonrpc.json.MessageJsonHandler(emptyMap()).gson`) into `InitializeParams` / `InitializeResult` and asserts the values (the fixtures dir is read via a path relative to the project dir, `File("protocol/fixtures/...")`).

- [ ] **Step 6: Run** `./gradlew test shadowJar` → PASS; `build/libs/brewery-daemon-all.jar` exists. Smoke: `printf 'Content-Length: 71\r\n\r\n{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}' | java -jar build/libs/brewery-daemon-all.jar` prints a response containing `"daemonVersion":"0.1.0"` (compute the real Content-Length of the body you send).

- [ ] **Step 7: Commit** `feat(daemon): brewery-daemon skeleton with JSON-RPC handshake`

---

### Task 2: Project model — Gradle via Tooling API, Maven via pom.xml

**Files:** `brewery-daemon/src/main/kotlin/dev/brewery/daemon/project/{ProjectModelReader,GradleModelReader,MavenModelReader}.kt`, `BreweryDaemon.kt`, tests + fixture projects `src/test/resources/projects/{gradle-multi,maven-multi,plain}`, fixture `protocol/fixtures/project-model.gradle.json`.

**Interfaces:**
- `fun interface ProjectModelReader { fun read(root: java.nio.file.Path): ProjectModel }`
- `fun detectBuildTool(root: Path): BuildTool` — `gradle` if `settings.gradle(.kts)` or `build.gradle(.kts)` exists, else `maven` if `pom.xml`, else `none`.
- `GradleModelReader` uses `GradleConnector.newConnector().forProjectDirectory(root.toFile())` (uses the project's wrapper if present) and `connection.getModel(GradleProject::class.java)`; modules = the root project and every child recursively (`path` = Gradle path like `:app`, root `:`), tasks = `project.tasks` sorted by name with `group`/`description`.
- `MavenModelReader` reads `pom.xml` with `org.apache.maven.model.io.xpp3.MavenXpp3Reader`, modules = root + `<modules>` entries (recursively, path = relative dir), tasks = the standard lifecycle phases `clean, validate, compile, test, package, verify, install, deploy` with group `lifecycle` and null description (plugin goals come later).
- `BreweryDaemon.openProject` stores the root and returns null; `projectModel` returns the model (cached until `openProject` is called again or build files change); errors → `ResponseErrorException(ResponseError(ErrorCodes.BUILD_IMPORT_FAILED, message, null))`; `projectModel` before `openProject` → `NOT_OPENED`. Reading runs off the JSON-RPC thread (`CompletableFuture.supplyAsync`).
- File watching: a `WatchService` on the root (and module dirs) for `build.gradle(.kts)`, `settings.gradle(.kts)`, `pom.xml` changes → drop the cache and send `project/changed`.

- [ ] **Step 1: Failing tests** — `ProjectModelTest`:
  - `detectBuildTool` for the three fixture projects.
  - Maven: `maven-multi` (root pom with `<modules><module>core</module><module>web</module></modules>`, child poms) → modules `[root artifactId, core, web]`, each with the 8 lifecycle tasks.
  - Gradle: `gradle-multi` (`settings.gradle.kts` with `include("app", "lib")`, empty `build.gradle.kts` files applying `java`) → module paths `[":", ":app", ":lib"]`, and `:app` has a task named `build` and one named `test`. This test downloads Gradle through the Tooling API on first run (the fixture has no wrapper; set `GradleConnector.useGradleVersion("9.8.1")` in the reader when no wrapper is present) — mark it `@Tag("slow")` but keep it in the default run.
  - `projectModel` before `openProject` fails with code -32002.
  - The Gradle model serialised with lsp4j Gson equals `protocol/fixtures/project-model.gradle.json` after replacing `rootDir` (write the fixture from the expected model: modules/tasks for `gradle-multi` minus volatile fields — if task lists vary across Gradle versions, the fixture holds a reduced example `{buildTool, rootDir, modules:[{name, path, tasks:[{name:"build",group:"build",description:"Assembles and tests this project."}]}]}` and the test asserts the *shape* by deserialising the fixture into `ProjectModel`).
- [ ] **Step 2:** `./gradlew test` → FAIL. **Step 3:** implement. **Step 4:** `./gradlew test shadowJar` → PASS.
- [ ] **Step 5: Commit** `feat(daemon): Gradle and Maven project model`

---

### Task 3: `brewery_daemon_client` crate

**Files:** `crates/brewery_daemon_client/{Cargo.toml,src/brewery_daemon_client.rs,src/jdk.rs,src/protocol.rs}`, `LICENSE-GPL` symlink, root `Cargo.toml` (member + workspace dep, marked).

**Interfaces:**
- `pub fn find_java(min_major: u32) -> anyhow::Result<PathBuf>` (search order from Global Constraints; reads `release` file `JAVA_VERSION="21.0.11"` in each JDK home, or `java -version` as fallback for PATH/JAVA_HOME) — split into a pure `fn pick_jdk(candidates: Vec<JdkCandidate>, min_major: u32) -> Option<JdkCandidate>` and `fn parse_release_version(contents: &str) -> Option<u32>` for tests.
- `pub fn find_daemon_jar() -> anyhow::Result<PathBuf>` (order from Global Constraints).
- `protocol.rs`: Rust structs mirroring `Protocol.kt` with `serde(rename_all = "camelCase")`; `lsp::request::Request` impls `Initialize` (`"initialize"`), `Shutdown`, `OpenProject` (`"project/open"`), `GetProjectModel` (`"project/model"`); notification `ProjectChanged` (`"project/changed"`); `pub const PROTOCOL_VERSION: u32 = 1;`.
- `pub struct DaemonClient { server: Arc<lsp::LanguageServer> }` with `pub async fn start(root: &Path, cx: &mut AsyncApp) -> anyhow::Result<Self>` (spawn `java -jar <jar>` via `lsp::LanguageServer::new` with a `LanguageServerBinary`, then send `Initialize`, check the version, then `OpenProject`), `pub async fn project_model(&self) -> anyhow::Result<ProjectModel>`, `pub fn on_project_changed(&self, f: impl Fn() + Send + 'static) -> lsp::Subscription`, `pub async fn shutdown(self)`.

- [ ] **Step 1: Failing tests** — `parse_release_version` (`JAVA_VERSION="21.0.11"` → 21, `"1.8.0_402"` → 8, garbage → None); `pick_jdk` picks the first candidate with major ≥ 21; protocol fixtures: deserialise `brewery-daemon/protocol/fixtures/initialize.result.json` and `project-model.gradle.json` into the Rust types (`include_str!("../../../brewery-daemon/protocol/fixtures/...")`) and serialise `InitializeParams { protocol_version: 1 }` equal to `initialize.params.json` (compare as `serde_json::Value`).
- [ ] **Step 2: Integration test** `#[gpui::test]` gated on env `BREWERY_DAEMON_JAR` being set (skip with an explicit early return and a printed note otherwise): start the daemon on `brewery-daemon/src/test/resources/projects/maven-multi`, get the model, assert three modules, shut down.
- [ ] **Step 3: Implement**, run `cargo test -p brewery_daemon_client` (and with `BREWERY_DAEMON_JAR=<repo>/brewery-daemon/build/libs/brewery-daemon-all.jar`), clippy `-D warnings`.
- [ ] **Step 4: Commit** `feat: brewery_daemon_client — JDK discovery and daemon JSON-RPC client`

---

### Task 4: Gradle/Maven tool window

**Files:** `crates/brewery_build/` (panel), `crates/zed/src/zed.rs` (load panel next to the other panels in `initialize_panels`, marked), `crates/zed/Cargo.toml`, root `Cargo.toml`.

**Interfaces:**
- Consumes: `brewery_daemon_client::{DaemonClient, ProjectModel, BuildTool}`; `workspace::{Panel, DockPosition}`; `workspace::Workspace::schedule_task(TaskSourceKind, &TaskTemplate, &TaskContext, omit_history: bool, window, cx)` (`crates/workspace/src/tasks.rs:29`).
- Produces: `pub struct BuildPanel` implementing `workspace::Panel` with `persistent_name() = "Build Tool Panel"`, default position Right, icon `IconName::` (pick a fitting existing icon, e.g. `Hammer` if present, else `Settings`), tooltip "Gradle/Maven"; `pub fn init(cx)` registering `brewery::ToggleBuildPanel` (`actions!(brewery, [ToggleBuildPanel, ReloadBuildProject])`).
- Pure helper `pub fn task_command(model: &ProjectModel, module_path: &str, task: &str, os_windows: bool) -> (String, Vec<String>)`: Gradle → `gradlew(.bat)` in the root if it exists else `gradle`, args `[":app:build"]` (root path `:` → just `build`); Maven → `mvnw(.cmd)` if present else `mvn`, args `["-pl", "core", "package"]` (root → `["package"]`).

Behaviour: when the panel is first shown (or on workspace open if the project has build files), start a `DaemonClient` for the first worktree root, fetch the model, render a tree: build tool header with a Reload button, modules collapsible, tasks under each (grouped by `group` when present). Double click (or Enter) on a task → `schedule_task` with a `TaskTemplate { label: format!("{module}: {task}"), command, args, cwd: Some(root), ..Default::default() }` so it runs in the terminal and becomes the Run widget's "Rerun" target. `project/changed` → refetch. Errors (no JDK, no jar, import failed) show as a message in the panel with the error text and a Reload button; nothing panics. The daemon is shut down when the panel/workspace is dropped.

- [ ] **Step 1: Failing tests** — `task_command` for Gradle (root/module, with and without wrapper, Windows/non-Windows) and Maven (root/module, wrapper/no wrapper) using a temp dir; a `#[gpui::test]` that adds the panel to a test workspace and checks `persistent_name`, default position Right and that `ToggleBuildPanel` opens it.
- [ ] **Step 2:** implement; **Step 3:** `cargo test -p brewery_build`, clippy, `cargo check -p zed`.
- [ ] **Step 4: Commit** `feat: Gradle/Maven tool window backed by brewery-daemon`

---

### Task 5: CI, docs, on-screen check

- [ ] `.github/workflows/brewery_ci.yml`: new job `daemon` (ubuntu-24.04 and windows-2025): `actions/setup-java@v4` with `distribution: temurin`, `java-version: 21`; `./gradlew test shadowJar` in `brewery-daemon/` (`gradlew.bat` on Windows, `shell: bash` works with `./gradlew` on both); upload `brewery-daemon-all.jar` as artifact. Add `-p brewery_daemon_client -p brewery_build -p extension_host` to the Rust hook-in test step. Validate with actionlint.
- [ ] `docs/brewery/FORK.md`: daemon build commands; `README.md`: dev setup — JDK 21 needed, `cd brewery-daemon && ./gradlew shadowJar` before running Brewery.
- [ ] Controller: build the jar, run Brewery on a clean profile (no other Brewery instance running — the single-instance check would hand the project to it) with the demo project and a Maven fixture project; open the Gradle/Maven tool window from the right stripe; double-click `build`/`package`; it runs in the terminal.
- [ ] Commit `ci: build and test brewery-daemon; docs for the daemon`.
