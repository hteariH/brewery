# Brewery — design

Date: 2026-10-10
Status: M0 done locally 2026-10-10 (builds and hook tests pass on Windows and Linux); CI not run yet — repo not published

## 1. Goal

Brewery is an open-source Java IDE for Windows and Linux that covers the
features people buy IntelliJ IDEA Ultimate for — Spring support and database
tools — on top of a fast, natively compiled, GPU-rendered editor. It looks and
feels as close to IntelliJ IDEA (New UI) as is legally possible.

Brewery is built as a fork of [Zed](https://github.com/zed-industries/zed).
This is an experiment: if the fork turns out to be a poor fit, the UI will be
rewritten on GPUI directly. The architecture is shaped so that such a rewrite
replaces only the UI layer.

### Non-goals for 0.1

- Own Java parser or semantic analysis (we use Eclipse JDT LS).
- Oracle and MS SQL Server first-class support (generic JDBC mode only).
- Collaboration, Zed cloud account, Zed AI, telemetry to Zed servers.
- Official macOS support (an unsigned, unsupported build is published).
- Flatpak and AppImage packages.
- Git changelists, shelve, interactive rebase dialog, cherry-pick/revert
  from the log, GitHub pull request integration (planned for 0.2).
- Anything derived from IntelliJ IDEA Ultimate code. No decompilation, no
  reverse engineering of proprietary JetBrains plugins.

## 2. Decisions

| Topic | Decision |
|---|---|
| Rendering | Native code, GPU rendering, single cross-platform look (Zed/GPUI) |
| Base | Fork of Zed (GPL-3) |
| Scope of 0.1 | Java core + Spring + databases + IntelliJ-style git core (log graph, file history, 3-way merge, branches) |
| Git logic | In the fork (Rust), on top of Zed's `git` crate — the one exception to "logic lives in the daemon" |
| Databases | Deep support: PostgreSQL, MySQL/MariaDB, SQLite. Any other database through a generic JDBC mode with a user-supplied driver |
| Java runtime | Bundled jlink'ed Temurin JRE for internal servers only; projects build and run on the user's JDK; IDE can discover and download JDKs |
| Platforms | Windows and Linux supported; macOS unsigned and unsupported |
| Name | Brewery |
| Licences | Fork: GPL-3. `brewery-daemon` and protocol: Apache 2.0 |
| Architecture | Thin Zed fork for UI + one JVM daemon for project/run/test/DB/JDK + off-the-shelf language servers |
| Look | IntelliJ New UI layout: tool window stripes, main toolbar with run widget, bottom tool windows |

## 3. Legal boundaries

- No JetBrains names, logos or product icons anywhere in the product, its
  binaries, its packaging or its marketing.
- Platform icons are taken from `JetBrains/intellij-community` (Apache 2.0)
  with attribution in `NOTICE`. Product icons and the JetBrains logo are not.
- Fonts: JetBrains Mono (OFL) for code, Inter (OFL) for UI.
- The README states: "Inspired by the IntelliJ IDEA interface. Not affiliated
  with or endorsed by JetBrains."
- Behaviour may be studied by using IntelliJ IDEA; its binaries are never
  decompiled or inspected.

## 4. Repositories

| Repository | Contents | Licence |
|---|---|---|
| `brewery` | Zed fork: UI, theme, icons, tool windows, toolbar, daemon client, packaging | GPL-3 |
| `brewery-daemon` | Kotlin/JVM daemon and the JSON-RPC protocol schema | Apache 2.0 |

This directory (`brewery/` in the workspace) holds design documents until the
two repositories exist; it will then become the docs repository or be merged
into `brewery`.

Two repositories because the licences, languages and release rhythms differ:
the fork regularly merges upstream Zed, the daemon does not depend on it.

## 5. Runtime processes

```
brewery (UI, Rust)
 ├─ on project open:
 │   ├─ brewery-daemon     one per project window, bundled JRE
 │   ├─ jdtls              bundled JRE
 │   └─ spring-boot-ls     bundled JRE, only when the project uses Spring
 └─ on debug: java-debug (jdtls plugin) ↔ the user's application JVM
```

User applications, tests and builds run on the project's JDK, never on the
bundled JRE.

### Responsibility split

| Capability | Owner |
|---|---|
| Java completion, diagnostics, navigation, refactorings, formatting | jdtls (LSP) |
| `application.yml` / `.properties`, beans, endpoints, Spring hints | Spring Tools LS (LSP) |
| Breakpoints, stepping, variables | java-debug (DAP; Zed already has a DAP client) |
| Gradle task tree, Maven goals, run configurations, process start/stop, tests | `brewery-daemon` |
| Databases | `brewery-daemon` |
| JDK discovery and download | `brewery-daemon` |

Deliberate duplication: jdtls imports Gradle/Maven to learn the classpath;
the daemon independently reads the same build to learn what can be run. The
classpath for language features comes from jdtls; runnable things come from
the daemon.

## 6. UI ↔ daemon protocol

- Transport: JSON-RPC 2.0 over stdio with `Content-Length` framing (as LSP).
  The UI reuses Zed's `lsp` crate transport.
- Handshake: `initialize { protocolVersion }`. On mismatch the UI shows an
  error and does not talk to the daemon.
- Schema: JSON Schema in `brewery-daemon/protocol/`. Kotlin and Rust types
  are generated from it; CI fails if generated code differs from the committed
  code.

### Methods

Requests `→`, daemon notifications `←`.

```
initialize, shutdown
project/open, project/model             → modules, tasks, project JDK
                                          ← project/changed
run/configurations, run/start, run/stop → processes
                                          ← run/output { runId, stream, text }
                                          ← run/exited { runId, code }
test/discover, test/run                  ← test/result { id, status, durationMs, failure? }
db/connections/list|save|delete|test
db/introspect { connectionId, path }     → lazily loaded schema tree level
db/execute { connectionId, sql }         → { cursorId, columns, updateCount? }
db/fetch { cursorId, offset, limit }     → rows
db/cancel { cursorId }
db/close { cursorId }
jdk/list, jdk/download                   ← jdk/progress
```

- Large results are paged through cursors; never one response with all rows.
- Process output streams as notifications; the UI caps the console buffer.
- Every request is cancellable via `$/cancelRequest`.

### Errors

Standard JSON-RPC codes plus domain codes, each with a user-facing message and
log details: `DB_AUTH_FAILED`, `DB_CONNECTION_FAILED`, `DB_QUERY_FAILED`,
`JDK_NOT_FOUND`, `BUILD_IMPORT_FAILED`, `RUN_START_FAILED`, `INTERNAL_ERROR`
(carries a log entry id).

If the daemon dies the UI restarts it (at most 3 times per 5 minutes, then
stops and shows a notification with a link to the log). Open editors are not
affected.

## 7. UI in the fork

### Reused from Zed

- `title_bar` — already has project and branch pickers; gets the run widget.
- `Panel` trait and left/right/bottom docks — kept; panel buttons move from
  the status bar to vertical stripes along the window edges.
- Project panel, outline panel (→ Structure), git panel, terminal,
  diagnostics (→ Problems).
- Editor runnables in the gutter — filled for `main`, tests and Spring Boot.
- Built-in JetBrains base keymap — default for Brewery.

### New crates

| Crate | Purpose |
|---|---|
| `brewery_chrome` | tool window stripes, main toolbar, Run/Debug widget, navigation bar |
| `brewery_daemon_client` | daemon lifecycle, JSON-RPC, restarts, generated types |
| `brewery_run` | Run and Services tool windows, console with stack trace links |
| `brewery_build` | Gradle / Maven tool windows: task tree, reload project |
| `brewery_db` | Database tool window, SQL console, virtualised result grid |
| `brewery_search_everywhere` | Shift Shift modal with Classes / Files / Symbols / Actions tabs over Zed's existing finders |
| `brewery_git` | Log with commit graph, file history, 3-way merge, branches popup (section 7a) |
| `brewery_theme` | Brewery Dark and Light themes, icon set with `NOTICE` |

### Tool windows in 0.1

- Left: Project, Commit, Structure, Database
- Right: Gradle/Maven, Notifications
- Bottom: Run, Debug, Services, Problems, Terminal, Git (Log)

### Spring in the editor

- Gutter icons on beans and endpoints; click navigates between injection
  points and bean definitions.
- Hover and completion come from Spring Tools LS.
- Services lists running Spring Boot apps with links to their endpoints.

### SQL in the editor

The daemon sends the cached schema; SQL parsing for completion uses
tree-sitter in the UI.

### Upstream merges

Monthly merge from Zed's stable branch. Edits inside Zed's own files are
limited to short hook-ins, each marked `// brewery:` so that
`grep -r "brewery:"` lists every place where the fork touches Zed code.

### Removed from Zed

Collaboration, Zed account and cloud, Zed AI, telemetry. The auto-updater is
pointed at GitHub Releases.

## 7a. Git

Git logic stays in Rust inside the fork, in `brewery_git`, built on Zed's
`git` crate. Zed already has a mature git layer; duplicating it on the JVM
would add nothing. If the UI is ever rewritten on GPUI, this crate moves with
it.

### Reused from Zed

Changes panel with file- and hunk-level stage/unstage, commit, amend; gutter
change markers with hunk revert; inline blame; branch switching; fetch, pull,
push; project-wide diff; inline conflict markers with ours/theirs actions.
The exact set is re-checked against current upstream in M0.

The changes panel becomes the IntelliJ-style **Commit** tool window on the
left stripe.

### New in 0.1

- **Log** (bottom tool window): commit graph with lanes, filters by branch,
  author, date, path and message text; commit details with changed files and
  diff. Graph layout is computed incrementally and rows are virtualised so
  that repositories with 100k+ commits stay responsive.
- **File history**: per-file log, compare any revision with the working copy.
- **Compare with branch / revision** for a file or the whole project.
- **3-way merge**: ours / result / theirs panes, accept per-hunk from either
  side, auto-merge of non-conflicting hunks, "accept left/right" for the whole
  file. Opens from the conflicts list after a merge, rebase or pull.
- **Branches popup** (from the toolbar branch widget): local and remote
  branches, recent branches, checkout, new branch, rebase onto, merge into
  current, compare with current, delete.

### After 0.1

Changelists, shelve, interactive rebase dialog, cherry-pick and revert from
the Log, GitHub pull requests.

## 8. `brewery-daemon`

Kotlin, Java 21 (bundled JRE), coroutines, `org.eclipse.lsp4j.jsonrpc` for
transport.

| Module | Purpose | Built on |
|---|---|---|
| `protocol` | generated types and method interfaces | codegen from JSON Schema |
| `server` | handshake, routing, cancellation, logging | lsp4j.jsonrpc |
| `project.gradle` | modules, tasks, project JDK, live build and test events | Gradle Tooling API |
| `project.maven` | modules and goals from `pom.xml`; runs `mvnw` / `mvn` | maven-model-builder |
| `run` | run configurations, processes, output streaming, process-tree kill | ProcessBuilder / ProcessHandle |
| `run.spring` | finds `@SpringBootApplication`, generates run configurations, profiles | JavaParser (annotations only) |
| `test` | discovery and execution; Gradle live events, Maven surefire reports | |
| `db` | connections, introspection, execution, cursors, cancellation | JDBC + dialects |
| `jdk` | discovery of installed JDKs, Temurin download | Adoptium API |

### Storage

- `.brewery/run.json` — run configurations (committable).
- `.brewery/db.json` — database connections without secrets (committable).
- Passwords — OS secret store (Windows Credential Manager, libsecret) via
  `java-keyring`. Never written to files.
- Logs — `%LOCALAPPDATA%\Brewery\logs`, `~/.local/state/brewery/logs`.

### Databases

- One thread and one `java.sql.Connection` per connection.
- Cancellation via `Statement.cancel()`.
- Introspection: `DatabaseMetaData` as the base; PostgreSQL, MySQL and SQLite
  dialects add schemas, indexes, foreign keys, views, sequences. Generic JDBC
  mode uses `DatabaseMetaData` only.
- Bundled drivers: PostgreSQL, MySQL Connector/J (or MariaDB), SQLite (xerial).
  User drivers are loaded from a JAR, each in its own classloader.

### Failure behaviour

- Requests fail only with domain error codes; uncaught exceptions become
  `INTERNAL_ERROR` with a log entry id.
- A failed Gradle sync keeps the previous project model and posts a
  notification linking to the build output.

## 9. Testing

### Daemon

- Unit tests per module.
- Integration tests against real Gradle and Maven projects in `fixtures/`.
- Testcontainers for PostgreSQL and MySQL; SQLite on a temp file.
- Contract tests: every schema method responds according to the schema.

### UI

- GPUI `TestAppContext` tests for our crates, with a fake daemon replaying
  recorded responses.
- `brewery_git`: tests against throwaway repositories built by test helpers
  (linear history, merges, octopus merge, conflicts); graph layout tested on
  a generated 100k-commit repository for speed.

### End to end

Reference project: `spring-petclinic`. Before every release: open the
project, wait for import, get Java and `application.yml` completion, run the
app, hit a breakpoint, run SQL against the database, open the Log and file
history, resolve a prepared merge conflict in the 3-way merge. Manual checklist first,
automated later.

Tracked metrics: start-up to open window, petclinic import time, typing
latency, total memory of all processes.

## 10. CI and releases

### CI (GitHub Actions)

- Matrix: Windows, Ubuntu, macOS (unsupported). Rust build cache.
- On PRs: `cargo check` + tests for Brewery crates. Full builds on `main` and
  on tags.
- `brewery-daemon`: Gradle build and tests, including Testcontainers.
- Bundle script downloads jdtls, Spring Tools LS, java-debug and JDBC drivers
  at pinned versions with checksums and builds the JRE with `jlink`.

### Packages

| Platform | Packages |
|---|---|
| Windows | Inno Setup installer, winget manifest |
| Linux | `.deb`, `.rpm` (both built with nFPM), AUR packages (`brewery-bin` from the release tarball) |
| macOS | unsigned zip, marked unsupported |

The Linux release tarball exists only as the source for `brewery-bin`; it is
not advertised as a separate install method.

Windows code signing: apply to SignPath Foundation (free for open source)
before 0.1. Until then SmartScreen warns on install.

Estimated bundle size: 250–300 MB.

## 11. Milestones

Each milestone is checked against `spring-petclinic`.

| Milestone | Result |
|---|---|
| M0 Fork | builds on Windows and Linux, renamed to Brewery, Zed cloud features removed, CI green |
| M1 Look | theme, icons, keymap, tool window stripes, main toolbar, Search Everywhere |
| M2 Java core | bundled jdtls and JRE, daemon skeleton, Gradle/Maven model, run configurations, tests, debugging |
| M3 Spring | Spring Tools LS, bean and endpoint gutter icons, Services, one-click Spring Boot run |
| M4 Databases | Database tool window, SQL console, result grid, PostgreSQL / MySQL / SQLite, generic JDBC |
| M5 Git | Commit tool window, Log with graph and filters, file history, compare with branch/revision, 3-way merge, branches popup |
| M6 Release 0.1 | installers and packages, auto-update via GitHub Releases, README with JetBrains disclaimer, public release |

Each milestone gets its own implementation plan. M0 is planned first.

## 12. Open risks

- **Zed internals churn.** Moving dock buttons to stripes touches `workspace`,
  which changes often upstream. Mitigation: hook-in rule from section 7,
  monthly merges rather than rare large ones.
- **Spring Tools LS outside VS Code/Eclipse.** It is distributed inside the
  VS Code extension and Eclipse update site and expects some client-specific
  commands. M3 starts with a spike to confirm which features work over plain
  LSP.
- **Git log on large repositories.** Graph layout over 100k+ commits can
  stall the UI. Mitigation: incremental layout off the main thread, virtual
  rows, a speed test in CI (section 9).
- **Zed build cost on CI.** Full builds take 30–40 minutes; PRs run only
  checks for Brewery crates.
- **GPL-3 obligations** apply to the fork and its binaries; the daemon stays
  Apache 2.0 because it runs as a separate process and talks over a protocol.
