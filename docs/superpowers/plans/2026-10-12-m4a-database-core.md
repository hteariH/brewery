# Brewery M4a — Database Core (daemon) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** `brewery-daemon` can store database connections, keep their passwords in the OS secret store, test a connection, browse a schema tree lazily, run SQL with paged results, and cancel a running statement — for PostgreSQL, MySQL/MariaDB, SQLite and any JDBC driver the user adds.

**Architecture:** Spec section 8 "Databases". A `db` package in the daemon: `ConnectionStore` (`.brewery/db.json` per project, no secrets) + `SecretStore` (java-keyring: Windows Credential Manager / libsecret / macOS Keychain; an in-memory fake for tests), `DriverRegistry` (bundled PostgreSQL, MariaDB Connector/J for MySQL and MariaDB, xerial SQLite; user JARs each in their own `URLClassLoader`), `Session` per connection (one dedicated thread + one `java.sql.Connection`), `Introspector` (`DatabaseMetaData` base + small dialect overrides), `CursorManager` (statement + result set per cursor id, `fetch(offset, limit)`, `close`, `cancel` via `Statement.cancel()`). UI comes in M4b, SQL completion in M4c.

**Tech Stack:** Kotlin; `org.postgresql:postgresql:42.7.14` (BSD-2), `org.mariadb.jdbc:mariadb-java-client:3.5.10` (LGPL-2.1 — used unmodified as a separate jar inside the fat jar; notices must state the licence; MySQL Connector/J is NOT used because of its GPL licence), `org.xerial:sqlite-jdbc:3.53.4.0` (Apache 2.0), `com.github.javakeyring:java-keyring:1.0.4` (BSD-3); tests: Testcontainers 2.x PostgreSQL and MariaDB modules (skipped automatically when Docker is unavailable; CI ubuntu runs them).

**Spec:** `docs/superpowers/specs/2026-10-10-brewery-design.md` (sections 6 "Methods" db/*, 8 "Databases", 9 testing).

## Global Constraints

- Protocol (JSON-RPC, camelCase; enums by name; fixtures in `brewery-daemon/protocol/fixtures/db-*.json`):
  - `db/drivers {}` → `[DriverInfo { id, name, bundled: Boolean, jarPaths: [String] }]`; bundled ids `postgresql`, `mysql`, `mariadb`, `sqlite`.
  - `db/connections/list {}` → `[ConnectionInfo { id, name, driverId, url, user: String?, hasPassword: Boolean, options: {String:String} }]`
  - `db/connections/save { connection: ConnectionInfo (id may be empty → new UUID), password: String? (null = keep, "" = remove) }` → `ConnectionInfo`
  - `db/connections/delete { id }` → null (also removes the secret)
  - `db/connections/test { connection, password? }` → `{ ok: Boolean, message: String, serverVersion: String? }` (never throws for connection failures)
  - `db/drivers/addJar { driverId: String, name: String, className: String, jarPaths: [String] }` → `DriverInfo` (user drivers persisted in the project's `.brewery/db.json`)
  - `db/introspect { connectionId, path: [String] }` → `[DbNode { name, kind, path: [String], hasChildren: Boolean, detail: String? }]`, `kind` ∈ `catalog | schema | table | view | column | index | foreignKey | sequence | function | folder`; path `[]` lists catalogs or schemas (dialect-dependent), a table's children are folders `columns`, `indexes`, `foreignKeys`; column `detail` = `"<type> NULL|NOT NULL [PK]"`.
  - `db/execute { connectionId, sql }` → `{ cursorId: String?, columns: [ColumnInfo { name, type }], updateCount: Long?, elapsedMs: Long }` (cursorId null for non-query statements).
  - `db/fetch { cursorId, offset: Int, limit: Int }` → `{ rows: [[String?]], done: Boolean }` — values rendered to strings in the daemon (null stays null; binary → `0x…` hex prefix, first 64 bytes; long text truncated at 10 000 chars with `…`).
  - `db/cancel { cursorId? , connectionId }` → null (cancels the running statement of that session).
  - `db/close { cursorId }` → null.
- Domain error codes: -32010 `DB_CONNECTION_FAILED`, -32011 `DB_AUTH_FAILED` (SQLState `28*`), -32012 `DB_QUERY_FAILED` (message = driver message + SQLState), -32013 `DB_NOT_FOUND` (unknown connection/cursor id).
- `.brewery/db.json` shape: `{ "connections": [ConnectionInfo without hasPassword], "drivers": [user DriverInfo] }`, LF, pretty-printed, stable order; never contains a password.
- Secret key in the OS store: service `brewery-db`, account `<projectRoot canonical path>#<connectionId>`.
- Each session: one thread, statements serialised; `execute` returns as soon as the statement has produced its first result (rows are pulled by `fetch`); idle sessions closed after 10 minutes; cursor result sets kept until `close` or session close; max 20 open cursors per session (oldest closed).
- Never log SQL text or passwords to stdout; stdout stays JSON-RPC only.
- LF only; Apache 2.0; `thirdPartyNotices` must list every new jar with its licence.

### Task 1: Connections, secrets, drivers
**Files:** `brewery-daemon/src/main/kotlin/dev/brewery/daemon/db/{ConnectionStore.kt, SecretStore.kt, DriverRegistry.kt, DbProtocol.kt}`, `Protocol.kt`/`BreweryDaemon.kt` wiring, `build.gradle.kts`, tests.
- [ ] TDD: store round-trip (save/list/delete, id assignment, stable JSON, no password on disk), secret store fake (save/keep/remove semantics), driver registry (bundled ids load their `java.sql.Driver` class; a user JAR built in the test with a trivial `Driver` implementation is loaded in its own classloader and registered), `db/connections/test` against an SQLite temp file (ok) and a bad URL (ok=false, message), error codes over a Launcher pair.
- [ ] Real `SecretStore` via java-keyring behind the same interface; if the platform keyring is unavailable, fall back to "not stored" with `hasPassword=false` and a warning in the `test`/`save` result message — never crash.
- [ ] Commit `feat(daemon): database connections, secrets and drivers`.

### Task 2: Sessions, introspection
**Files:** `db/{Session.kt, Introspector.kt, Dialects.kt}`, tests (SQLite always; PostgreSQL/MariaDB via Testcontainers, `@EnabledIf` Docker available).
- [ ] TDD: SQLite schema with two tables, a view, an index and a foreign key → `introspect []` then table → folders → columns/indexes/foreignKeys with the `detail` format; PostgreSQL: schemas (excluding `pg_catalog`, `information_schema` unless asked), sequences under a schema; MariaDB/MySQL: catalogs as databases; session thread reuse; idle close.
- [ ] Commit `feat(daemon): lazy database schema introspection`.

### Task 3: Execute, fetch, cancel
**Files:** `db/{CursorManager.kt, ValueRenderer.kt}`, tests.
- [ ] TDD: `execute` select → columns + cursor; `fetch` pages (offset/limit, `done`); update statement → `updateCount`, no cursor; value rendering (null, binary hex, long text truncation, timestamps ISO-8601); `cancel` stops a long query (SQLite: a recursive CTE that runs for seconds; PG: `pg_sleep(30)`) and the next execute works; cursor limit (21st closes the oldest); `close`; query error → -32012 with SQLState in the message.
- [ ] Commit `feat(daemon): run SQL with paged results and cancellation`.

### Task 4: CI and notices
- [ ] Daemon CI job: Docker is available on ubuntu runners — Testcontainers tests run there; on Windows runners they skip. Make sure `NoticesTest` covers the four new jars; README section "Databases (daemon)" in `brewery-daemon/README.md` (protocol summary, secret storage, licences of drivers).
- [ ] Commit `ci(daemon): database tests and notices`.
