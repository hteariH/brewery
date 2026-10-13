# Brewery M5c — File History and Compare Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** IntelliJ's file history and compare: Show History for a file opens a tab in the Log tool window with that file's commits and the actions Compare with Local, Show Diff and Get; Compare with Branch or Revision works for a file (side-by-side against the chosen ref) and for the whole project; the Branches popup's "Compare with current" becomes live.

**Architecture:** Gap analysis `docs/superpowers/plans/m5-gap-analysis.md` sections 3 "File history", 4 "Compare with branch / revision" and "M5c". New in `brewery_git`: `RevisionDiffView` (a workspace `Item` with a read-only buffer holding `rev:path` loaded through `Repository::send_job` + `load_revisions`, diffed against either the live worktree buffer or a second revision buffer, using Zed's split diff editor and `build_buffer_diff` as `FileDiffView` does), file-history tabs in `LogPanel` (overriding `git_ui_core::set_file_history_opener` from `brewery_git::init`), a ref/revision picker, and project compare through `BranchDiff::deploy_branch_diff_with_base_ref` (made `pub`).

**Tech Stack:** Rust, GPUI, Zed `git`, `project`, `git_ui`, `git_ui_core`, `editor`, `language`, `picker`, `workspace` crates.

**Spec:** `docs/superpowers/specs/2026-10-10-brewery-design.md` (section 7a "File history", "Compare with branch / revision", M5).

## Global Constraints

- Zed-file edits: `crates/git_ui/src/branch_diff.rs` (`deploy_branch_diff_with_base_ref` → `pub`, marked), JetBrains keymaps; anything else only if unavoidable, marked `// brewery:` and a few lines. The `GitGraph::selected_commit_sha` accessor from M5b is reused.
- `RevisionDiffView`: title `<file name> (<short ref>) ↔ Working Copy` or `<file name> (<a>) ↔ (<b>)`; tooltip with full refs and path; left (old) side read-only, language from the path; right side is the real worktree buffer (editable, so the user can copy changes over) for working-copy compares; a file missing at the revision shows as entirely added (and missing in the working copy as entirely deleted); binary or non-UTF-8 content → a message "Binary file — can't show differences"; files over 5 MB → the same kind of message. Revisions are resolved once when the view opens (sha shown in the tooltip).
- File history: `brewery::ShowFileHistory` (JetBrains keymap: none by default in IntelliJ; add to the editor tab context menu and the project panel context menu only if those menus are extensible from outside — otherwise rely on `git::FileHistory`, which is now routed to the Log panel by the opener override). Each file-history tab is a `GitGraph` with `LogSource::Path`, tab title `History: <file name>`, closable; the Log panel shows its tab strip when more than one tab exists and remembers the active tab while running. Toolbar above a history tab: **Compare with Local** (selected commit's version vs working copy), **Show Diff** (selected commit vs its first parent, for this file), **Get** (restore that revision of the file into the working copy, confirm first: "Replace the working copy of <file> with its version from <short sha>?"), disabled when nothing is selected.
- Compare with Branch or Revision: action `brewery::CompareWithRevision`; a picker listing local branches, remote branches, tags (headers), plus a "Use '<typed text>' as revision" row for free text; target = the active editor's file when an editor is focused, else the whole project. File → `RevisionDiffView`; project → `BranchDiff` with the chosen ref and a note in the view header or toast "Compared with the merge base of <ref>". Free-text revisions are validated with `git rev-parse --verify --quiet <rev>^{commit}` via GitCli (refuse `-`-prefixed input).
- Branches popup "Compare with '<current>'" (M5a stub): opens the project compare against that branch.
- Git subprocesses only through `GitCli` / `Repository` jobs; no shell; every user value one argv element.
- Tests: real `git` in temp repos with isolated config (M5a helpers), gpui tests for views/actions.

### Task 1: RevisionDiffView
**Files:** `crates/brewery_git/src/{revision_diff_view.rs, revision_diff_view_tests.rs}`, `brewery_git.rs`.
- [ ] TDD on a temp repo: modified file vs HEAD~1 shows the expected hunks; file missing at the revision → all added; deleted in working copy → all deleted; rev vs rev; binary → message; title/tooltip; language set from the path (e.g. `.rs` → Rust when the language is registered in the test registry, else assert the path-based lookup is requested).
- [ ] `cargo test -p brewery_git`, clippy, `cargo check -p zed`.
- [ ] Commit `feat: compare a file with a revision`.

### Task 2: File history
**Files:** `crates/brewery_git/src/{file_history.rs, file_history_tests.rs}`, `log_panel.rs` (tabs), `brewery_git.rs` (opener override).
- [ ] TDD: `git::FileHistory` / `ShowFileHistory` on a file opens a history tab in the Log panel with `LogSource::Path`; a second file opens a second tab; closing a tab; Compare with Local opens RevisionDiffView for the selected sha; Show Diff opens selected-vs-parent; Get (confirmed) restores the file content and the git panel sees the change; buttons disabled without a selection.
- [ ] Commit `feat: file history in the Log tool window`.

### Task 3: Compare with branch or revision
**Files:** `crates/brewery_git/src/{compare.rs, compare_tests.rs}`, `branches_popup.rs` (enable Compare), `crates/git_ui/src/branch_diff.rs` (pub), JetBrains keymaps if a binding fits.
- [ ] TDD: picker rows grouped (local/remote/tags) + free-text row; validation refuses unknown and `-`-prefixed revisions; file target opens RevisionDiffView against the ref; project target opens BranchDiff for the ref; the popup's Compare with current works.
- [ ] Commit `feat: compare with a branch or revision`.

### Task 4: CI, docs, on-screen check
- [ ] README "Git": history and compare paragraph (and the merge-base note for project compare).
- [ ] Controller: `[win]` build; history of a file in the Brewery repo; Compare with Local; Compare with Branch (computer-use only if the user grants access, else ask the user).
- [ ] Commit `docs: file history and compare`.
