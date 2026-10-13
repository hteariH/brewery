# Brewery M5d — 3-Way Merge Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** IntelliJ's conflict resolution: after a merge, rebase or pull stops on conflicts, a **Conflicts** dialog lists the conflicted files with Accept Yours, Accept Theirs and Merge…; **Merge…** opens a 3-pane editor (yours | result | theirs) where non-conflicting changes are already applied, each change can be accepted from either side, and Apply saves and marks the file resolved.

**Architecture:** Gap analysis `docs/superpowers/plans/m5-gap-analysis.md` section 5 "3-way merge" and "M5d". All new logic in `brewery_git`: `merge3.rs` (pure line-based diff3 using the workspace's `imara-diff`), `merge_view.rs` (a workspace `Item` with three `Editor`s: ours read-only, result editable, theirs read-only), `conflicts_dialog.rs` (a modal). Inputs come from the index stages `:1:path` (base), `:2:path` (ours), `:3:path` (theirs) through `GitCli::show_bytes_at` (or the backend's `load_revisions`) on the repository job queue. Entry points: the dialog after a Brewery-run merge/rebase (Branches popup) and on `GitStoreEvent::ConflictsUpdated`; the action `brewery::ResolveConflicts`; clicking a conflicted entry in the git panel through a new hook in `crates/git_ui_core/src/brewery_hooks.rs` checked by a 3-line `// brewery:` edit in `GitPanel::open_diff`.

**Tech Stack:** Rust, GPUI, Zed `editor`, `git`, `project`, `git_ui`, `git_ui_core`, `workspace`, `ui` crates, `imara-diff`.

**Spec:** `docs/superpowers/specs/2026-10-10-brewery-design.md` (section 7a "3-way merge", M5).

## Global Constraints

- Zed-file edits: new file `crates/git_ui_core/src/brewery_hooks.rs` (+ `pub mod brewery_hooks;` marked), `crates/git_ui/src/git_panel.rs` (≤ 5 marked lines in `open_diff`), JetBrains keymaps if bindings are added. Nothing else in Zed files.
- `merge3`: `pub fn merge3(base: &str, ours: &str, theirs: &str) -> Merge3` with `Merge3 { chunks: Vec<Chunk> }`, `Chunk { base: Range<usize>, ours: Range<usize>, theirs: Range<usize> /* line ranges */, kind: ChunkKind::{Unchanged, Ours, Theirs, BothSame, Conflict} }`; helpers `auto_merged_result(&self, sources) -> (String, Vec<ResultChunk>)` where non-conflict chunks take their changed side and conflict chunks start as the base text, `apply_choice(chunk, Side::{Ours, Theirs, Both})`. Line endings preserved (CRLF files stay CRLF; the trailing newline state follows the merged side). Equivalence: when there are no conflicts, the result equals `git merge-file -p ours base theirs` output.
- Side labels: during a merge, left = "Yours (<current branch>)", right = "Theirs (<MERGE_HEAD branch or short sha>)"; during a rebase git swaps stages — left must still be the user's own changes: label and order from the operation (`brewery_git::cli::in_progress()` + REBASE_HEAD/onto), so "Yours" is always the branch being rebased. Document the mapping in code.
- MergeView: title `<file name> (merge)`; toolbar: Previous/Next conflict (also F7/Shift+F7 in the JetBrains keymap if free), "Accept Left", "Accept Right" (whole file), "Apply" (enabled when no unresolved conflict remains; "Apply anyway" asks for confirmation if some remain), "Cancel". Per chunk: on the left and right editors a gutter or inline button `>>` / `<<` to take that side into the result, and `x` to ignore it; resolved chunks greyed. Highlights: conflict = red, change from one side = blue/green per side (use theme colours `version_control_*` or `status().conflict/created/modified`). Scroll sync by chunk: scrolling one editor scrolls the others so the same chunk stays aligned at the top (no padding blocks in 0.1). The result editor is fully editable; manual edits inside a chunk mark it resolved. Apply = write the result to the worktree file (through the project buffer if it is open, else the fs), then stage it (`Repository::stage_entries` or `git add` via GitCli), close the view; the conflict disappears from the git panel.
- Binary conflicts: no MergeView; the dialog offers only Accept Yours / Accept Theirs.
- Conflicts dialog: list of conflicted paths (from the repository snapshot status), each with Accept Yours / Accept Theirs (`GitCli::checkout_side`, which stages and handles delete/modify) and Merge…; header shows the operation ("Merging <branch> into <current>" / "Rebasing <current> onto <onto>"); footer buttons Abort (merge/rebase abort, confirm) and Continue (enabled when nothing is left: `continue_merge` / `continue_rebase`, outcomes as toasts). Opens: after a Brewery merge/rebase that returns `Conflicts`, and the M5a conflicts toast's action now opens it; `brewery::ResolveConflicts` action (also in the git panel's Conflicts section if the hook covers it).
- Every git call through `GitCli`/`Repository` jobs; no shell; refs/paths one argv element.
- Tests: pure tests for `merge3` (table + property-style randomised tests against `git merge-file` on small random inputs, seeded), gpui tests on temp repos with real conflicts (merge and rebase).

### Task 1: diff3 engine
**Files:** `crates/brewery_git/src/{merge3.rs, merge3_tests.rs}`, `Cargo.toml` (imara-diff).
- [ ] TDD: identical sides → all Unchanged; one-sided changes → Ours/Theirs; same change both sides → BothSame; overlapping different changes → Conflict; adjacent non-overlapping edits (match git's behaviour: git treats adjacent changes as a conflict — follow `git merge-file`); insertions at start/end; deletions; CRLF; no trailing newline; randomised equivalence with `git merge-file -p` for conflict-free cases (seeded, ~200 cases, small alphabets); conflict detection matches `git merge-file` exit status (number of conflicts) on random cases.
- [ ] Commit `feat: three-way merge engine`.

### Task 2: MergeView
**Files:** `crates/brewery_git/src/{merge_view.rs, merge_view_tests.rs}`, `brewery_git.rs`.
- [ ] TDD on a conflicted temp repo (merge): open the view for the file; result starts with non-conflicting changes applied and the conflict showing base text; accept left on the conflict → result has ours; accept both; manual edit marks resolved; Next/Previous conflict move the cursor; Apply writes the file, stages it and the repository no longer reports a conflict for it; rebase conflict → labels/sides mapped so Yours is the rebased branch; binary file → not offered.
- [ ] `cargo test -p brewery_git`, clippy, `cargo check -p zed`.
- [ ] Commit `feat: 3-pane merge editor`.

### Task 3: Conflicts dialog and entry points
**Files:** `crates/brewery_git/src/{conflicts_dialog.rs, conflicts_dialog_tests.rs}`, `branches_popup.rs` (open the dialog on Conflicts), `crates/git_ui_core/src/{git_ui_core.rs, brewery_hooks.rs}`, `crates/git_ui/src/git_panel.rs` (hook check), keymaps.
- [ ] TDD: a merge from the Branches popup that conflicts opens the dialog listing the files; Accept Theirs resolves one file; Merge… opens MergeView; Continue after all are resolved completes the merge; Abort restores the pre-merge HEAD; clicking a conflicted entry in the git panel opens MergeView through the hook (and without the hook Zed's behaviour is unchanged); `brewery::ResolveConflicts` opens the dialog when conflicts exist and shows "No conflicts" otherwise.
- [ ] Commit `feat: Conflicts dialog`.

### Task 4: CI, docs, on-screen check
- [ ] README "Git": conflicts and merge paragraph; remove "comes in the next update".
- [ ] Controller: `[win]` build; scratch repo with a conflict; Branches popup → Merge → dialog → Merge… (computer-use only if the user grants access, else ask the user).
- [ ] Commit `docs: 3-way merge`.
