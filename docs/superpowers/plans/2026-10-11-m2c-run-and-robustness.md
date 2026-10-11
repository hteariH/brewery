# Brewery M2c — Run on Windows and Robustness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Gutter ▶ run/test actions for Java work on Windows, the daemon is restarted when it dies, Kotlin noise is gone, and the M2b deferred items are closed.

**Architecture:** The Java extension's run/test tasks (`zed-extensions/java` `languages/java/tasks.json`, tags `java-main`, `java-test-method`, `java-test-class`, …) run with `shell: { program: "/bin/sh", args: ["-c"] }` and POSIX path logic (`${f#$p/}`, `dirname`). On Windows Brewery maps `/bin/sh` and `/bin/bash` to Git for Windows' `sh.exe`/`bash.exe` and converts the paths the script sees (`cwd`, `ZED_*` path variables) to MSYS form (`C:\a\b` → `/c/a/b`), so the extension's scripts run unchanged. Run configurations for 0.1 stay Zed tasks (`.zed/tasks.json` + language tasks); the Run widget's "Run…" already opens their picker (Ruling: `.brewery/run.json` deferred — Zed tasks already persist per project).

**Spec:** `docs/superpowers/specs/2026-10-10-brewery-design.md` (sections 6, 8, 11 M2).

## Global Constraints
- Edits in Zed files carry `brewery:` markers; LF only; no unwrap/expect outside tests.
- Daemon: Apache 2.0, package `dev.brewery.daemon`, JDK 21.
- Restart policy (spec §6): restart a dead daemon at most 3 times per 5 minutes, then stop and show a notification-style message in the panel with the log hint.

### Task 1: POSIX-shell tasks on Windows

**Files:** find where a task's `Shell::WithArguments { program, args }` becomes the spawned command on Windows (start at `crates/task/src/task_template.rs` `resolve_task`, `crates/util/src/shell.rs`, `crates/terminal/src/terminal.rs` / `crates/project/src/terminals.rs`); new pure module `crates/util/src/brewery_msys.rs` (new Brewery file in Zed crate; one marked `mod`/`pub mod` line in `util.rs`).

**Interfaces (pure, tested):**
- `pub fn to_msys_path(path: &str) -> Option<String>`: `C:\Users\a b\x` → `/c/Users/a b/x`; `D:/x` → `/d/x`; non-drive paths → None.
- `pub fn find_git_posix_shell(program: &str, candidates: &[PathBuf]) -> Option<PathBuf>`: for `program` in `["/bin/sh", "/bin/bash", "sh", "bash"]` returns the first existing `sh.exe`/`bash.exe` among candidates; the real candidate list (only on Windows): `%ProgramFiles%\Git\bin`, `%ProgramFiles%\Git\usr\bin`, `%LocalAppData%\Programs\Git\bin`, and the dir of `git.exe` on PATH → `..\bin`.

**Behaviour (Windows only, `cfg(windows)` at the hook):** when a task's shell program is `/bin/sh` or `/bin/bash` and a Git shell is found: replace the program with that path; convert `cwd` passed to the shell to MSYS form via env `PWD`-compatible handling (keep the process cwd as the Windows path — Git's sh derives `$PWD` from it as `/c/...` automatically; verify) and convert every env var whose name starts with `ZED_` and whose value is an absolute Windows path (`ZED_FILE`, `ZED_DIRNAME`, `ZED_WORKTREE_ROOT`, `ZED_RELATIVE_FILE` stays relative but switch `\` to `/`) to MSYS form. If no Git shell is found, keep Zed's behaviour and log a warning once ("Install Git for Windows to run Java gutter tasks").

- [ ] Tests first for `to_msys_path`, `find_git_posix_shell` (temp dirs), and an env-conversion helper `fn msys_env(env: &mut HashMap<String, String>)`.
- [ ] Implement + hook; `cargo test -p util`, the crate(s) touched, clippy, `cargo check -p zed`.
- [ ] Commit `feat: run POSIX-shell tasks through Git for Windows`.

### Task 2: Restart a dead daemon

**Files:** `crates/brewery_daemon_client` (exit detection), `crates/brewery_build` (restart policy + UI state).

- [ ] Client: expose process exit as an event — `lsp::LanguageServer` exposes server termination via its io task / `on_io` or the stdout reader ending; find the cleanest signal (read `crates/lsp/src/lsp.rs`; if there is none, have `DaemonClient` own a watchdog that awaits the server's output-reader task or periodically checks `process.try_status()` every 2 s on the background executor). Produce `pub fn on_exit(&self, f: impl FnOnce() + Send + 'static)`.
- [ ] Panel: pure `struct RestartPolicy { max: usize = 3, window: Duration = 5 min, history: VecDeque<Instant> }` with `fn allow(&mut self, now: Instant) -> bool`; on daemon exit (not caused by our own shutdown) restart if allowed, else show "The Brewery daemon stopped repeatedly. Check the log (Help → Open Log) and press Reload." Tests for `RestartPolicy` and a gpui test with a fake daemon that exits.
- [ ] Commit `feat: restart the daemon when it dies`.

### Task 3: Kotlin noise

- [ ] Find out why the `kotlin` extension was installed on a clean profile without user action (search `crates/extension_host` / `extensions_ui` for automatic installs of suggested extensions; check whether the Java extension or Zed auto-installs for `.kts`). Report the cause.
- [ ] `.kts` is covered by the Java extension's bundled Kotlin grammar ("Gradle KTS"); make Brewery not suggest/install the `kotlin` extension for `.kts` (edit the suggestion table in `crates/extension_suggest/src/extension_suggest.rs`: `("kotlin", &["kt"])`, marked) and, if the cause is an automatic install, stop it for `kotlin` specifically (marked hook) or generally when the user hasn't confirmed.
- [ ] Zed opened several "Language server kotlin-lsp" log tabs when the server failed to download. Find where a failing language server opens log/error buffers automatically and limit it to one tab per server (or none — a notification is enough), marked. Test where practical.
- [ ] Commit `fix: no Kotlin extension noise for Gradle KTS files`.

### Task 4: M2b deferred items

- [ ] Daemon: trailing-edge debounce (notify 500 ms after the *last* event); `openProject` cancels a pending notification; queued import for an old root is skipped when the root changed (compare root at run time).
- [ ] Shadow jar third-party notices: include `META-INF/LICENSE*`/`NOTICE*` of dependencies (shadow `mergeServiceFiles()` already; add the shadow plugin's license/notice merging transformers, or generate `THIRD-PARTY-NOTICES.txt` from dependency POMs), checked by a test that the jar contains a notices file.
- [ ] CI: Linux-only step running `cargo test -p brewery_daemon_client` with `BREWERY_DAEMON_JAR` pointing at a jar built in the same job (or downloaded artifact from the `daemon` job via `needs`); Gradle cache in the daemon job (`actions/setup-java` `cache: gradle`).
- [ ] Commit `chore(daemon): debounce, notices, CI integration test`.

### Task 5: On-screen check (controller)
- [ ] Windows, clean profile: open a Gradle project with a `main` class and a JUnit test; gutter ▶ on `main` runs it via Gradle in the terminal; gutter ▶ on a test method runs that test.
