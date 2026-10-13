# M5 (IntelliJ-style Git): gap analysis

Date: 2026-10-13. Scope: spec section "7a. Git" and the M5 row in
`docs/superpowers/specs/2026-10-10-brewery-design.md`. Line numbers are for the
tree at `43843193d3`.

## TL;DR

| Requirement | What Zed has now | Gap | Approach | Zed edits |
|---|---|---|---|---|
| Commit tool window | `GitPanel` already on the left dock (`git_panel.rs:9508`, default `"dock": "left"`), `alt-0` / `ctrl-k` in the JetBrains keymap | Labelled "Git Panel" with a branch icon; `ctrl-k` does not focus the message editor | Rename/re-icon in the Brewery stripe code, keymap tweaks | none in git_ui |
| Log | `GitGraph` (`git_graph.rs`, 8.5k lines): lanes, ref labels, a virtualised table, a details pane with changed files, commit search. It opens as a **centre tab** (`git_graph::Open`) | Not a bottom tool window. No filters (branch, author, date, path); search only moves between matches | `LogPanel` (bottom dock) that hosts `Entity<GitGraph>`, plus a Brewery filter bar that feeds a new `LogSource` variant | `LogSource::Args` variant (git crate) and about 4 match arms |
| File history | `git::FileHistory` opens `GitGraph` with `LogSource::Path` (`--follow`), `git_graph.rs:1074-1160`; project panel goes through `git_ui_core::set_file_history_opener` | No "compare revision with working copy", no "show diff with previous" for one file | Host the Path graph as a tab in `LogPanel`, add Brewery actions that use a selected-commit accessor | 1 pub accessor in `git_graph.rs` |
| Compare with branch/revision | Project level: `git::CompareWithBranch`, branch picker → `BranchDiff` (merge-base diff), `branch_diff.rs:138`. File level: only `FileDiffView` for two files on disk (`git_ui_core/src/file_diff_view.rs:45`) | No file-vs-revision view; no project-vs-arbitrary-revision | New `RevisionDiffView` (SplittableEditor + `build_buffer_diff`); project level reuses `BranchDiff` | 1 visibility change (`pub(crate)` → `pub`) |
| 3-way merge | Inline conflict markers with "Use ours / Use theirs / Use both / Resolve with Agent" (`conflict_view.rs:335-455`); `ConflictSet` parser with `base` support (`project/src/git_store/conflict_set.rs:105`); git panel "Conflicts" section | **No merge editor at all.** No merge, rebase, abort or continue operations in the git layer | New `MergeView` item, a diff3 engine and a Conflicts dialog, all in `brewery_git` | 1 hook in `git_panel.rs` `open_diff` |
| Branches popup | Title-bar trigger → `git_ui_core::build_branch_picker` → `GitPicker` popover with Branches/Stashes tabs (`title_bar.rs:1038-1093`, `git_ui.rs:71`); `BranchList` has All/Local/Remote filters, checkout, new branch, delete/force delete, add remote | No recent-by-checkout list, rebase onto, merge into current, compare with current, or per-branch submenu | A Brewery picker installed with `set_branch_picker_builder` after `git_ui::init` | 1 line in `main.rs` |

Upstream churn since 2026-07-01: `git_graph.rs` 25 commits;
`git_panel.rs` + `branch_picker.rs` + `conflict_view.rs` 79 commits. Do not
edit their bodies. Wrap them, and use the hook globals that already exist.

## What the git layer offers (crates/git, crates/project git_store)

`GitRepository` trait (`crates/git/src/repository.rs:795-1140`):

| Need | Available | Notes |
|---|---|---|
| Graph data | `initial_graph_data(LogSource, LogOrder, Sender<Vec<..>>)` :1113/:3389 | Streams `git log --format=%H%x00%P%x00%D --date-order <source>` in chunks of `GRAPH_CHUNK_SIZE = 1000` (:61). There is **no commit limit**: it loads everything. |
| Commit metadata | `commit_data_reader()` :1133/:3584, which keeps `git cat-file --batch` open | Loaded lazily for each visible row (`Repository::fetch_commit_data`, git_store.rs:7639) |
| Search | `search_commits(LogSource, {query, case_sensitive}, Sender<Oid>)` :3461 | `git log --fixed-strings --grep` or a sha prefix. Returns matches only, not a filtered graph. |
| Log sources | `LogSource::{All, Branch, Sha, Path}` :731 | All = `--branches --remotes --tags HEAD`; Path = `--follow -- path`. Only one dimension at a time. |
| File history (aux) | `file_history_changed_files(paths, limit)` :1127 | Changed-file sets for commits that touch the paths |
| Show/diff | `show`, `load_commit`, `diff(DiffType::{HeadToIndex, HeadToWorktree, MergeBase})`, `diff_tree(DiffTreeType::{MergeBase, MergeBaseWithWorktree, Since})` :1931 | `Since` is a plain two-commit tree diff, which is what compare with revision needs |
| Blob/revision text | `load_revisions(Vec<"rev:path">)` (cat-file batch) :828, `load_blob_content(oid)` | Also takes `:1:path`, `:2:path`, `:3:path`, the merge stages that 3-way merge needs |
| Blame | `blame`, `blame_at_revision` | used by `blame_ui.rs` |
| Branches | `branches`, `change_branch`, `create_branch`, `rename_branch`, `delete_branch`, `default_branch`, `create_ref/update_ref/delete_ref` | |
| Merge base | used inside `diff_tree` (`git merge-base`) :2037 | not exposed on its own |
| Merge / rebase / abort / continue / reflog | **missing** | only `pull` (+ `PullRebase`) |

`project::git_store::Repository`:

- `graph_data(source, order, range)` (git_store.rs:7432) starts the stream once
  per `(LogSource, LogOrder)` and caches it in `initial_graph_data`. The cache is
  cleared only on ref changes (:6704). With filters each combination stays
  cached until that happens, so the Brewery code must bound it.
- `send_job(..)` (:6914) is public and hands out `RepositoryState::Local { backend, environment }`,
  so a Brewery crate can run backend calls or its own `git` processes on the
  repository's serial job queue. **This is the main extension point.**
- `open_diff_since(oid, buffer, repo)` (:1664): a live `BufferDiff` of a buffer
  against a blob.
- `open_conflict_set(buffer)` (:2092) and `MergeDetails.merge_heads_by_conflicted_path`
  (:595), `has_conflict` / `had_conflict_on_last_merge_head_change` (:6417)
  give the conflict state. `GitStoreEvent::ConflictsUpdated` (:868) fires
  whenever it changes.
- Remote (collab) projects: graph and search are proxied over proto; file
  history is local-only. Brewery's features can be local-only for 0.1.

## Per feature

### 1. Commit tool window

Exists:
- `GitPanel` (`git_panel.rs`) is the changes list. It has file and hunk
  staging, the commit editor, amend/signoff, push/pull/fetch, stash, a
  Conflicts section (:603, :684) and a count badge (`icon_label` :9536).
- The dock defaults to the left side (`assets/settings/default.json:1100`), so
  it shows on Brewery's left stripe (`brewery_stripes.rs:31`).
- JetBrains keymap: `alt-0` and `ctrl-k` → `git_panel::ToggleFocus`
  (`assets/keymaps/linux/jetbrains.json:126,134,153`); `ctrl-alt-z` → `git::Restore`.

Missing compared with IntelliJ: the tooltip/title "Commit" and a commit
icon; `ctrl-k` should focus the commit message editor; changelists (post 0.1).

Approach: in `brewery_stripes.rs` (already a Brewery file) add a
`persistent_name → (title, icon)` override table with
`"GitPanel" → ("Commit", IconName::…)`. Do not touch `git_panel.rs`. For the
key: bind `ctrl-k` to `git_panel::FocusEditor` (`git_panel.rs:129`, :2483) in a Brewery keymap block. Risk: low.

### 2. Log tool window

Exists (`git_graph.rs`):
- Actions `git_graph::{Open, FocusSearch, ScrollUp/Down, ToggleChangedFilesView}`
  and `OpenAtCommit{sha}` (:582-607). Commit context menu: View Diff, Copy SHA,
  Copy Ref/Tag, Create Tag, custom git tasks (`commit_context_menu.rs`).
- `GitGraph::new(repo_id, git_store, workspace, Option<LogSource>, ..)` is
  public (:1621). `select_commit_by_sha` is public (:2526).
- Lanes: `GraphData::add_commits` (:950) assigns lanes **incrementally** for
  each streamed chunk. Rows are a `uniform_list` table (:3450, :4544).
  Edges are drawn on a canvas for the visible range only (:3544). This is
  already IntelliJ-grade virtualisation.
- Details pane with metadata, message, a changed-files flat/tree list, and
  diff stats. Clicking a file opens `CommitView` with a file filter
  (`commit_view.rs:183`).
- Search: `--grep` / sha. It highlights the matches and steps between them;
  it does not filter the list (:2242).
- Serialised as a workspace item; it is a centre-pane `Item` (:4745).

How it loads: it always runs one full `git log` over all refs and streams
1000-commit chunks; there is no cap or paging. Memory is about sha+parents+refs
for each commit, roughly 10-20 MB for 100k commits, which is acceptable. The
first chunk can be late on big repos without a commit-graph file, because
`--date-order` needs a full walk. Brewery should check
`core.commitGraph`/`git commit-graph write` and offer it.

Missing: a bottom-dock tool window; filters by branch (several), user,
date range and path; filter-as-you-type over the result; "Go to hash/branch".
Cherry-pick and revert are post-0.1.

Approach:
- `brewery_git::LogPanel` (`impl Panel`, `DockPosition::Bottom`, `alt-9`,
  which is null in the JetBrains keymap today, :152). It holds
  `Vec<Entity<GitGraph>>` tabs (Log plus file-history tabs) and renders the
  active one. GitGraph renders as a plain view, and its item-only parts
  (nav history, tab title) are simply unused.
- Filters: add **one** variant to `LogSource` in `crates/git`:
  `Args(Arc<[SharedString]>)` (`// brewery:`). `get_args` returns it
  verbatim; `log_source_to_proto` bails ("local only");
  `serialize_log_source_*` in `git_graph.rs:5070-5117` map it to `All`. That
  is about 4 short arms. `brewery_git::log_filter::LogFilter { branches, authors, since, until, paths, text }`
  → `to_git_args()` (`--author=`, `--since=`, `--until=`, `--grep`
  `--fixed-strings -i`, `--` paths). Applying a filter recreates the GitGraph
  entity with `Some(LogSource::Args(..))`.
- Graph with filters: path limiting with `%P` does not rewrite parents
  unless `--parents` is given, and author/date limiting breaks edges
  anyway. Pass `--parents` for path filters. For author/date/text, accept
  dots without connecting edges (IntelliJ does the same).
- Cache bound: when a filter is replaced, drop the old
  `(LogSource::Args, order)` entry. This needs a small public
  `Repository::forget_graph_data(source, order)`, about 4 lines, `// brewery:`.

Risks: GitGraph's internals change often upstream. Only rely on `new`,
`select_commit_by_sha` and the accessor below. Its key context `GitGraph`
keeps working inside a panel.

### 3. File history

Exists: `git::FileHistory` (editor or git panel selection) and the project
panel opener (`git_ui.rs:86`) → `open_or_reuse_graph(.., LogSource::Path)`
(`git_graph.rs:1214`). It shows a table without the graph column. Double
click opens `CommitView` filtered to the file.

Missing: compare the selected revision with the working copy; "Show Diff"
(revision vs its parent for this file only, which is close to what CommitView
filtered already does); "Get" (checkout that revision of the file);
"Annotate revision".

Approach:
- Override `set_file_history_opener` in `brewery_git::init` (must run after
  `git_ui::init`) so file history opens as a tab in `LogPanel`. Also register
  `brewery_git::ShowFileHistory`, bound in the JetBrains keymap and the VCS
  context menus. `git::FileHistory` stays upstream's.
- Add `pub fn selected_commit_sha(&self) -> Option<Oid>` to `GitGraph`
  (`// brewery:`, about 5 lines). A Brewery toolbar above the hosted graph
  has buttons Compare with Local, Show Diff and Get, which act on that sha.
- `Get` = `Repository::checkout_files(sha, [path])` (git_store.rs:7132).

### 4. Compare with branch / revision

Exists:
- Project level: `git::CompareWithBranch` → branch modal →
  `BranchDiff::deploy_branch_diff_with_base_ref` (`branch_diff.rs:138-200`).
  This is a merge-base diff (`DiffTreeType::MergeBaseWithWorktree`).
  `git::DiffBranch` diffs against the default branch.
- `FileDiffView` (git_ui_core) compares two files on disk. Its public
  `build_buffer_diff(old, new)` (:223) works with in-memory buffers.
- `TextDiffView` compares the clipboard with the selection (not relevant).

Missing: file vs branch/tag/revision; project vs an arbitrary revision
(not only branches); two-dot semantics (IntelliJ compares with the branch tip,
not with the merge base).

Approach:
- `brewery_git::RevisionDiffView`: an `Item` like `FileDiffView`. The left
  side is a read-only in-memory buffer loaded with `load_revisions(["rev:path"])`
  through `send_job`, with the language taken from the path. The right side is
  the live worktree buffer. It uses `SplittableEditor` + `build_buffer_diff`
  and is titled `name.rs (main) ↔ Working Copy`. The same view does
  revision-vs-revision for file history.
- `brewery_git::CompareWithBranchOrRevision` action: picker over branches,
  tags and "type a revision". For a file target it opens RevisionDiffView;
  for a project target it calls `BranchDiff::deploy_branch_diff_with_base_ref`
  (change `pub(crate)` to `pub`, `// brewery:`) with the chosen ref. Note in
  the UI that the project diff is merge-base based. A two-dot variant
  (`DiffTreeType::Since`) can come later.

### 5. 3-way merge

Exists: inline conflict markers. `ConflictSet` parses ours/base/theirs
regions with branch names, and `conflict_view.rs` adds the Use HEAD/Use
origin/Use both/Agent buttons above each conflict. The git panel lists
conflicted files in a "Conflicts" section and refuses to commit while they
are unstaged (:3885). There is no 3-pane view, no merge/rebase command, and
no conflicts dialog.

Missing: all of section 7a's 3-way merge.

Approach (all in `brewery_git`):
- `merge3.rs`: a pure diff3 over lines (imara-diff is already a workspace dep;
  `diffy` exists too, but it only gives text with markers). Output is
  `Vec<Chunk { base, ours, theirs, kind: Unchanged | OursOnly | TheirsOnly | BothSame | Conflict }>`.
  Auto-merge = apply all non-conflict chunks.
- Inputs: `load_revisions([":1:p", ":2:p", ":3:p"])`. During a rebase, "ours"
  and "theirs" are swapped; label them from `MERGE_HEAD`/`REBASE_HEAD`
  (git_store.rs:6475).
- `MergeView` item: three `Editor`s. Left = ours (read-only), centre = result
  (a buffer seeded with base + auto-merged chunks), right = theirs
  (read-only). Chunk row highlights, gutter `>>`/`<<`/`x` buttons per chunk,
  "Accept Left", "Accept Right", "Apply" (writes the worktree file, then
  `stage_paths`), and next/previous conflict. Scroll is synced by chunk
  anchors. Padding blocks for alignment can wait; `SplittableEditor` only
  handles two sides, so do not try to reuse it here.
- Conflicts dialog (modal): conflicted paths from `RepositorySnapshot` status
  with Accept Yours / Accept Theirs (`git checkout --ours|--theirs` + `git add`)
  and Merge… Opens on `GitStoreEvent::ConflictsUpdated` after a Brewery-run
  merge/rebase/pull, and from the action `brewery_git::ResolveConflicts`.
- Entry from the git panel: add a hook global in a **new** file
  `crates/git_ui_core/src/brewery_hooks.rs` (`set_conflict_opener`), plus a
  3-line `// brewery:` check in `GitPanel::open_diff` (`git_panel.rs:2569`):
  when the entry is conflicted and the hook is set, call it.
- Merge/rebase/abort/continue: `brewery_git::cli` runs `git` in the
  repository work dir with `LocalRepositoryState.environment`, inside
  `send_job`. Do not extend the `GitRepository` trait (Real + Fake + proto
  would be 3+ upstream files).

Risks: this is the largest UI piece. Getting the scroll sync and the result
buffer's undo right takes time. Also, the inline conflict buttons will keep
appearing on the same file in normal editors, which is fine.

### 6. Branches popup

Exists: `title_bar.rs:1038-1093` renders the branch button, and its
`PopoverMenu` calls `git_ui_core::build_branch_picker`, a global set in
`git_ui.rs:71` to `GitPicker` (Branches | Stashes tabs). `BranchList`
(`branch_picker.rs`, 3.5k lines) provides fuzzy search, filters All/Local/Remote
(:595), sorting by last commit date, checkout (with local-tracking creation for
remotes, repository.rs:5033), create branch from current or selected
(:1065), delete/force delete (:1111), add remote. JetBrains keymap
`ctrl-~` → `git::Branch`.

Missing: a Recent section (by checkout, from `git reflog`); a per-branch
submenu with Checkout, New Branch from…, Checkout and Rebase onto current,
Compare with Current, Show Diff with Working Tree, Rebase current onto, Merge
into current, Rename, Delete; top actions (Update Project, Commit…, Push…,
New Branch…, Checkout Tag or Revision…); local/remote grouping headers.

Approach: `brewery_git::BranchesPopup`, a `Picker` with its own delegate
(do not edit `BranchListDelegate`). It is installed by calling
`git_ui_core::set_branch_picker_builder` from `brewery_git::init`. That
requires `brewery_git::init(cx)` to run **after** `git_ui::init` (main.rs:790,
while the other Brewery inits are at :674-691), so add one marked line. The
submenu is a `ContextMenu` opened with →/Enter on a row. Merge/rebase go
through `brewery_git::cli` and then open the Conflicts dialog when needed.
Compare with Current uses the same machinery as feature 4 plus a commits list
(`git log A..B`, shown in a `LogPanel` tab with `LogSource::Args`). Stashes
stay reachable via `zed_actions::git::ViewStash`. Risk: low, because the hook
already exists.

## Edits to Zed files (all short, `// brewery:`)

| File | Edit | Size |
|---|---|---|
| `crates/zed/src/main.rs` | `brewery_git::init(cx)` after `git_ui::init` | 1 line |
| `crates/zed/src/zed.rs` | `LogPanel::load` + `add_panel_when_ready` (pattern at :785-818) | 2 lines |
| `crates/git/src/repository.rs` | `LogSource::Args(Arc<[SharedString]>)` + `get_args` arm | ~6 lines |
| `crates/project/src/git_store.rs` | `log_source_to_proto` arm (bail); `forget_graph_data` | ~10 lines |
| `crates/git_ui/src/git_graph.rs` | serialisation arms for `Args`; `selected_commit_sha()` | ~10 lines |
| `crates/git_ui/src/branch_diff.rs` | `deploy_branch_diff_with_base_ref` → `pub` | 1 word |
| `crates/git_ui/src/git_panel.rs` | conflict-open hook in `open_diff` | ~3 lines |
| `crates/git_ui_core/src/{git_ui_core.rs, brewery_hooks.rs}` | `pub mod` + new hook file | 1 line + new file |
| `crates/workspace/src/dock/brewery_stripes.rs` | Brewery file: Commit title/icon override | n/a |
| `assets/keymaps/linux/jetbrains.json` (+ macos) | `alt-9`, history, compare, conflicts, `ctrl-k` | few lines |

## Proposed split

Each task is sized for one implementer, with tests in the same task. Tests
use `gpui::TestAppContext` and real `git` in a temp dir, like the existing
tests in `repository.rs:4439+`.

### M5a: foundation, Commit tool window, Branches popup

1. **Crate + git CLI.** New crate `crates/brewery_git` (`brewery_git.rs`,
   `cli.rs`). `GitCli::run(repo: &Entity<Repository>, args) -> Task<Result<Output>>`
   via `send_job`. Helpers: `merge(branch)`, `rebase(onto)`,
   `abort/continue (merge|rebase)`, `checkout_side(path, Ours|Theirs)`,
   `recent_branches(n)` (reflog parse), `show_at(rev, path)`,
   `rev_list(a..b)`. Wire init in main.rs. Tests: each helper on a temp repo,
   including a conflicting merge that leaves MERGE_HEAD.
2. **Commit tool window.** Stripe title/icon override in
   `brewery_stripes.rs` with a unit test like the existing stripe tests; keymap
   `ctrl-k` focuses the commit editor. Test: the stripe summary shows "Commit".
3. **Branches popup.** `branches_popup.rs`: picker delegate with
   Recent/Local/Remote groups, top actions, per-branch submenu (checkout, new
   from, rebase onto, merge into current, compare with current (stub until
   M5c), rename, delete). Install via `set_branch_picker_builder`. Tests:
   grouping/order from a fake branch list (`set_branch_list_for_test`,
   git_store.rs:11468); merge and delete on a temp repo.

### M5b: Log tool window

1. **LogPanel.** `log_panel.rs`: bottom-dock Panel hosting GitGraph tabs,
   `alt-9`, `ToggleLogPanel`, persisted active tab. Zed edits: zed.rs panel
   load and `selected_commit_sha()`. Tests: panel opens at the bottom, the
   hosted graph loads commits from a temp repo, `OpenAtCommit` selects.
2. **Filters.** `LogSource::Args` (git crate + git_store + git_graph arms),
   `log_filter.rs` (`LogFilter::to_git_args`, pure), a filter bar UI (branch
   multi-select, user incl. "me", date presets/range, paths, text),
   `forget_graph_data`. Tests: `to_git_args` table; a filtered graph on a
   temp repo returns only matching commits; the cache entry is dropped on
   change.
3. **Scale check.** Bench or test that generates a 100k-commit repo
   (`git fast-import`), measures time to first chunk and full load, and
   scrolls the view. Add the commit-graph hint. Deliverable: numbers in the
   plan and any fix (for example `--topo-order` choice, or chunk size).

### M5c: file history and compare

1. **RevisionDiffView.** `revision_diff_view.rs`: rev:path vs worktree
   buffer and rev vs rev, title/tooltip, language detection. Tests: hunks for
   a modified file; a file missing at the revision shows as all added.
2. **File history.** `file_history.rs`: override `set_file_history_opener`,
   `ShowFileHistory` action, a toolbar (Compare with Local, Show Diff, Get)
   over the hosted Path graph. Tests: the action opens a LogPanel tab with
   `LogSource::Path`; Compare with Local opens RevisionDiffView for the
   selected sha; Get restores the content.
3. **Compare with branch/revision.** `compare.rs`: ref/revision picker; file
   → RevisionDiffView; project → `BranchDiff` (made `pub`); popup "Compare
   with Current" = commits `A..B` tab + file diff. Tests: file and project
   targets on a temp repo with two branches.

### M5d: 3-way merge

1. **diff3 engine.** `merge3.rs`, pure: chunking, auto-merge,
   accept-left/right per chunk and for the whole file, result text. Property
   tests: merging identical sides is a no-op; the result is identical to
   `git merge-file` output when there is no conflict.
2. **MergeView.** `merge_view.rs`: three editors, highlights, per-chunk
   buttons, next/previous conflict, Accept Left/Right, Apply (save + stage),
   synced scroll, rebase side labels. Tests: open on a conflicted temp repo;
   accept a chunk and check the result buffer; Apply stages and clears the
   conflict.
3. **Conflicts dialog + entry points.** `conflicts_dialog.rs`; the
   `git_ui_core` hook + `git_panel.rs` 3-line check; open after Brewery
   merge/rebase/pull on `ConflictsUpdated`; Abort/Continue buttons. Tests:
   a merge from the branches popup with a conflict opens the dialog; Accept
   Theirs resolves; a conflicted git-panel entry opens MergeView.

Order: M5a.1 first (everything uses the CLI). M5b, M5c and M5d.1 can then run
in parallel. M5c.2 needs M5b.1; M5d.2 needs M5d.1; M5a.3's
"compare with current" is finished in M5c.3.
