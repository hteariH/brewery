//! IntelliJ's Compare with Branch or Revision: a picker over local branches, remote branches and
//! tags, plus the typed text as a revision. With an editor focused the file is compared (its
//! version at the revision against the working copy, in a [`RevisionDiffView`]); otherwise the
//! whole project is compared in Zed's `BranchDiff`.
//!
//! The project compare is Zed's merge-base diff: the changes made since the current branch
//! forked from the chosen ref, not a two-dot diff against the ref's tip. A toast says so, and
//! the diff's toolbar and tab tooltip keep saying it.
//!
//! Listed refs are compared by their full name (`refs/heads/x`, `refs/tags/x`): `rev-parse`
//! prefers a tag over a branch of the same short name. Titles show the short name.

use crate::branches_popup::{OutcomeToast, show_toast, show_workspace_toast};
use crate::cli::{GitCli, Oid, RepoPath, check_revision};
use crate::revision_diff_view::{NewSide, RevisionDiffView};
use anyhow::{Result, anyhow};
use editor::Editor;
use fuzzy_nucleo::{Case, LengthPenalty, StringMatchCandidate};
use git::repository::Branch;
use git_ui::branch_diff::BranchDiff;
use gpui::{
    App, Context, DismissEvent, Entity, Focusable as _, SharedString, Task, WeakEntity, Window,
    actions, rems,
};
use picker::{Picker, PickerDelegate};
use project::ProjectPath;
use project::git_store::Repository;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use ui::{HighlightedLabel, ListItem, ListItemSpacing, ListSubHeader, prelude::*};
use workspace::Workspace;

actions!(
    brewery,
    [
        /// Compares the focused editor's file, or else the whole project, with a branch, tag or
        /// revision.
        CompareWithRevision,
    ]
);

const TAG_LIST_TIMEOUT: Duration = Duration::from_secs(30);

/// The toast after a project compare: `BranchDiff` diffs against the merge base.
pub fn merge_base_note(rev: &str) -> String {
    format!("Compared with the merge base of {}", short_ref_name(rev))
}

/// `x` for `refs/heads/x`, `refs/remotes/x` and `refs/tags/x`; other revisions as they are.
pub fn short_ref_name(rev: &str) -> &str {
    ["refs/heads/", "refs/remotes/", "refs/tags/"]
        .iter()
        .find_map(|prefix| rev.strip_prefix(prefix))
        .unwrap_or(rev)
}

/// What is compared with the chosen revision.
#[derive(Clone, Debug, PartialEq)]
pub enum CompareTarget {
    /// One file (repository-relative), at the revision against the working copy.
    File {
        repo: Entity<Repository>,
        path: RepoPath,
    },
    /// The whole repository, as a merge-base diff.
    Project { repo: Entity<Repository> },
}

impl CompareTarget {
    pub fn repo(&self) -> &Entity<Repository> {
        match self {
            Self::File { repo, .. } | Self::Project { repo } => repo,
        }
    }

    fn placeholder(&self) -> String {
        match self {
            Self::File { path, .. } => format!(
                "Compare {} with a branch, tag or revision",
                path.file_name().unwrap_or(path.as_unix_str())
            ),
            Self::Project { .. } => "Compare the project with a branch, tag or revision".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefGroup {
    Local,
    Remote,
    Tags,
}

impl RefGroup {
    pub fn title(self) -> &'static str {
        match self {
            Self::Local => "Local Branches",
            Self::Remote => "Remote Branches",
            Self::Tags => "Tags",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::Local => IconName::GitBranch,
            Self::Remote => IconName::Server,
            Self::Tags => IconName::Bookmark,
        }
    }

    /// The full ref name of `name` in this group.
    pub fn full_ref(self, name: &str) -> String {
        let prefix = match self {
            Self::Local => "refs/heads/",
            Self::Remote => "refs/remotes/",
            Self::Tags => "refs/tags/",
        };
        format!("{prefix}{name}")
    }
}

/// The refs the picker offers, each list sorted by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RefList {
    pub local: Vec<String>,
    pub remote: Vec<String>,
    pub tags: Vec<String>,
}

impl RefList {
    /// Local and remote branches from Zed's branch list (a remote's `HEAD` alias left out).
    pub fn from_branches(branches: &[Branch], mut tags: Vec<String>) -> Self {
        let mut local = Vec::new();
        let mut remote = Vec::new();
        for branch in branches {
            if !branch.is_remote() {
                local.push(branch.name().to_string());
            } else if !branch.name().ends_with("/HEAD") {
                remote.push(branch.name().to_string());
            }
        }
        local.sort();
        remote.sort();
        tags.sort();
        Self {
            local,
            remote,
            tags,
        }
    }

    fn groups(&self) -> [(RefGroup, &[String]); 3] {
        [
            (RefGroup::Local, &self.local),
            (RefGroup::Remote, &self.remote),
            (RefGroup::Tags, &self.tags),
        ]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CompareRow {
    Header(RefGroup),
    Ref {
        name: String,
        group: RefGroup,
        /// Matched character positions in `name`.
        positions: Vec<usize>,
    },
    /// The typed text, used as a revision as it is (validated on confirm).
    Revision(String),
}

impl CompareRow {
    pub fn label(&self) -> String {
        match self {
            Self::Header(group) => group.title().to_string(),
            Self::Ref { name, .. } => name.clone(),
            Self::Revision(rev) => format!("Use '{rev}' as revision"),
        }
    }

    /// The revision this row compares with: a listed ref by its full name, typed text as it
    /// is; `None` for a header.
    pub fn revision(&self) -> Option<String> {
        match self {
            Self::Header(_) => None,
            Self::Ref { name, group, .. } => Some(group.full_ref(name)),
            Self::Revision(rev) => Some(rev.clone()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompareRows {
    pub rows: Vec<CompareRow>,
    pub selected: Option<usize>,
}

/// Typed text that names a commit by itself rather than a ref to search for: a sha of 7 or
/// more hex digits, or a revision expression (`HEAD~2`, `main^`, `@{1}`).
fn looks_like_revision(query: &str) -> bool {
    query.contains(['~', '^', '@'])
        || (query.len() >= 7 && query.chars().all(|c| c.is_ascii_hexdigit()))
}

/// The picker rows: a header and the refs of each non-empty group (fuzzy-filtered by `query`),
/// then, for a non-empty query that is not exactly a listed ref, a row using the text as a
/// revision. The best match is selected; the revision row when nothing matches or the text
/// looks like a revision itself.
pub fn compare_rows(refs: &RefList, query: &str) -> CompareRows {
    let query = query.trim();
    let groups = refs.groups();
    let names = groups
        .iter()
        .flat_map(|(group, names)| names.iter().map(move |name| (*group, name)))
        .collect::<Vec<_>>();
    let matches: Option<HashMap<usize, (f64, Vec<usize>)>> = (!query.is_empty()).then(|| {
        let candidates = names
            .iter()
            .enumerate()
            .map(|(id, (_, name))| StringMatchCandidate::new(id, name.to_string()))
            .collect::<Vec<_>>();
        fuzzy_nucleo::match_strings(
            &candidates,
            query,
            Case::Smart,
            LengthPenalty::On,
            usize::MAX,
        )
        .into_iter()
        .map(|m| (m.candidate_id, (m.score, m.positions)))
        .collect()
    });

    let mut rows = Vec::new();
    let mut best: Option<(usize, f64)> = None;
    let mut id = 0;
    for (group, group_names) in groups {
        let mut header = false;
        for name in group_names {
            let hit = match &matches {
                None => Some((0.0, Vec::new())),
                Some(matches) => matches.get(&id).cloned(),
            };
            id += 1;
            let Some((score, positions)) = hit else {
                continue;
            };
            if !header {
                rows.push(CompareRow::Header(group));
                header = true;
            }
            // The highest score wins; the earliest row wins a tie.
            if best.is_none_or(|(_, best_score)| score > best_score) {
                best = Some((rows.len(), score));
            }
            rows.push(CompareRow::Ref {
                name: name.clone(),
                group,
                positions,
            });
        }
    }

    let exact = names.iter().any(|(_, name)| name.as_str() == query);
    let mut selected = best.map(|(ix, _)| ix);
    if !query.is_empty() && !exact {
        if selected.is_none() || looks_like_revision(query) {
            selected = Some(rows.len());
        }
        rows.push(CompareRow::Revision(query.to_string()));
    }
    CompareRows { rows, selected }
}

/// Tag names (`refs/tags/` stripped), sorted by name.
pub fn list_tags(repo: &Entity<Repository>, cx: &mut App) -> Task<Result<Vec<String>>> {
    let output = GitCli::run(
        repo,
        vec![
            "for-each-ref".into(),
            "--format=%(refname:strip=2)".into(),
            "refs/tags".into(),
        ],
        Some(TAG_LIST_TIMEOUT),
        cx,
    );
    cx.background_spawn(async move {
        let output = output.await?;
        if !output.success() {
            return Err(anyhow!("git for-each-ref failed: {}", output.message()));
        }
        let mut tags = output
            .stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        tags.sort();
        Ok(tags)
    })
}

/// The commit `rev` names. Empty, `-`-prefixed and unknown revisions are errors.
pub fn validate_revision(repo: &Entity<Repository>, rev: &str, cx: &mut App) -> Task<Result<Oid>> {
    if let Err(error) = check_revision(rev, "revision") {
        return Task::ready(Err(error));
    }
    let rev = rev.to_string();
    let resolved = GitCli::resolve_commit(repo, &rev, cx);
    cx.background_spawn(async move {
        resolved
            .await?
            .ok_or_else(|| anyhow!("'{rev}' is not a branch, tag or commit"))
    })
}

/// What `CompareWithRevision` compares: the active editor's file when that editor has focus
/// and the file is in a repository, else the project's active repository.
pub fn target_for(workspace: &Workspace, window: &Window, cx: &App) -> Option<CompareTarget> {
    let git_store = workspace.project().read(cx).git_store().read(cx);
    if let Some(editor) = workspace.active_item_as::<Editor>(cx)
        && editor.focus_handle(cx).contains_focused(window, cx)
    {
        let editor = editor.read(cx);
        let file = editor.file_at(editor.selections.newest_anchor().head(), cx);
        let target = file.and_then(|file| {
            git_store.repository_and_path_for_project_path(
                &ProjectPath {
                    worktree_id: file.worktree_id(cx),
                    path: file.path().clone(),
                },
                cx,
            )
        });
        if let Some((repo, path)) = target {
            return Some(CompareTarget::File { repo, path });
        }
    }
    let repo = workspace.project().read(cx).active_repository(cx)?;
    Some(CompareTarget::Project { repo })
}

/// Opens the comparison of `target` with `rev` (already validated).
pub fn open_compare(
    workspace: &mut Workspace,
    target: CompareTarget,
    rev: String,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Task<Result<()>> {
    match target {
        CompareTarget::File { repo, path } => {
            let open = RevisionDiffView::open(
                workspace.weak_handle(),
                repo,
                path,
                rev,
                NewSide::WorkingCopy,
                window,
                cx,
            );
            cx.background_spawn(async move { open.await.map(|_| ()) })
        }
        CompareTarget::Project { repo } => {
            compare_project(workspace, repo, rev.into(), window, cx);
            Task::ready(Ok(()))
        }
    }
}

/// The project against `rev` in Zed's `BranchDiff` (a merge-base diff), with a toast saying
/// so. Errors while loading the diff are shown by `BranchDiff` itself.
pub fn compare_project(
    workspace: &mut Workspace,
    repo: Entity<Repository>,
    rev: SharedString,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let project = workspace.project().clone();
    let note = merge_base_note(&rev);
    BranchDiff::deploy_branch_diff_with_base_ref(workspace, project, repo, rev, None, window, cx);
    show_workspace_toast(workspace, OutcomeToast::success(note), cx);
}

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &CompareWithRevision, window, cx| {
            toggle(workspace, window, cx)
        });
    })
    .detach();
}

fn toggle(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let Some(target) = target_for(workspace, window, cx) else {
        show_workspace_toast(
            workspace,
            OutcomeToast::error("Compare failed", "no git repository"),
            cx,
        );
        return;
    };
    let handle = workspace.weak_handle();
    workspace.toggle_modal(window, cx, |window, cx| {
        let delegate = CompareDelegate::new(handle, target, window, cx);
        Picker::list(delegate, window, cx)
            .initial_width(rems(34.))
            .show_scrollbar(true)
    });
}

pub struct CompareDelegate {
    workspace: WeakEntity<Workspace>,
    target: CompareTarget,
    refs: RefList,
    rows: Vec<CompareRow>,
    selected_index: usize,
}

impl CompareDelegate {
    fn new(
        workspace: WeakEntity<Workspace>,
        target: CompareTarget,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Self {
        let repo = target.repo().clone();
        let refs = RefList::from_branches(&repo.read(cx).branch_list, Vec::new());
        let tags = list_tags(&repo, cx);
        cx.spawn_in(window, async move |picker, cx| {
            let tags = match tags.await {
                Ok(tags) => tags,
                Err(error) => {
                    log::debug!("tags unavailable: {error:#}");
                    return;
                }
            };
            picker
                .update_in(cx, |picker, window, cx| {
                    picker.delegate.refs.tags = tags;
                    picker.refresh(window, cx);
                })
                .ok();
        })
        .detach();
        Self {
            workspace,
            target,
            rows: compare_rows(&refs, "").rows,
            refs,
            selected_index: 0,
        }
    }

    pub fn target(&self) -> &CompareTarget {
        &self.target
    }

    pub fn rows(&self) -> &[CompareRow] {
        &self.rows
    }

    pub fn selected_row(&self) -> Option<&CompareRow> {
        self.rows.get(self.selected_index)
    }
}

impl PickerDelegate for CompareDelegate {
    type ListItem = AnyElement;

    fn name() -> &'static str {
        "brewery compare with revision"
    }

    fn match_count(&self) -> usize {
        self.rows.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(&mut self, ix: usize, _: &mut Window, _: &mut Context<Picker<Self>>) {
        self.selected_index = ix;
    }

    fn can_select(&self, ix: usize, _: &mut Window, _: &mut Context<Picker<Self>>) -> bool {
        self.rows
            .get(ix)
            .is_some_and(|row| row.revision().is_some())
    }

    fn placeholder_text(&self, _: &mut Window, _: &mut App) -> Arc<str> {
        self.target.placeholder().into()
    }

    fn no_matches_text(&self, _: &mut Window, _: &mut App) -> Option<SharedString> {
        Some("Type a branch, tag or commit".into())
    }

    fn update_matches(
        &mut self,
        query: String,
        _: &mut Window,
        _: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        let rows = compare_rows(&self.refs, &query);
        self.rows = rows.rows;
        self.selected_index = rows.selected.unwrap_or(0);
        Task::ready(())
    }

    fn confirm(&mut self, _secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(rev) = self.selected_row().and_then(CompareRow::revision) else {
            return;
        };
        let target = self.target.clone();
        let workspace = self.workspace.clone();
        let valid = validate_revision(target.repo(), &rev, cx);
        // An invalid revision keeps the picker open, so the text can be corrected.
        cx.spawn_in(window, async move |picker, cx| {
            let result = match valid.await {
                Err(error) => Err(error),
                Ok(_) => {
                    picker.update(cx, |_, cx| cx.emit(DismissEvent)).ok();
                    match workspace.update_in(cx, |workspace, window, cx| {
                        open_compare(workspace, target, rev, window, cx)
                    }) {
                        Ok(open) => open.await,
                        Err(error) => Err(error),
                    }
                }
            };
            if let Err(error) = result {
                cx.update(|_, cx| {
                    show_toast(
                        &workspace,
                        OutcomeToast::error("Compare failed", &format!("{error:#}")),
                        cx,
                    )
                })
                .ok();
            }
        })
        .detach();
    }

    fn dismissed(&mut self, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        cx.emit(DismissEvent);
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _: &mut Window,
        _: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let row = self.rows.get(ix)?;
        Some(match row {
            CompareRow::Header(group) => ListSubHeader::new(group.title())
                .inset(true)
                .into_any_element(),
            CompareRow::Ref {
                name,
                group,
                positions,
            } => ListItem::new(("ref", ix))
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .start_slot(
                    Icon::new(group.icon())
                        .size(IconSize::Small)
                        .color(Color::Muted),
                )
                .child(HighlightedLabel::new(name.clone(), positions.clone()))
                .into_any_element(),
            CompareRow::Revision(_) => ListItem::new(("revision", ix))
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .start_slot(
                    Icon::new(IconName::GitCommit)
                        .size(IconSize::Small)
                        .color(Color::Muted),
                )
                .child(Label::new(row.label()))
                .into_any_element(),
        })
    }
}

#[cfg(test)]
#[path = "compare_tests.rs"]
mod tests;
