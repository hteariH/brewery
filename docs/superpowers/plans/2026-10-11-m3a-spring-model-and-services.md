# Brewery M3a — Spring Model and Services Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Brewery understands a Spring project the way IntelliJ's Services and Endpoints views do: it finds Spring Boot applications, beans and HTTP endpoints, lists them in a **Services** tool window (bottom), runs a Spring Boot application with one click, and jumps to any bean or endpoint in the source.

**Architecture:** Spec deviation (Ruling, recorded): instead of embedding Spring Tools LS — which needs a jdtls bridge (classpath listeners, javadoc commands) that Zed's LSP client cannot provide (spec §12 risk confirmed) — Spring knowledge lives in `brewery-daemon`. The daemon parses Java sources with JavaParser (syntax only, no symbol resolution: annotations matched by simple name), exposes `spring/model` and `run/configurations`, and notifies `spring/changed` when Java sources change. The Rust side gets one shared daemon per project (`brewery_daemon_client::ProjectDaemon`, a GPUI entity in a global registry keyed by project), used by both the Gradle/Maven panel and the new Services panel. Config-file completion (`application.yml`/`.properties`) is M3b.

**Tech Stack:** Kotlin, `com.github.javaparser:javaparser-core:3.28.2`; Rust GPUI panel crate `brewery_services`.

**Spec:** `docs/superpowers/specs/2026-10-10-brewery-design.md` (sections 7 "Spring in the editor", 8 `run.spring`, 11 M3).

## Global Constraints

- Protocol additions (JSON, camelCase), fixtures in `brewery-daemon/protocol/fixtures/`:
  - `spring/model {}` → `SpringModel { applications: [SpringApp], beans: [SpringBean], endpoints: [SpringEndpoint] }`
  - `SpringApp { name, mainClass, modulePath, file, line }` (file absolute path, line 1-based)
  - `SpringBean { name, type, kind, file, line }`, `kind` ∈ `component | service | repository | controller | restController | configuration | bean`
  - `SpringEndpoint { method, path, handler, file, line }`, `method` ∈ `GET | POST | PUT | DELETE | PATCH | ANY`; `handler` = `SimpleClass#method`
  - `run/configurations {}` → `[RunConfiguration { label, command, args, cwd, kind }]`, `kind` ∈ `springBoot | application`
  - notification `spring/changed {}` (Java sources changed; debounced like `project/changed`)
- Annotation matching by simple name (also accepts the fully-qualified form): `SpringBootApplication`, `Component`, `Service`, `Repository`, `Controller`, `RestController`, `Configuration`, `Bean`, `RequestMapping`, `GetMapping`, `PostMapping`, `PutMapping`, `DeleteMapping`, `PatchMapping`.
- Endpoint path = class `@RequestMapping` value/path (first) + method mapping value/path (first), normalised to one leading `/` and no `//`; `@RequestMapping` on a method with `method = RequestMethod.X` gives X, else ANY.
- Bean name: explicit annotation value if a string literal, else class name with first letter lowercased; `@Bean` methods → method name, type = method return type text.
- Source roots scanned: for each module from the project model, `src/main/java` (and `src/main/kotlin` is out of scope for M3a); skip `build/`, `target/`, `.gradle/`.
- Spring Boot run command: Gradle module with a `bootRun` task → `<wrapper or gradle> :<module>:bootRun` (root → `bootRun`); Gradle without `bootRun` → `<wrapper> :<module>:run -PmainClass=<fqcn>` only if an `application` plugin task `run` exists, else skip; Maven → `<mvnw or mvn> spring-boot:run -pl <module> -am` (root → `spring-boot:run`). Use the same wrapper detection as `brewery_build::task_command`.
- Zed-file edits marked `brewery:`; LF; no unwrap/expect outside tests; never read an entity while it is being updated.

---

### Task 1: Daemon — Spring model

**Files:** `brewery-daemon/src/main/kotlin/dev/brewery/daemon/spring/{SpringScanner.kt,SpringModel.kt}`, `Protocol.kt` (types + `@JsonRequest("spring/model")`, `@JsonNotification("spring/changed")` on `BreweryClient`), `BreweryDaemon.kt` (wire), `build.gradle.kts` (javaparser dependency, notices regenerate), tests + fixture project `src/test/resources/projects/spring-petshop/` (Gradle, `src/main/java/com/example/petshop/{PetshopApplication.java,OwnerController.java,OwnerRepository.java,OwnerService.java,AppConfig.java}`), fixture `protocol/fixtures/spring-model.json`.

- [ ] Failing tests (`SpringScannerTest`): the fixture yields 1 application (`com.example.petshop.PetshopApplication`), beans `ownerController`(restController), `ownerRepository`(repository), `ownerService`(service), `appConfig`(configuration), `clock`(bean, type `Clock`); endpoints `GET /owners`, `GET /owners/{id}`, `POST /owners`, `ANY /owners/legacy` with handlers `OwnerController#list` etc.; files are absolute, lines point at the annotated declaration; a file with a syntax error is skipped without failing the scan; `spring/model` over a Launcher pair round-trips the fixture shape; `spring/model` before `project/open` → -32002.
- [ ] Implement scanning off the JSON-RPC thread (reuse the single-thread importer executor or a dedicated one), cache until a `.java` change under a scanned root (extend the watcher's file filter; separate debounced `spring/changed`).
- [ ] `./gradlew test shadowJar` (JAVA_HOME = JDK 21); notices test still passes with javaparser added.
- [ ] Commit `feat(daemon): Spring applications, beans and endpoints from sources`.

### Task 2: Daemon — run configurations

**Files:** `brewery-daemon/src/main/kotlin/dev/brewery/daemon/run/RunConfigurations.kt`, `Protocol.kt`, `BreweryDaemon.kt`, tests, fixture `protocol/fixtures/run-configurations.json`.

- [ ] Failing tests: Gradle fixture with `bootRun` → `{label: "PetshopApplication", command: "<root>/gradlew(.bat)", args: ["bootRun"], cwd: root, kind: springBoot}` (root module) and `[":app:bootRun"]` for a submodule; Gradle without bootRun but with `run` → `["run", "-PmainClass=..."]`, kind `application`; Maven → `spring-boot:run` / `-pl core -am spring-boot:run`; Windows vs non-Windows wrapper names (inject the OS flag).
- [ ] Implement from the cached project model + spring model.
- [ ] Commit `feat(daemon): Spring Boot run configurations`.

### Task 3: Rust — shared project daemon

**Files:** `crates/brewery_daemon_client/src/{project_daemon.rs, protocol.rs}`, `crates/brewery_build/src/brewery_build.rs` (use the shared daemon instead of owning one).

**Interfaces:**
- `pub struct ProjectDaemon` (GPUI entity) owning the `DaemonClient`, restart policy and state (`Connecting | Ready | Failed { error, kind }`), emitting `ProjectDaemonEvent::{Ready, ProjectChanged, SpringChanged, Failed}`; `pub fn ProjectDaemon::for_project(project: &Entity<Project>, cx: &mut App) -> Entity<ProjectDaemon>` (global registry `HashMap<EntityId, WeakEntity<ProjectDaemon>>`; created lazily; dropped when no panel holds it); `pub fn project_model(&self, cx) -> Task<Result<ProjectModel>>`, `spring_model`, `run_configurations`, `reload(cx)`.
- Move the restart policy (3 per 5 min) and error-kind logic from `brewery_build` into `ProjectDaemon`; `BuildPanel` subscribes to events. Keep `BuildPanel`'s existing tests passing (adapt the fake-daemon seam to `ProjectDaemon`).
- [ ] Tests: two subscribers share one daemon (start called once); restart policy still enforced; `brewery_build` tests green.
- [ ] Commit `refactor: one shared daemon per project for all Brewery panels`.

### Task 4: Services tool window

**Files:** `crates/brewery_services/` (new crate), `crates/zed/src/zed.rs` (load panel, marked), `Cargo.toml`s.

**Interfaces:** `pub struct ServicesPanel` (`Panel`, persistent name `"Services Panel"`, default position Bottom, icon `IconName::Server` if present else `IconName::Box`, tooltip "Services"), actions `brewery::ToggleServicesPanel`.

Behaviour: tree with three sections — **Spring Boot** (applications with ▶ Run button → `Workspace::schedule_task` with a `TaskTemplate` from the matching `RunConfiguration`, label `"<App> (Spring Boot)"`, so the Run widget's Rerun picks it up), **Endpoints** (grouped by controller simple name; each row `GET /owners/{id}` with the method coloured; click → open `file` at `line`), **Beans** (grouped by kind; click → open). Reload button; empty states ("No Spring Boot application found"); errors from `ProjectDaemon`. Opening a file at a line: `workspace.open_abs_path(...)` then move the cursor (find the helper Zed uses for "open file at line", e.g. `editor::Editor::go_to_singleton_buffer_point` or `workspace::OpenOptions`), never reading the workspace inside a panel update callback (capture what you need when the panel is created; defer).

- [ ] Tests: grouping helpers (endpoints by controller, beans by kind, method label); a gpui test that toggles the panel through `ToggleServicesPanel` with a fake `ProjectDaemon` returning a fixed model and asserts the rendered rows count; run-button builds the expected `TaskTemplate` from a `RunConfiguration`.
- [ ] `cargo test -p brewery_services -p brewery_build -p brewery_daemon_client`, clippy, `cargo check -p zed`.
- [ ] Commit `feat: Services tool window with Spring Boot apps, endpoints and beans`.

### Task 5: CI, docs, on-screen check
- [ ] CI hook-in list + FORK.md: add `-p brewery_services`. README: Spring features section (what works, what M3b adds).
- [ ] Controller: release-fast build or debug; open `brewery-daemon/src/test/resources/projects/spring-petshop`; Services panel shows the app, 4 endpoints, 5 beans; click an endpoint opens `OwnerController.java` at the line.
- [ ] Commit `docs: Spring support in M3a`.
