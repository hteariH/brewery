# Brewery M3b — Spring Configuration Language Server Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** In `application*.properties` and `application*.yml`/`.yaml`, Brewery completes Spring Boot configuration keys (with type, default and description), completes values for booleans and enums, shows documentation on hover, and warns about unknown and deprecated keys — like IntelliJ Ultimate.

**Architecture:** A second entry point in `brewery-daemon`'s jar runs a standard LSP server over stdio (`java -jar brewery-daemon-all.jar --spring-config-lsp`, using `org.eclipse.lsp4j:org.eclipse.lsp4j:1.0.0`). It finds the project's dependency jars (Gradle: Tooling API `IdeaProject` library dependencies; Maven: `mvn(w) -q dependency:build-classpath -Dmdep.outputFile=<tmp>`), reads `META-INF/spring-configuration-metadata.json` and `META-INF/additional-spring-configuration-metadata.json` from each jar, and serves completion/hover/diagnostics. On the Rust side, crate `brewery_spring_config` implements Zed's `LspAdapter` (binary = JDK 21+ from `brewery_daemon_client::find_java`, jar from `find_daemon_jar`) and registers it for the `YAML` and `Properties` languages with `LanguageRegistry::register_lsp_adapter` (the pattern Zed uses for Tailwind in `crates/languages/src/lib.rs`). The server answers only for documents whose file name matches `application(-<profile>)?.(properties|yml|yaml)` (and `bootstrap*`); for other files it returns empty results, so other YAML files are untouched.

**Spec:** `docs/superpowers/specs/2026-10-10-brewery-design.md` (section 7 "Spring in the editor": `application.yml`/`.properties` completion and hints; M3).

## Global Constraints
- Daemon: Apache 2.0, package `dev.brewery.daemon.springconfig`; stdout = LSP only (System.out redirected like `Main.kt` does).
- Metadata model (Spring Boot format): `groups[]`, `properties[] { name, type, description, defaultValue, deprecation { level, replacement, reason } }`, `hints[] { name, values[] { value, description } }`.
- Key matching: relaxed binding — compare canonical forms (lowercase, `_` and `-` removed, `[n]` indices stripped); map-typed properties (`java.util.Map<...>`) accept any sub-key under their name.
- Diagnostics: unknown key → Warning "Unknown Spring Boot property '<key>'"; deprecated → Warning with replacement/reason; only when metadata was found (if no metadata at all, publish no diagnostics). Keys under `logging.level.*`, `spring.profiles.*` and any map-typed property are never "unknown".
- Completion items: label = full key (properties) or the next key segment (YAML, nested under the parent at the cursor), detail = type simple name, documentation = description (+ "Default: x"), deprecated flag; values: `true`/`false` for Boolean, hint values, enum constants are out of scope (no class loading).
- YAML parsing: block mappings and `- ` sequences are enough; derive the key path at a position from indentation (no flow-style support); documents separated by `---`.
- Rust: Zed-file edits marked `brewery:`; new crate per Brewery conventions; LF only; no unwrap/expect outside tests.
- Server name: `brewery-spring-config`.

### Task 1: Metadata + classpath (daemon)
**Files:** `brewery-daemon/src/main/kotlin/dev/brewery/daemon/springconfig/{Metadata.kt, ClasspathResolver.kt}`, tests, fixture jar(s) built in the test from a JSON file (zip a `META-INF/spring-configuration-metadata.json` into a temp jar at test time).
- [ ] TDD: metadata parsing (properties, hints, deprecation), merging several jars, relaxed key canonicalisation (`server.servlet.context-path` == `server.servlet.contextPath` == `SERVER_SERVLET_CONTEXTPATH` canonical), map-typed property detection; classpath resolver for Gradle via Tooling API on a fixture project with one `implementation` dependency on a local flat-dir jar (no network: use `repositories { flatDir { dirs("libs") } }` and a jar the test creates), and Maven via a stubbed command runner (unit-test the command line and output-file parsing; do not require Maven installed).
- [ ] Commit `feat(daemon): Spring configuration metadata from the classpath`.

### Task 2: The language server (daemon)
**Files:** `brewery-daemon/src/main/kotlin/dev/brewery/daemon/springconfig/{SpringConfigServer.kt, PropertiesDocument.kt, YamlDocument.kt}`, `Main.kt` (dispatch on `--spring-config-lsp`), `build.gradle.kts` (lsp4j dependency), tests.
- [ ] TDD: `PropertiesDocument` key at position, value position, all key/line pairs; `YamlDocument` key path at position, all key paths with ranges, `---` documents; server tests through an lsp4j Launcher pair: `initialize` (rootUri) → `didOpen` an `application.properties` → `completion` at an empty line returns keys from the fixture metadata; on `server.port=` value completion for Boolean/hints; `hover` returns description; diagnostics for an unknown key and a deprecated key; a non-application file gets no diagnostics/completion.
- [ ] Capabilities: completion (trigger characters `.` `:` `=`), hover, textDocumentSync full; publish diagnostics on open/change (debounced 300 ms).
- [ ] Commit `feat(daemon): Spring configuration language server`.

### Task 3: Zed adapter
**Files:** `crates/brewery_spring_config/` (new), registration hook in `crates/zed/src/main.rs` next to `languages::init` (marked), Cargo files.
- [ ] Read `crates/language/src/language.rs` for the `LspAdapter` / `LspInstaller` traits this Zed version uses, and an adapter that runs a user-provided binary without downloading (e.g. how `check_if_user_installed` works in `crates/languages/src/rust.rs` or `json.rs`), and `register_lsp_adapter` in `crates/language/src/language_registry.rs`.
- [ ] Adapter returns binary `<java> -jar <daemon jar> --spring-config-lsp`; if no JDK or jar is found, the server is not started and a single log warning explains why (no panics, no repeated popups).
- [ ] Register for `YAML` and `Properties` (the Java extension's properties language name is `Properties`).
- [ ] Tests: unit tests for the binary command (with a fake java/jar path); `cargo test -p brewery_spring_config`, clippy, `cargo check -p zed`.
- [ ] Commit `feat: Spring configuration language server for application.yml and .properties`.

### Task 4: CI, docs, on-screen check
- [ ] CI hook-in tests: add `-p brewery_spring_config`. README "Spring support": add config completion. FORK.md test list.
- [ ] Controller (capture-only, no synthetic input): open the spring-petshop fixture with an `application.properties` containing an unknown key; diagnostics appear in the editor (visible underline / Problems count) once the server has started. Ask the user to check completion interactively.
- [ ] Commit `docs: Spring configuration completion`.
