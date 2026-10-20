# Brewery Lite: a Zed extension with Brewery's non-UI features

Date: 2026-10-20. Status: approved.

## Goal

Ship the parts of Brewery that need no custom UI as a regular Zed extension, so people on stock Zed get them without switching editors. The full IDE stays a fork: Zed extensions can't add panels, tool windows, menus or popups, which is where most of Brewery lives.

Brewery Lite contains:

1. Spring Boot configuration language server: completion, hover and diagnostics in `application*.yml` and `application*.properties` (the daemon's existing `--spring-config-lsp` mode).
2. An MCP context server for Zed's AI agent: project model, Spring beans and endpoints, run configurations, database schema and read-only queries (a new daemon mode, `--mcp`).
3. The Brewery theme and the IntelliJ icon theme.

Out of scope: gutter debugging (the extension API lacks the `finalize_substituted_config` hook our locator needs), SQL completion (no connection UI in Lite), keymap (Zed ships a JetBrains base keymap).

## Layout

`extensions/brewery-lite/` in this repository:

- `extension.toml`: id `brewery-lite`, name "Brewery Lite", version equal to `crates/zed/BREWERY_VERSION`, licence Apache-2.0, repository `https://github.com/hteariH/brewery`.
  - `[language_servers.brewery-spring-config]` with `languages = ["YAML", "Properties"]`.
  - `[context_servers.brewery]`.
  - `[[capabilities]] kind = "process:exec", command = "java", args = ["-version"]` and the same for the downloaded JRE's `java` (any path, `-version` only).
- `Cargo.toml`: `cdylib`, `zed_extension_api` from crates.io (the version matching `crates/extension_api`, 0.8.0), `sha2` for checksum checks. Not a member of the main workspace (it targets `wasm32-wasip2`).
- `src/lib.rs`: the `Extension` impl; `src/java.rs`, `src/daemon_jar.rs`: provisioning with pure, natively testable helpers.
- `themes/brewery.json`: copy of `assets/themes/brewery/brewery.json`.
- `icon_themes/brewery-icons.json` and `icons/`: copy of `crates/brewery_theme/icons` (with its NOTICE).
- `LICENSE-APACHE`, `README.md` (English).

The theme and icon copies are kept in sync by a small check in `script/brewery` that fails CI when they drift.

## Provisioning

Both servers run `java -jar <daemon jar> <mode>`.

Daemon jar:
- Downloaded from the GitHub release `v<extension version>` of `hteariH/brewery`, asset `brewery-daemon-<version>.jar`, into the extension's work directory. Matching versions, never "latest", so extension and daemon protocol always agree.
- Verified against the release's `SHA256SUMS` before first use; on mismatch the file is deleted and the server fails with a clear message.
- Older `brewery-daemon-*.jar` files in the work directory are deleted once the current one is in place.

Java 21+, first match wins:
1. `java_path` from the extension's settings (`lsp.brewery-spring-config.settings.java_path` for the LSP, the context server's settings for MCP).
2. LSP only: `JAVA_HOME` from the worktree's shell env, then `which java` in the worktree. MCP has no worktree access, so it skips this step.
3. `java -version` on PATH via `process:exec`; accepted when the parsed major version is >= 21.
4. Otherwise download Temurin 21 JRE for the current OS/arch from the Adoptium API (`api.adoptium.net/v3/binary/latest/21/ga/<os>/<arch>/jre/hotspot/normal/eclipse`) into the work directory and use its `bin/java`.

Failures surface through `set_language_server_installation_status` (LSP) or the context server's error, with the reason and what to set.

## Daemon: `--mcp` mode

MCP over stdio: newline-delimited JSON-RPC 2.0, implemented directly with Gson (no Kotlin MCP SDK, which pulls in ktor). Supports `initialize`, `notifications/initialized`, `ping`, `tools/list`, `tools/call`. Unknown methods return JSON-RPC errors. Logs go to stderr.

Project root: the process working directory (Zed starts context servers in the first worktree's root and does not send MCP roots).

Tools (results as JSON text content; tool errors as `isError: true` results):
- `project_model`: build tool, modules, source sets, dependencies, JDK, from `ProjectModelReader`.
- `spring_beans`: beans with type, source location and stereotype, from `SpringScanner`.
- `spring_endpoints`: HTTP endpoints (method, path, handler).
- `run_configurations`: main classes and Spring Boot applications (`RunConfigurations`).
- `db_schema` (`table` optional): tables and columns, or one table's columns.
- `db_query` (`sql`): read-only query, at most 200 rows, returns columns, rows and a `truncated` flag.

Database: the connection comes from the project's Spring config: `spring.datasource.url`, `username`, `password` in `src/main/resources/application.yml` / `application.properties` (default profile; `${ENV:default}` placeholders resolved from the environment). No datasource gives a tool error saying so. Read-only is enforced twice: the JDBC connection is `readOnly` inside a transaction that is always rolled back, and only a single statement starting with `SELECT`, `WITH`, `SHOW`, `EXPLAIN`, `DESCRIBE` or `VALUES` is accepted (comments stripped, using the existing `SqlText` splitter).

## Release

`brewery_release.yml` attaches `brewery-daemon-<version>.jar` (the shadow jar the bundle job already builds) to the draft release and lists it in `SHA256SUMS`. Publishing to the Zed extension registry is a PR to `zed-industries/extensions` adding this repo as a submodule with `path = "extensions/brewery-lite"`; the owner opens it. `docs/brewery/FORK.md` gets a "Brewery Lite" section describing the flow.

## Testing

- Daemon (Kotlin): MCP protocol round trips over piped streams (initialize, tools/list, each tool on a fixture project, unknown method, malformed line); datasource parsing (yml, properties, placeholders); read-only guard (accepts/rejects); `db_query` against Postgres in a container on CI, plus H2 locally.
- Extension (Rust): unit tests of the pure helpers on the host target (parse `java -version` output, asset names, SHA256SUMS lookup, Adoptium os/arch mapping); `cargo build --target wasm32-wasip2` in CI.
- CI: a `brewery-lite` job (path filter `extensions/brewery-lite/**`) building and testing the extension, plus the icon/theme drift check.
- Manual end to end: install as a dev extension in a Brewery build, confirm the Spring LSP starts on a Spring sample and the MCP tools answer in the agent panel.
