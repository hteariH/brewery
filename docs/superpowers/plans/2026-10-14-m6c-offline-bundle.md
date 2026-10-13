# Brewery M6c — Offline Bundle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A packaged Brewery works for Java on a fresh machine without downloads: a bundled JRE (jlink) for the daemon and jdtls, the daemon jar, jdtls with a Brewery launcher, Lombok, java-debug, java-lsp-proxy and the Java extension itself, all pinned and checksum-verified at bundle time.

**Architecture:** Gap analysis `docs/superpowers/plans/m6-gap-analysis.md` sections 4.2, 4.3, 4.4 and "M6c". A pinned list `script/brewery/bundle-deps.toml` + one Python fetcher `script/brewery/fetch_bundle_deps.py` (stdlib only, runs on Windows/Linux/macOS CI) lays out `bundle/<os>-<arch>/` = `jre/`, `jdtls/` (+ `brewery-jdtls` / `brewery-jdtls.cmd` launchers), `lombok.jar`, `java-debug.jar`, `java-lsp-proxy(.exe)`, `extensions/java/`, `brewery-daemon/brewery-daemon.jar`, `licenses/`. At runtime a Brewery hook seeds `extensions/java` into Zed's installed-extensions dir if absent, and a computed default-settings overlay points the Java extension's override settings at the bundled files (empty overlay without a bundle, so dev builds behave as today). `brewery_bundle` (M6a) provides the paths; the daemon already prefers the bundled JRE.

**Tech Stack:** Python 3 (stdlib) scripts, jlink (Temurin JDK 21), shell/cmd launchers, Rust (startup hook + settings overlay), GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-10-10-brewery-design.md` (bundled JRE for internal servers only; M6).

## Global Constraints

- `bundle-deps.toml`: every artifact with exact version, URL and SHA-256 per OS/arch (Temurin JDK 21 latest GA for windows-x64, linux-x64, linux-aarch64, macos-aarch64, macos-x64 from the Adoptium API; jdtls milestone tarball; Lombok; java-debug 0.53.x as used by the extension; java-lsp-proxy per platform from the zed-extensions/java release matching the pinned extension tag; the Java extension at the tag Brewery auto-installs today — find the exact version in `~/.local/share/brewery` / `%LOCALAPPDATA%\Brewery\extensions\installed\java\extension.toml` or the gallery). The fetcher refuses any mismatch, caches downloads under `~/.cache/brewery-bundle` (or `--cache`), and is idempotent; `--os`, `--arch`, `--out` arguments.
- jlink module list as in the gap analysis 4.2 (start: `java.se, jdk.unsupported, jdk.crypto.ec, jdk.zipfs, jdk.net, jdk.management, jdk.jdi, jdk.attach, jdk.jdwp.agent, java.instrument, jdk.localedata, jdk.charsets, jdk.jfr, jdk.httpserver, jdk.compiler`), `--strip-debug --no-man-pages --no-header-files --compress=zip-6`; `jre/legal/` kept; `licenses/` gets JRE (GPLv2+CE, Adoptium source URL for the exact tag), jdtls (EPL-2.0), Lombok (MIT), java-debug (EPL-1.0), java-lsp-proxy + extension (Apache-2.0), daemon THIRD-PARTY-NOTICES.txt. jlink must run on the target OS (CI matrix), not cross.
- jdtls launcher: before writing it, read the pinned extension's `src/jdtls.rs` (`build_jdtls_launch_args`, `get_jdtls_launcher_from_path`) to match the args the extension passes (`-data`, `--jvm-arg=...`, lombok javaagent) and keep debugging via java-debug working; config area writable (user cache dir), shared config read-only from the bundle; no Python.
- Runtime (Rust, marked `// brewery:` in Zed files, logic in `brewery_bundle` or a new `brewery_bundle_setup` module): (1) seed `extensions/java` into `paths::extensions_dir()/installed/java` only if absent, before `extension_host` loads the index; (2) default-settings overlay only when the bundle exists: `lsp.jdtls.settings.java_home = <root>/jre`, `jdk_auto_download = false`, `jdtls_launcher`, `lombok_jar`, `java_debug_jar`, `lsp_proxy_path`, `check_updates = "once"`; user settings still win. Verify the setting names against the pinned extension's README/source.
- Daemon jar version tied to BREWERY_VERSION (`./gradlew -Pversion=$(cat crates/zed/BREWERY_VERSION) shadowJar`).
- CI: a `bundle` job (matrix windows/linux; macOS optional/continue-on-error) runs the fetcher on tags and dispatch, uploads `bundle-<os>-<arch>` artifacts; a Linux offline smoke step: with `--network none` (docker) or `unshare -n`, start the daemon e2e test with the bundled JRE + jar, and start jdtls through the launcher against a small Maven/Gradle fixture expecting an LSP `initialize` response.
- Bundle size target ≈ 300 MB with the app; report the measured size.

### Task 1: Pinned deps and fetcher
**Files:** `script/brewery/bundle-deps.toml`, `script/brewery/fetch_bundle_deps.py`, `script/brewery/test_fetch_bundle_deps.py`.
- [ ] TDD (Python unittest): toml parsing, per-OS/arch selection, checksum mismatch → failure and no partial output, cache hit skips download (local HTTP server fixture), layout of `--out`.
- [ ] Fill real versions + SHA-256 (compute by downloading once; record the source URLs).
- [ ] Commit `build: pinned offline bundle dependencies`.

### Task 2: jlink JRE and jdtls launcher
**Files:** fetcher (jlink step), `script/brewery/jdtls/brewery-jdtls`, `script/brewery/jdtls/brewery-jdtls.cmd`, licences collection.
- [ ] Build locally on Windows: `jre/bin/java -version`; daemon starts with it (`BREWERY_DAEMON_JAVA` unset, bundle root pointed at the output); jdtls through the launcher answers `initialize` for `brewery-daemon/src/test/resources/projects/` fixture (write a small Python LSP client in the test script). Measure sizes.
- [ ] Commit `build: bundled JRE and jdtls launcher`.

### Task 3: Extension seed and settings overlay
**Files:** `crates/brewery_bundle/src/...` (or a new module), `crates/zed/src/main.rs` hook (marked), settings overlay hook (find how Brewery merges defaults today: `assets/settings/default.json` + `crates/settings/src/brewery_defaults.rs`; compute JSON at startup and layer it as defaults).
- [ ] TDD: overlay empty without a bundle; with a temp bundle the overlay contains the absolute paths; user setting overrides overlay; seeding copies only when absent and is idempotent; a broken bundle dir (missing pieces) yields a partial overlay only for present pieces.
- [ ] Commit `feat: use the bundled Java tools`.

### Task 4: CI bundle job and offline smoke
**Files:** `.github/workflows/brewery_ci.yml` (or a new `brewery_bundle.yml`), FORK.md.
- [ ] Bundle job + Linux offline smoke per Global Constraints; FORK.md "Offline bundle" section (how to build it locally, how to bump pins).
- [ ] Commit `ci: offline bundle and smoke test`.
