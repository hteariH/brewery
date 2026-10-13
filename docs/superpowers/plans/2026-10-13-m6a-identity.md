# Brewery M6a — Release Identity and Plumbing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Everything a release build shows or looks up says Brewery: its own version (separate from Zed's internal version), Windows exe metadata, CLI and app names and fallbacks, and one bundle-root helper that finds the bundled JRE, daemon jar, jdtls and seeded extensions in Windows, Linux (`libexec/`) and macOS layouts.

**Architecture:** Gap analysis `docs/superpowers/plans/m6-gap-analysis.md` sections 1.1, 4.1, 4.2 (lookup part), 6, 7 and "M6a". `brewery_core::BREWERY_VERSION` (from `crates/zed/BREWERY_VERSION` via a build script, default `0.1.0-dev`); Zed's `AppVersion`/`CARGO_PKG_VERSION` stays for remote server, extensions and LSP client info. New leaf crate `brewery_bundle` (no workspace deps, like `brewery_cloud`) with pure path logic and `from_exe(&Path)` test variants; `brewery_daemon_client::jdk` uses it so a bundled JRE beats `JAVA_HOME`/PATH and the jar lookup works in all layouts.

**Tech Stack:** Rust; `windows_resources` build helper; Zed `cli`, `util`, `install_cli`, `zed` crates.

**Spec:** `docs/superpowers/specs/2026-10-10-brewery-design.md` (release/packaging sections, M6).

## Global Constraints

- Zed-file edits marked `// brewery:` and minimal: `crates/windows_resources/src/windows_resources.rs` (product names = `Brewery`, `Brewery Preview`, `Brewery Nightly`, `Brewery Dev`; `CompanyName "Brewery contributors"`; `LegalCopyright "Copyright 2026 Brewery contributors. Portions copyright Zed Industries, Inc."`; FILEVERSION/PRODUCTVERSION from `BREWERY_VERSION` when set), `crates/zed/src/zed.rs` About text (shows "Brewery <BREWERY_VERSION>" and, smaller, "based on Zed <zed version>"), CLI/app names per the gap analysis section 7 table (clap names `brewery`, version strings `Brewery {}`, socket `brewery-{}.sock`, Linux lookup `../libexec/brewery-editor`, `./brewery`; Windows `../Brewery.exe`, `./brewery.exe`; util app→CLI `bin/brewery.exe`, `../bin/brewery`; macOS install path `/usr/local/bin/brewery`); the cargo binary `cli` keeps its name (packages rename it). Flatpak module untouched.
- `crates/zed/BREWERY_VERSION`: one line, semver, `0.1.0` for the release (the workflow in M6e asserts it equals the tag). `brewery_core::BREWERY_VERSION: &str` and `brewery_core::brewery_version() -> semver::Version` (or the workspace's semver type).
- `brewery_bundle` API: `bundle_root()`, `bundled_jre_home()` (`<root>/jre` when `<root>/jre/release` exists), `bundled_daemon_jar()` (`<root>/brewery-daemon/brewery-daemon.jar`), `bundled_jdtls_home()` (`<root>/jdtls`), `bundled_extension(id)` (`<root>/extensions/<id>`), each with a `*_from_exe(&Path)` variant. Root = exe dir; its parent when the exe dir is named `libexec` (Linux `/opt/brewery/libexec/brewery-editor`); `../Resources` when the exe dir is `MacOS` inside an `.app`.
- Java lookup order for the daemon (`brewery_daemon_client::jdk`): bundled JRE → existing order (JAVA_HOME, PATH, known JDK dirs) — keep the JDK 21+ requirement; jar lookup: `BREWERY_DAEMON_JAR` env → bundled → dev path (`brewery-daemon/build/libs`). Existing tests keep passing.
- LF only; no unwrap/expect outside tests; CI hook-in test lists get `-p brewery_bundle` (and `-p cli` if its tests are cheap; say so).

### Task 1: Brewery version and exe metadata
**Files:** `crates/zed/BREWERY_VERSION`, `crates/brewery_core/{build.rs, src/...}`, `crates/windows_resources/src/windows_resources.rs`, `crates/zed/src/zed.rs` (About), tests.
- [ ] TDD: `BREWERY_VERSION` parses as semver and equals the file content; About text helper (pure fn) produces "Brewery 0.1.0" + "based on Zed 1.25.0"; on Windows CI the built exe's `VersionInfo.ProductName` is "Brewery Dev" for a dev build — add a CI step in the Windows check or build job that prints and asserts it (`(Get-Item target\...\brewery.exe).VersionInfo`).
- [ ] Commit `feat: Brewery version and Windows exe metadata`.

### Task 2: CLI and app names
**Files:** `crates/cli/src/main.rs`, `crates/util/src/util.rs`, `crates/install_cli/src/install_cli_binary.rs`, `crates/zed/src/main.rs`, `crates/zed/resources/windows/brewery.sh` (copy of zed.sh calling Brewery.exe), tests.
- [ ] Extract the candidate path lists into consts/fns and unit-test them (contain the Brewery names in the right order; no `zed` names left except the untouched flatpak module); `--version` string test.
- [ ] Commit `feat: Brewery names for the CLI and app lookup`.

### Task 3: Bundle root and lookups
**Files:** `crates/brewery_bundle/` (new), `crates/brewery_daemon_client/src/jdk.rs` (+ tests), Cargo files, CI lists, FORK.md.
- [ ] TDD with temp dirs: Windows flat layout, Linux libexec layout, macOS `.app` layout; bundled JRE preferred over JAVA_HOME (inject env); jar lookup order; missing pieces → None.
- [ ] Commit `feat: find bundled Java, daemon and jdtls`.

### Task 4: CI and docs
- [ ] CI hook-in lists and FORK.md: `-p brewery_bundle` (+ `-p cli` if added). README: nothing user-facing yet beyond the version in About; FORK.md: "Brewery version vs Zed version" paragraph.
- [ ] Commit `docs: Brewery version and bundle layout`.
