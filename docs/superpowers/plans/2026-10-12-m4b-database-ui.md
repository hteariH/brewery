# Brewery M4b — Database Tool Window, SQL Console, Result Grid Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Like IntelliJ's Database tool window: a **Database** panel on the left lists the project's data sources and lets the user add, edit, test and delete them; expanding a data source browses its schema lazily; **New Query Console** opens a SQL editor tab bound to the connection; Ctrl+Enter runs the statement at the cursor (or the selection) and shows the result in a virtualised grid under the editor, with paging, cancel, elapsed time and errors.

**Architecture:** Spec section 5 "New crates" (`brewery_db`) and section 8 "Databases"; the daemon side is done in M4a (`db/*` methods, `brewery-daemon/README.md` "Databases (daemon)"). Rust side:
- `brewery_daemon_client`: protocol types and client methods for every `db/*` method, plus `ProjectDaemon` wrappers (same pattern as `spring_model`).
- New crate `crates/brewery_db/`: `DatabasePanel` (dock panel), `DataSourceModal` (add/edit dialog), `QueryConsole` (workspace `Item` with an `Editor` on top and a `ResultView` below), `ResultGrid` (virtualised with `uniform_list`), pure helpers (statement splitting, URL templates, grid paging state).

**Tech Stack:** Rust, GPUI, Zed `workspace`/`editor`/`ui`/`menu`/`picker` crates; SQL highlighting via Zed's SQL extension auto-installed (`auto_install_extensions` default, like `java`).

**Spec:** `docs/superpowers/specs/2026-10-10-brewery-design.md` (sections 5, 8, 11 M4).

## Global Constraints

- Zed-file edits marked `brewery:`; new crate per Brewery conventions (license-file symlink/copy like `brewery_services`, `publish = false`, workspace deps); LF only; no `unwrap`/`expect` outside tests; never read an entity while it is being updated (capture entities at creation, defer with `cx.defer`/`spawn`).
- Panel: persistent name `"Database Panel"`, default position Left, icon `IconName::DatabaseZap` if present else `IconName::Server`/`Box`, tooltip "Database", action `brewery::ToggleDatabasePanel`; registered in `crates/zed/src/zed.rs` next to the Services panel (marked), and the tool-window stripes pick it up the same way as the other panels.
- Tree rows: data source (driver icon/label + name; error badge when introspection failed), then nodes from `db/introspect` by `path`; expansion is lazy and cached per data source; Refresh clears the cache of the selected data source. Column rows show `detail` muted. Folder nodes (`columns`, `indexes`, `foreignKeys`) show as "columns", "indexes", "keys".
- Toolbar: `+` (Add data source), Edit, Delete (confirm prompt), Refresh, New Query Console. Context menu on a data source with the same actions. Double-click on a table → opens a console with `SELECT * FROM <qualified name>` and runs it (identifier quoting per driver: `"` for postgresql/sqlite, `` ` `` for mysql/mariadb; quote only when needed).
- Data source dialog: Name, Driver (bundled + user drivers from `db/drivers`), URL (prefilled from a per-driver template: `jdbc:postgresql://localhost:5432/postgres`, `jdbc:mysql://localhost:3306/`, `jdbc:mariadb://localhost:3306/`, `jdbc:sqlite:<project root>/db.sqlite`), User, Password (masked; when editing, empty field + "stored" hint means keep; a "Forget password" checkbox sends `""`), **Test Connection** (shows ok + server version, or the error message), Save, Cancel. "Add driver…" section: name, class name, jar paths (comma/newline separated) → `db/drivers/addJar`. Warnings from save (`ConnectionInfo.warning`) shown as a toast.
- Query console: tab title `"<data source> console"`; buffer is an in-memory SQL buffer (language "SQL" when available); Ctrl+Enter (`brewery::RunStatement`) runs the selection or the statement at the cursor (split by `;` outside strings, quoted identifiers, `--` and `/* */` comments, PostgreSQL `$tag$` bodies); run-all is out of scope; `brewery::CancelStatement` also has a Stop button while running. One running statement per console; a new run closes the previous cursor (`db/close`).
- Result view: for queries a grid; for updates "N rows affected" + elapsed; for errors the daemon message in error colour. Status line: "N rows fetched[+] in X ms". Grid: fixed row height, header row with column name + muted type, columns sized from the first page (clamped 60–400 px, monospace measurement by char count), horizontal scroll; `null` cells render muted "<null>"; first page 500 rows fetched on execute, further pages (500) fetched when scrolling within 100 rows of the end, until `done`. Copy (`ctrl-c`) copies the selected cell's value; selecting a cell by click.
- All daemon calls go through `ProjectDaemon` for the panel's project; errors shown inline (panel: error row/notice; console: result view); daemon not ready → "Waiting for the Brewery daemon…".
- Tests: pure helpers unit-tested; gpui tests with a fake daemon seam (as in `brewery_services`), no real daemon required.

### Task 1: Client protocol for databases
**Files:** `crates/brewery_daemon_client/src/{protocol.rs, brewery_daemon_client.rs, project_daemon.rs}`, tests.
- [ ] Types mirroring M4a exactly (serde camelCase, enums by name, `Option` for nullable): `DriverInfo`, `ConnectionInfo` (incl. `warning: Option<String>`, `options: BTreeMap`), `SaveConnectionParams`, `TestConnectionResult`, `AddDriverParams`, `DbNode`/`DbNodeKind`, `IntrospectParams` (incl. optional `include_system`), `ExecuteResult`, `ColumnInfo`, `FetchResult { rows: Vec<Vec<Option<String>>>, done }`, cancel/close params. Error-code constants -32010..-32013 and a helper to read the daemon message from an LSP error.
- [ ] Client methods + `ProjectDaemon` wrappers for every `db/*` method.
- [ ] Tests: deserialize/serialize every `brewery-daemon/protocol/fixtures/db-*.json` fixture (read at test time via `include_str!` relative path) and assert field values.
- [ ] `cargo test -p brewery_daemon_client`, clippy.
- [ ] Commit `feat: database protocol in the daemon client`.

### Task 2: Database panel and data source dialog
**Files:** `crates/brewery_db/` (new: `brewery_db.rs`, `panel.rs`, `data_source_modal.rs`, `tree.rs`, tests), `crates/zed/src/zed.rs` + `Cargo.toml`s (marked), `assets/settings/default.json` only if a setting is needed (avoid).
- [ ] Pure `tree.rs`: flattened visible rows from data sources + expanded node cache; expand/collapse; refresh; error rows. Unit tests.
- [ ] Panel UI per Global Constraints; data source dialog per Global Constraints (URL templates as a pure fn, tested).
- [ ] gpui tests with a fake daemon: panel lists two data sources; expanding one calls introspect with `[]` then the child path and renders rows; Add → dialog → Save calls `db/connections/save` with the entered fields and refreshes the list; Delete asks for confirmation (test the confirmed path).
- [ ] `cargo test -p brewery_db`, clippy, `cargo check -p zed`.
- [ ] Commit `feat: Database tool window with data sources and schema tree`.

### Task 3: Query console and result grid
**Files:** `crates/brewery_db/src/{console.rs, statements.rs, result_grid.rs}`, tests; `assets/settings/default.json` (`auto_install_extensions`: add `"sql": true`, marked), the settings guard test in `crates/settings/src/brewery_defaults.rs`.
- [ ] Pure `statements.rs`: statement at offset / selection splitting (tests incl. strings with `;`, comments, `$$` bodies, cursor after the last `;`, empty statements skipped).
- [ ] Pure grid paging state: rows buffer, `needs_more(visible_end)`, `done`, column widths from the first page. Tests.
- [ ] Console `Item` + actions `RunStatement`, `CancelStatement` with keybindings in the JetBrains base keymap overlay used by Brewery (find where Brewery's keymap additions live; else add a small `brewery_db` keymap via `KeyBinding::new` in `init` with a context `QueryConsole`).
- [ ] Grid UI per Global Constraints (uniform_list, header, horizontal scroll, null rendering, cell selection + copy).
- [ ] Panel: "New Query Console" and table double-click open a console (double-click runs the generated SELECT).
- [ ] gpui tests with a fake daemon: run statement at cursor sends the right SQL; result rows render; scrolling near the end requests the next page; update count path; error path; cancel calls `db/cancel`; a second run closes the first cursor.
- [ ] `cargo test -p brewery_db -p settings`, clippy, `cargo check -p zed`.
- [ ] Commit `feat: SQL query console with a paged result grid`.

### Task 4: CI, docs, on-screen check
- [ ] CI hook-in tests and FORK.md test list: add `-p brewery_db`. README: "Databases" section (what works in 0.1, drivers, passwords in the OS store).
- [ ] Controller (capture-only, no synthetic input): release-fast build; open a scratch project with an SQLite file prepared by a script (`.brewery/db.json` written by hand for that data source); Database panel shows the data source and, when opened via its toggle action at start-up if possible, the tree. Ask the user to check the dialog, console and grid interactively.
- [ ] Commit `docs: Database tool window`.
