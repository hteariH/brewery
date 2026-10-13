# Brewery M4c — SQL Completion in the Query Console Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** In a query console, typing SQL offers IntelliJ-style completion: keywords, schemas, tables and views of the console's data source, columns of the tables in the statement (also after `alias.`), with the type as detail.

**Architecture:** Spec section 5 "SQL in the editor": the daemon sends the cached schema, the UI parses SQL for the completion context. The query console's buffer is in memory, so no language server is involved:
- Daemon: `db/schema { connectionId, refresh: Boolean? }` returns a flattened snapshot (schemas → tables → columns) built with the M4a `Introspector`, cached per connection, invalidated by `refresh`, by DDL run through `db/execute` (leading keyword `CREATE`/`ALTER`/`DROP`/`RENAME`/`TRUNCATE` per `SqlText`), and when the connection is saved or deleted.
- Rust (`brewery_db`): a `SchemaCache` per data source (fetched lazily on the first completion request, refreshed by the panel's Refresh and after a DDL run in a console), a pure `sql_context` analyser (tokeniser reused from `statements.rs`), and an `editor::CompletionProvider` installed on the console editor with `set_completion_provider`.

**Tech Stack:** Kotlin (daemon), Rust/GPUI (`editor::CompletionProvider`, `project::Completion`).

**Spec:** `docs/superpowers/specs/2026-10-10-brewery-design.md` (sections 5, 8, 11 M4).

## Global Constraints

- Protocol: `db/schema { connectionId: String, refresh: Boolean? }` → `DbSchema { schemas: [SchemaInfo { name: String (empty for SQLite / no-schema databases), tables: [TableInfo { name, kind: "table"|"view", columns: [ColumnInfo { name, type }] }] }], truncated: Boolean }`; fixture `brewery-daemon/protocol/fixtures/db-schema.result.json`; at most 5000 tables in total (`truncated: true` beyond that); system schemas excluded (same rules as introspection); errors as M4a (-32010/-32011/-32013).
- The daemon builds the snapshot on the session thread with metadata calls only (no SQL logged). PostgreSQL/MariaDB: one `getColumns` call per schema/catalog (not per table) where the driver allows it.
- Completion context (pure fn `context_at(sql: &str, offset: usize) -> SqlContext`), for the statement under the cursor only:
  - after `FROM`, `JOIN`, `INTO`, `UPDATE`, `TABLE`, `DESCRIBE` → tables/views (+ schemas; after `schema.` → tables of that schema);
  - after `<alias or table>.` → columns of the table the alias refers to (aliases from `FROM t a`, `FROM t AS a`, `JOIN s.t a`, comma-separated FROM lists; also the bare table name);
  - after `SELECT`, `WHERE`, `AND`, `OR`, `ON`, `BY`, `SET`, `HAVING`, `,` in a select list, `(` → columns of all tables referenced in the statement (also those after the cursor, e.g. `SELECT | FROM owners`) + keywords + functions out of scope;
  - inside a string literal or comment → no completion;
  - otherwise → keywords.
- Matching: case-insensitive prefix plus Zed's fuzzy filtering of the menu; inserted identifiers are quoted only when needed (same quoting rules as M4b's double-click SELECT); keywords inserted in upper case if the typed prefix is upper case or empty, else lower case.
- Items: label = name, detail = type (columns) / "table" / "view" / "schema" / "keyword"; columns sorted before keywords in column contexts; sort tables of the current statement's FROM first.
- Completion triggers: identifier characters (Zed's default word trigger) and `.`.
- No network calls on the UI thread; the first request with no cache starts the fetch and returns keywords only, the menu refreshes when the schema arrives (or the next keystroke uses it).
- Zed-file edits marked `brewery:`; LF; no unwrap/expect outside tests.

### Task 1: Daemon schema snapshot
**Files:** `brewery-daemon/src/main/kotlin/dev/brewery/daemon/db/{SchemaSnapshot.kt}`, `Introspector.kt`/`DbService.kt`/`Protocol.kt`/`BreweryDaemon.kt` wiring, tests (SQLite always; PG/MariaDB container tests skipped without Docker), fixture.
- [ ] TDD: SQLite snapshot of the M4a fixture schema (tables, view, columns with types, schema name ""); cache hit returns the same snapshot without metadata calls (count calls via a wrapper or a test hook); `refresh: true` rebuilds; a `CREATE TABLE` through `db/execute` invalidates; save/delete of the connection invalidates; truncation at a small injected limit; PG: two schemas, system schemas excluded; MariaDB: catalogs as schemas.
- [ ] Commit `feat(daemon): schema snapshot for SQL completion`.

### Task 2: Completion in the console
**Files:** `crates/brewery_daemon_client/src/{protocol.rs, project_daemon.rs, brewery_daemon_client.rs}` (types + `db_schema`), `crates/brewery_db/src/{schema_cache.rs, sql_context.rs, completion.rs}`, `console.rs` (install provider; DDL run → refresh), `panel.rs` (Refresh → refresh cache), tests.
- [ ] Pure `sql_context` tests covering every rule above (aliases, `AS`, schema-qualified, comma FROM lists, cursor before FROM, strings/comments, multiple statements in the buffer).
- [ ] `SchemaCache` tests: lazy fetch once, concurrent requests share one fetch, refresh, error keeps keywords-only completion.
- [ ] gpui test with the fake daemon: typing `SELECT * FROM ow` in a console offers `owners`; `SELECT o. FROM owners o` offers its columns with types; accepting a completion inserts the (quoted when needed) name.
- [ ] `cargo test -p brewery_db -p brewery_daemon_client`, clippy, `cargo check -p zed`.
- [ ] Commit `feat: SQL completion in the query console`.

### Task 3: CI, docs, on-screen check
- [ ] README "Databases": completion paragraph. Daemon README protocol list: `db/schema`.
- [ ] Controller: CI release build, demo SQLite project; in a console type `select  from owners o` and `o.` and check the completion menus (computer-use allowed by the user for Brewery checks).
- [ ] Commit `docs: SQL completion`.
