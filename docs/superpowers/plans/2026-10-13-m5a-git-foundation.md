# Brewery M5a — Git Foundation, Commit Tool Window, Branches Popup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Lay the base for IntelliJ-style git in Brewery: a `brewery_git` crate that can run the git operations Zed's git layer lacks (merge, rebase, abort/continue, reflog, show at revision), the git panel presented as IntelliJ's **Commit** tool window, and an IntelliJ-style **Branches** popup from the title-bar branch button.

**Architecture:** Gap analysis `docs/superpowers/plans/m5-gap-analysis.md` (sections "What the git layer offers", 1 "Commit tool window", 6 "Branches popup", "Edits to Zed files"). Spec section 7a. New crate `crates/brewery_git`; git commands run through `project::git_store::Repository::send_job` (`RepositoryState::Local { backend, environment }`) so they are serialised with Zed's own git jobs; no changes to Zed's `GitRepository` trait. The branches popup is a new `Picker` delegate installed with `git_ui_core::set_branch_picker_builder`, which requires `brewery_git::init` to run after `git_ui::init` in `crates/zed/src/main.rs`.

**Tech Stack:** Rust, GPUI, Zed `git`, `project`, `git_ui_core`, `picker`, `ui`, `workspace` crates; the `git` executable found by Zed's git layer.

**Spec:** `docs/superpowers/specs/2026-10-10-brewery-design.md` (section 7a, M5).

## Global Constraints

- Zed-file edits marked `// brewery:`, each a few lines (see the gap analysis table); never edit the bodies of `git_panel.rs`, `branch_picker.rs` or `git_graph.rs` in M5a. LF only; no `unwrap`/`expect` outside tests; never read an entity while it is being updated.
- `GitCli` API (`crates/brewery_git/src/cli.rs`): `pub fn run(repo: &Entity<Repository>, args: Vec<String>, cx: &mut App) -> Task<Result<GitOutput>>` (`GitOutput { status: i32, stdout: String, stderr: String }`; non-zero status is not an `Err`, the caller decides), using the repository's git binary path and environment from `RepositoryState::Local`; remote/collab repositories → `Err("not supported for remote projects")`. Helpers on top, each returning a typed result: `merge(branch) -> MergeOutcome { Merged | UpToDate | Conflicts(Vec<RepoPath>) | Failed(String) }`, `rebase(onto) -> same outcome type`, `abort_merge`, `continue_merge`, `abort_rebase`, `continue_rebase`, `checkout_side(paths, Side::Ours|Theirs)`, `recent_branches(limit) -> Vec<String>` (from `git reflog` checkout entries, most recent first, deduplicated, only existing local branches), `show_at(rev, path) -> Option<String>`, `rev_list(range, limit) -> Vec<Oid>`, `in_progress() -> Option<Operation::{Merge, Rebase, CherryPick}>`.
- Every mutating helper sets `GIT_EDITOR=true` / `--no-edit` so git never opens an editor; never passes user strings unvalidated as options (refuse branch names starting with `-`).
- Commit tool window: the git panel's stripe button tooltip/title reads "Commit" with a commit icon (an IntelliJ icon from `brewery_theme` if one exists, else Zed's `IconName::GitBranch`-family fallback), done in `brewery_stripes.rs` with a persistent-name override table; JetBrains keymap `ctrl-k` (macOS `cmd-k`) focuses the commit message editor (`git_panel::FocusEditor`) while `alt-0` keeps toggling the panel.
- Branches popup layout, top to bottom: search field; actions **New Branch…**, **Checkout Tag or Revision…**, **Update Project** (pull), **Push…**; group headers **Recent** (up to 5, from `recent_branches`, current excluded), **Local**, **Remote** (grouped by remote name), each sorted by name except Recent; the current branch marked with a check/star; typing filters all groups (fuzzy, Zed's matcher). Enter on a branch → checkout (remote → create tracking branch, the existing Zed behaviour); `→`/right-click opens a submenu: Checkout, New Branch from '<b>'…, Checkout and Rebase onto '<current>', Rebase '<current>' onto '<b>', Merge '<b>' into '<current>', Compare with '<current>' (disabled with tooltip "Coming in a later update" until M5c), Rename… (local only), Delete (confirm prompt; force delete offered when not merged). Merge/rebase outcomes: success → toast; conflicts → toast "Merge has conflicts: N files" with an action that opens the git panel (M5d replaces it with the Conflicts dialog); failure → error toast with git's stderr first line.
- Keybinding `ctrl-shift-backtick` stays Zed's; JetBrains keymap `ctrl-~` (`git::Branch`) opens the Brewery popup through the same builder hook.
- Tests use real `git` in temp dirs (see `crates/git/src/repository.rs` tests for helpers) and gpui tests with `FakeFs`/`set_branch_list_for_test` where a real repo is not needed. CI hook-in list and FORK.md test list get `-p brewery_git`.

### Task 1: Crate and git CLI
**Files:** `crates/brewery_git/{Cargo.toml, LICENSE-GPL link/copy per Brewery convention, src/brewery_git.rs, src/cli.rs, src/cli_tests.rs}`, root `Cargo.toml` (workspace member + dep), `crates/zed/Cargo.toml`, `crates/zed/src/main.rs` (`brewery_git::init(cx)` after `git_ui::init`, marked).
- [ ] TDD each helper against temp repos: fast-forward merge, up-to-date, conflicting merge (MERGE_HEAD exists, conflicted paths listed), abort/continue merge, rebase with and without conflicts, abort/continue rebase, checkout_side ours/theirs on a conflicted file, recent_branches from a scripted checkout sequence (dedup, deleted branch skipped), show_at existing/missing path, rev_list range, in_progress detection, branch name starting with `-` refused.
- [ ] `run` through `Repository::send_job` (find how to obtain the git binary path + env; follow `RealGitRepository`'s command construction).
- [ ] `cargo test -p brewery_git`, clippy, `cargo check -p zed`.
- [ ] Commit `feat: brewery_git crate with merge, rebase and reflog helpers`.

### Task 2: Commit tool window
**Files:** `crates/workspace/src/dock/brewery_stripes.rs` (+ its tests), `assets/keymaps/linux/jetbrains.json`, `assets/keymaps/macos/jetbrains.json` (marked where comments are allowed), icon asset in `brewery_theme` if needed (IntelliJ commit icon, Apache 2.0, NOTICE updated).
- [ ] TDD: stripe summary for the git panel shows title "Commit" and the override icon; other panels unchanged; keymap test (follow the existing keymap tests in `crates/zed`) that `ctrl-k` resolves to `git_panel::FocusEditor` in the git panel context and `alt-0` still toggles.
- [ ] Commit `feat: the git panel is the Commit tool window`.

### Task 3: Branches popup
**Files:** `crates/brewery_git/src/{branches_popup.rs, branches_popup_tests.rs}`, `brewery_git.rs` (install the builder in `init`).
- [ ] Pure grouping/ordering fn from (branches, current, recent) → rows with headers; tests incl. filtering, remotes grouped by remote, current excluded from Recent.
- [ ] Picker delegate + submenu + actions per Global Constraints; outcomes surfaced as toasts.
- [ ] gpui tests: popup lists groups for a fake branch list; on a temp repo, Merge into current produces Merged / Conflicts toasts; Delete with confirm removes the branch; Rename.
- [ ] `cargo test -p brewery_git`, clippy, `cargo check -p zed`.
- [ ] Commit `feat: IntelliJ-style Branches popup`.

### Task 4: CI, docs, on-screen check
- [ ] CI hook-in tests (both check jobs) and FORK.md: `-p brewery_git`. README: "Git" section (Commit tool window, Branches popup; Log, history, compare and merge coming in M5b–d).
- [ ] Controller: CI Windows build (`[win]`), a scratch repo with branches; check the stripe title and the popup (computer-use only if the user grants it; otherwise ask the user).
- [ ] Commit `docs: Git foundation`.
