# Brewery M5b — Log Tool Window Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** IntelliJ's **Log** tool window: a bottom-dock panel (Alt+9) showing Zed's commit graph with lanes and commit details, plus a filter bar (branches, user, date, paths, text) that filters the list, and good behaviour on 100k-commit repositories.

**Architecture:** Gap analysis `docs/superpowers/plans/m5-gap-analysis.md` section 2 "Log tool window" and "M5b". `brewery_git::LogPanel` (`impl Panel`, bottom dock) hosts `git_ui::git_graph::GitGraph` entities as tabs (the Log tab now; file-history tabs come in M5c) and renders the active one as a plain view. Filters map to one new `git::repository::LogSource::Args(Arc<[SharedString]>)` variant passed verbatim to `git log`; a pure `LogFilter::to_git_args()` builds the args; applying a filter recreates the hosted `GitGraph` with `Some(LogSource::Args(..))` and drops the previous graph-data cache entry through a small public `Repository::forget_graph_data`.

**Tech Stack:** Rust, GPUI, Zed `git`, `project`, `git_ui`, `workspace`, `ui` crates.

**Spec:** `docs/superpowers/specs/2026-10-10-brewery-design.md` (section 7a "Log", M5).

## Global Constraints

- Zed-file edits only those listed in the gap analysis "Edits to Zed files" table for M5b, each marked `// brewery:` and a few lines: `crates/git/src/repository.rs` (`LogSource::Args` + `get_args` arm), `crates/project/src/git_store.rs` (`log_source_to_proto` arm → error "local only"; `pub fn forget_graph_data(source, order)`), `crates/git_ui/src/git_graph.rs` (serialisation arms for `Args` → treated as `All`; `pub fn selected_commit_sha(&self) -> Option<Oid>` accessor), `crates/zed/src/zed.rs` (load the panel next to the other Brewery panels), JetBrains keymaps (`alt-9` → `brewery::ToggleLogPanel`, macOS `cmd-9`). Rely only on `GitGraph::new`, `select_commit_by_sha` and the new accessor.
- Panel: persistent name `"Log Panel"`, default position Bottom, icon `IconName::GitBranch`-family (pick the closest "history/graph" icon available), tooltip "Log", action `brewery::ToggleLogPanel`; tab strip only when more than one tab; repository = the active repository of the project (follow repository changes; empty state "No Git repository" when none).
- `LogFilter { branches: Vec<String>, authors: Vec<String>, since: Option<DateBound>, until: Option<DateBound>, paths: Vec<RepoPath>, text: Option<String> }`; `to_git_args()`: branches → the ref names (empty → `--branches --remotes --tags HEAD`, matching `LogSource::All`), each author `--author=<pattern>` (regex-escaped, case-insensitive with `-i`/`--regexp-ignore-case`), dates `--since=`/`--until=` in ISO 8601, text → `--grep=<text> --fixed-strings --regexp-ignore-case` (a full 7–40 hex string also matches by hash prefix: run as a separate "go to commit" instead of grep), paths → `--parents -- <paths>`; every user value passed as a single argv element, never starting with `-` for ref names (refuse), no shell.
- Filter bar (one row above the graph): Branch (multi-select popover of local+remote branches with search), User (popover: "me" = `user.name` from git config, plus authors seen in the loaded commits, plus free text), Date (presets: Last 24 hours, Last 7 days, Last 30 days, plus "Since…" text input accepting YYYY-MM-DD), Paths (popover: project-relative paths, free text, one per line), a text field "Text or hash" (applies on Enter), and a Reset button when any filter is set. Applying a filter shows a "Filtered" chip and the commit count when loading finishes.
- Filtered graphs: with author/date/text filters, commits whose parents are filtered out render without connecting edges (accept GitGraph's behaviour; no crash, no panic in lane assignment — test it).
- Cache: replacing a filter calls `forget_graph_data` for the previous `(LogSource::Args, order)`; at most the current filtered graph plus the unfiltered one stay cached.
- Remote/collab projects: the Log panel works unfiltered (GitGraph proxies), filters disabled with a tooltip "Filters work in local projects".
- Tests: pure `to_git_args` table; gpui tests on temp repos (real `git`, `core.autocrlf=false`, isolated global config as in `brewery_git` tests).

### Task 1: Log panel
**Files:** `crates/brewery_git/src/{log_panel.rs, log_panel_tests.rs}`, `brewery_git.rs` (register), `crates/git_ui/src/git_graph.rs` (accessor only), `crates/zed/src/zed.rs`, JetBrains keymaps, `crates/brewery_git/Cargo.toml` (git_ui dep).
- [ ] TDD: the panel registers at the bottom with the right name/icon/action; on a temp repo with 3 commits the hosted graph loads them (assert via GitGraph's public API or the accessor after selecting); `ToggleLogPanel` opens/focuses it; `OpenAtCommit`-style selection by sha works through the panel; no repository → empty state.
- [ ] `cargo test -p brewery_git`, clippy, `cargo check -p zed`.
- [ ] Commit `feat: Log tool window`.

### Task 2: Filters
**Files:** `crates/git/src/repository.rs`, `crates/project/src/git_store.rs`, `crates/git_ui/src/git_graph.rs` (arms), `crates/brewery_git/src/{log_filter.rs, log_filter_tests.rs, log_filter_bar.rs}`, `log_panel.rs`.
- [ ] TDD `to_git_args` (every field, combinations, escaping, refusing `-`-prefixed refs, hash detection).
- [ ] `LogSource::Args` end to end: on a temp repo with two authors, two branches, dated commits (set `GIT_AUTHOR_DATE`/`GIT_COMMITTER_DATE`) and two paths, each filter yields only the matching commits; author/date filters don't panic lane assignment; `forget_graph_data` drops the old entry.
- [ ] Filter bar UI + gpui test: setting User = one author re-creates the graph with the filtered source and shows the count; Reset restores the unfiltered graph.
- [ ] `cargo test -p brewery_git -p git -p project --lib` (only the new tests if the full suites are too slow; say which ran), clippy, `cargo check -p zed`.
- [ ] Commit `feat: filters in the Log tool window`.

### Task 3: Scale check
**Files:** `crates/brewery_git/src/log_scale_tests.rs` (`#[ignore]` by default, run explicitly), `docs/brewery/FORK.md` or `README.md` note, possible small fixes.
- [ ] Generate a 100k-commit repo with `git fast-import` in a temp dir (a few branches/merges); measure time to the first chunk and to full load through the Log panel's graph, and that scrolling a visible range stays cheap (no per-frame O(n) work — inspect and, if needed, profile `GraphData::add_commits` and the canvas range code). Record the numbers in this plan file under "Measurements".
- [ ] If `core.commitGraph` data is missing and the first chunk takes > 1 s, show a one-time notice in the Log panel offering `git commit-graph write --reachable` (run via GitCli on click).
- [ ] Commit `perf: Log tool window on large repositories`.

### Task 4: CI, docs, on-screen check
- [ ] README "Git": Log tool window paragraph. CI lists already include `brewery_git`; add `-p git` / `-p project` only if new tests live there and run fast enough (else document how to run them).
- [ ] Controller: `[win]` build; open the Brewery repo itself; Alt+9 shows the Log; set a User filter (computer-use only if the user grants access, else ask the user).
- [ ] Commit `docs: Log tool window`.

## Measurements

(filled in by Task 3)
