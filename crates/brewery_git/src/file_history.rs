//! File history in the Log tool window: a tab per file with that file's commits (Zed's
//! `GitGraph` for `LogSource::Path`, which follows renames) and the actions IntelliJ offers
//! there: Compare with Local, Show Diff and Get.
//!
//! `git::FileHistory` (editor, Git panel and Project panel) and `brewery::ShowFileHistory` open
//! the tab: the project panel goes through `git_ui_core::set_file_history_opener`, replaced
//! here; the editor and Git panel are caught in the capture phase, before `git_ui`'s own
//! handler opens a graph in the center pane. Without a Log panel in the workspace everything
//! falls back to `git_ui`.
//!
//! Renames: the history follows the file (`git log --follow`), so at an older commit the file
//! may have had another path. Each action asks git for the file's path at the selected commit
//! and at its first parent (`git log --follow --name-status -z` over the same history the tab
//! shows) and compares those paths.

use crate::cli::{GitCli, Oid, RepoPath};
use crate::log_panel::LogPanel;
use crate::revision_diff_view::{NewSide, OldSide, RevisionDiffView, SHORT_SHA_LEN};
use anyhow::{Context as _, Result, anyhow, bail};
use editor::Editor;
use git::repository::LogSource;
use git_ui::git_graph::GitGraph;
use git_ui::git_panel::GitPanel;
use gpui::{
    App, AsyncWindowContext, Context, Entity, FocusHandle, Focusable, PromptLevel, Render,
    Subscription, Task, WeakEntity, Window, actions,
};
use language::LineEnding;
use project::ProjectPath;
use project::git_store::{GitStore, Repository, RepositoryId};
use std::time::Duration;
use ui::{Tooltip, prelude::*};
use workspace::notifications::NotificationId;
use workspace::{Toast, Workspace};

actions!(
    brewery,
    [
        /// Shows the history of the selected file (editor, Git panel or Project panel) in the
        /// Log tool window.
        ShowFileHistory,
    ]
);

pub const COMPARE_WITH_LOCAL: &str = "Compare with Local";
pub const SHOW_DIFF: &str = "Show Diff";
pub const GET: &str = "Get";
pub const REPLACE: &str = "Replace";
const CANCEL: &str = "Cancel";
const FOLLOW_LOG_TIMEOUT: Duration = Duration::from_secs(120);

/// Must run after `git_ui::init`: it replaces the file history opener that `git_ui` installs.
pub fn init(cx: &mut App) {
    git_ui_core::set_file_history_opener(
        |workspace, project_path, window, cx| {
            if !open_for_project_path(workspace, project_path, window, cx) {
                open_in_center(workspace, project_path, window, cx);
            }
        },
        cx,
    );

    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|_, _: &ShowFileHistory, window, cx| {
            // Routed like `git::FileHistory`, so every source of a selection works.
            window.dispatch_action(Box::new(git::FileHistory), cx);
        });
        workspace.register_action_renderer(|div, workspace, _, cx| {
            if workspace.panel::<LogPanel>(cx).is_none() {
                return div;
            }
            let workspace = workspace.weak_handle();
            div.capture_action(move |_: &git::FileHistory, window, cx| {
                let handled = workspace
                    .update(cx, |workspace, cx| {
                        let Some((repo_id, paths)) = focused_targets(workspace, window, cx) else {
                            return false;
                        };
                        let mut handled = false;
                        for path in paths {
                            handled |=
                                LogPanel::show_file_history(workspace, repo_id, path, window, cx);
                        }
                        handled
                    })
                    .unwrap_or(false);
                if handled {
                    cx.stop_propagation();
                }
            })
        });
    })
    .detach();
}

/// Opens the history of `project_path` in the Log panel; `false` without a Log panel or a
/// repository for the path.
pub fn open_for_project_path(
    workspace: &mut Workspace,
    project_path: &ProjectPath,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> bool {
    let target = workspace
        .project()
        .read(cx)
        .git_store()
        .read(cx)
        .repository_and_path_for_project_path(project_path, cx)
        .map(|(repo, path)| (repo.read(cx).id, path));
    match target {
        Some((repo_id, path)) => LogPanel::show_file_history(workspace, repo_id, path, window, cx),
        None => false,
    }
}

/// `git_ui`'s own behaviour: a graph in the center pane.
fn open_in_center(
    workspace: &mut Workspace,
    project_path: &ProjectPath,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let Some((repo_id, source)) = git_ui::git_graph::resolve_file_history_target_from_project_path(
        workspace,
        project_path,
        cx,
    ) else {
        return;
    };
    let git_store = workspace.project().read(cx).git_store().clone();
    git_ui::git_graph::open_or_reuse_graph(workspace, repo_id, git_store, source, None, window, cx);
}

/// The files `git::FileHistory` is about, as `git_ui` resolves them: the Git panel's marked
/// files (or its selected entry) when the panel has focus, one tab each, else the active
/// editor's file.
///
/// The Project panel handles the action itself (through the opener) before this runs: both are
/// capture listeners on the workspace root, which run in registration order, and
/// `project_panel::init` runs before `brewery_git::init` (zed's `main.rs`).
fn focused_targets(
    workspace: &Workspace,
    window: &Window,
    cx: &App,
) -> Option<(RepositoryId, Vec<RepoPath>)> {
    if let Some(panel) = workspace.panel::<GitPanel>(cx)
        && panel.read(cx).focus_handle(cx).contains_focused(window, cx)
    {
        let (repo, paths) = panel.read(cx).file_history_targets()?;
        return Some((repo.read(cx).id, paths));
    }
    let editor = workspace.active_item_as::<Editor>(cx)?;
    let editor = editor.read(cx);
    let file = editor.file_at(editor.selections.newest_anchor().head(), cx)?;
    let project_path = ProjectPath {
        worktree_id: file.worktree_id(cx),
        path: file.path().clone(),
    };
    let (repo, path) = workspace
        .project()
        .read(cx)
        .git_store()
        .read(cx)
        .repository_and_path_for_project_path(&project_path, cx)?;
    Some((repo.read(cx).id, vec![path]))
}

/// The file's paths around a commit of its (rename-following) history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevisionPaths {
    /// The file's path at the commit.
    pub path: RepoPath,
    /// The commit's first parent and the file's path there; `None` for a root commit.
    pub parent: Option<(Oid, RepoPath)>,
}

/// One commit of `git log --follow --name-status -z --format=%x01%H%x00%P -- <path>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FollowedCommit {
    pub sha: Oid,
    pub parents: Vec<Oid>,
    /// The path at this commit.
    pub path: RepoPath,
    /// The path at the parents (the old name when this commit renamed the file).
    pub parent_path: RepoPath,
}

/// Parses the rename-following log of `path` (newest first). Commits without a status line
/// (merges) keep the path of the newer commit.
pub(crate) fn parse_follow_log(output: &str, path: &RepoPath) -> Vec<FollowedCommit> {
    let mut tokens = output.split('\0').peekable();
    let mut current = path.clone();
    let mut commits = Vec::new();
    while let Some(token) = tokens.next() {
        let Some(sha) = token.trim_start_matches('\n').strip_prefix('\x01') else {
            continue;
        };
        let parents = tokens.next().unwrap_or_default();
        let mut at = current.clone();
        let mut before = current.clone();
        while let Some(next) = tokens.peek() {
            if next.trim_start_matches('\n').starts_with('\x01') {
                break;
            }
            let status = tokens.next().unwrap_or_default().trim();
            if status.is_empty() {
                continue;
            }
            let repo_path = |token: Option<&str>| token.and_then(|token| RepoPath::new(token).ok());
            if status.starts_with(['R', 'C']) {
                let old = repo_path(tokens.next());
                let new = repo_path(tokens.next());
                if let (Some(old), Some(new)) = (old, new) {
                    before = old;
                    at = new;
                }
            } else if let Some(path) = repo_path(tokens.next()) {
                before = path.clone();
                at = path;
            }
        }
        let Ok(sha) = sha.parse::<Oid>() else {
            continue;
        };
        let parents = parents
            .split_whitespace()
            .filter_map(|parent| parent.parse::<Oid>().ok())
            .collect();
        current = before.clone();
        commits.push(FollowedCommit {
            sha,
            parents,
            path: at,
            parent_path: before,
        });
    }
    commits
}

/// The paths of `path` (a path in the working tree) at `sha` and its first parent.
pub fn revision_paths(
    repo: &Entity<Repository>,
    path: &RepoPath,
    sha: Oid,
    cx: &mut App,
) -> Task<Result<RevisionPaths>> {
    let log = GitCli::run(
        repo,
        vec![
            "log".into(),
            "--follow".into(),
            "--name-status".into(),
            "-z".into(),
            "--no-color".into(),
            "--no-show-signature".into(),
            "--format=%x01%H%x00%P".into(),
            "--".into(),
            path.as_unix_str().to_string(),
        ],
        Some(FOLLOW_LOG_TIMEOUT),
        cx,
    );
    let parent = GitCli::resolve_commit(repo, &format!("{sha}^"), cx);
    let path = path.clone();
    cx.background_spawn(async move {
        let output = log.await?;
        if !output.success() {
            bail!("git log failed: {}", output.message());
        }
        if let Some(commit) = parse_follow_log(&output.stdout, &path)
            .into_iter()
            .find(|commit| commit.sha == sha)
        {
            return Ok(RevisionPaths {
                path: commit.path,
                parent: commit
                    .parents
                    .first()
                    .map(|parent| (*parent, commit.parent_path)),
            });
        }
        // Not in the followed history (it changed since the tab loaded): same path.
        Ok(RevisionPaths {
            parent: parent.await?.map(|parent| (parent, path.clone())),
            path,
        })
    })
}

/// A file-history tab of the Log panel: the toolbar and the file's graph.
pub struct FileHistoryView {
    workspace: WeakEntity<Workspace>,
    git_store: Entity<GitStore>,
    repo_id: RepositoryId,
    path: RepoPath,
    graph: Entity<GitGraph>,
    _graph_changes: Subscription,
}

impl FileHistoryView {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        git_store: Entity<GitStore>,
        repo_id: RepositoryId,
        path: RepoPath,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let graph = cx.new(|cx| {
            GitGraph::new(
                repo_id,
                git_store.clone(),
                workspace.clone(),
                Some(LogSource::Path(path.clone())),
                window,
                cx,
            )
        });
        // The toolbar's buttons follow the graph's selection.
        let graph_changes = cx.observe(&graph, |_, _, cx| cx.notify());
        Self {
            workspace,
            git_store,
            repo_id,
            path,
            graph,
            _graph_changes: graph_changes,
        }
    }

    pub fn repo_id(&self) -> RepositoryId {
        self.repo_id
    }

    /// The file, repository-relative, as it is named in the working tree.
    pub fn path(&self) -> &RepoPath {
        &self.path
    }

    pub fn log_source(&self) -> LogSource {
        LogSource::Path(self.path.clone())
    }

    pub fn graph(&self) -> &Entity<GitGraph> {
        &self.graph
    }

    fn file_name(&self) -> &str {
        self.path.file_name().unwrap_or(self.path.as_unix_str())
    }

    /// `History: <file name>`.
    pub fn title(&self) -> SharedString {
        format!("History: {}", self.file_name()).into()
    }

    pub fn selected_sha(&self, cx: &App) -> Option<Oid> {
        self.graph.read(cx).selected_commit_sha()
    }

    /// Whether the toolbar's actions are enabled: a commit is selected.
    pub fn actions_enabled(&self, cx: &App) -> bool {
        self.selected_sha(cx).is_some()
    }

    fn repository(&self, cx: &App) -> Result<Entity<Repository>> {
        self.git_store
            .read(cx)
            .repositories()
            .get(&self.repo_id)
            .cloned()
            .context("the repository is gone")
    }

    fn selection(&self, cx: &App) -> Result<(Entity<Repository>, Oid)> {
        let sha = self.selected_sha(cx).context("no commit selected")?;
        Ok((self.repository(cx)?, sha))
    }

    /// Compares the selected commit's version of the file with the working copy.
    pub fn compare_with_local(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        self.run(
            COMPARE_WITH_LOCAL,
            window,
            cx,
            async |this, repo, sha, cx| {
                let (workspace, path) =
                    this.read_with(cx, |this, _| (this.workspace.clone(), this.path.clone()))?;
                let paths = cx
                    .update(|_, cx| revision_paths(&repo, &path, sha, cx))?
                    .await?;
                cx.update(|window, cx| {
                    RevisionDiffView::open_paths(
                        workspace,
                        repo,
                        OldSide::Revision(sha.to_string()),
                        paths.path,
                        NewSide::WorkingCopy,
                        path,
                        window,
                        cx,
                    )
                })?
                .await?;
                Ok(())
            },
        )
    }

    /// Compares the selected commit's version of the file with its first parent's.
    pub fn show_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<Result<()>> {
        self.run(SHOW_DIFF, window, cx, async |this, repo, sha, cx| {
            let (workspace, path) =
                this.read_with(cx, |this, _| (this.workspace.clone(), this.path.clone()))?;
            let paths = cx
                .update(|_, cx| revision_paths(&repo, &path, sha, cx))?
                .await?;
            let (old, old_path) = match paths.parent {
                Some((parent, parent_path)) => (OldSide::Revision(parent.to_string()), parent_path),
                None => (OldSide::Empty, paths.path.clone()),
            };
            cx.update(|window, cx| {
                RevisionDiffView::open_paths(
                    workspace,
                    repo,
                    old,
                    old_path,
                    NewSide::Revision(sha.to_string()),
                    paths.path,
                    window,
                    cx,
                )
            })?
            .await?;
            Ok(())
        })
    }

    /// Replaces the working copy of the file with the selected commit's version, after the
    /// user confirms, as IntelliJ's Get does: only the working copy changes, never the index.
    /// An open buffer is edited and saved; otherwise the same path is restored with
    /// `git restore --source=<sha> --worktree`, and across a rename the old version (filtered
    /// as `git cat-file --filters`) is written to the file's current path.
    pub fn get(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<Result<()>> {
        self.run(GET, window, cx, async |this, repo, sha, cx| {
            let (path, message) = this.read_with(cx, |this, _| {
                let short = sha.to_string()[..SHORT_SHA_LEN].to_string();
                let message = format!(
                    "Replace the working copy of {} with its version from {short}?",
                    this.file_name()
                );
                (this.path.clone(), message)
            })?;
            let answer = cx.update(|window, cx| {
                window.prompt(PromptLevel::Warning, &message, None, &[REPLACE, CANCEL], cx)
            })?;
            if answer.await.ok() != Some(0) {
                return Ok(());
            }
            let paths = cx
                .update(|_, cx| revision_paths(&repo, &path, sha, cx))?
                .await?;
            // In its checked-out form (eol and smudge filters, with the current path's
            // attributes), even across a rename.
            let content = cx
                .update(|_, cx| {
                    GitCli::checked_out_bytes_at(
                        &repo,
                        &sha.to_string(),
                        paths.path.clone(),
                        path.clone(),
                        cx,
                    )
                })?
                .await?
                .ok_or_else(|| anyhow!("{} does not exist at {sha}", paths.path.as_unix_str()))?;

            // An open buffer gets the content as an (undoable) edit and is saved, so no
            // editor shows a stale or conflicting copy.
            let open = this.update(cx, |this, cx| {
                let project = this.workspace.upgrade()?.read(cx).project().clone();
                let project_path = repo.read(cx).repo_path_to_project_path(&path, cx)?;
                let buffer = project.read(cx).get_open_buffer(&project_path, cx)?;
                Some((project, buffer))
            })?;
            if let Some((project, buffer)) = open
                && let Ok(text) = String::from_utf8(content.clone())
            {
                buffer.update(cx, |buffer, cx| {
                    if text.contains('\n') {
                        buffer.set_line_ending(LineEnding::detect(&text), cx);
                    }
                    buffer.set_text(text, cx);
                });
                project
                    .update(cx, |project, cx| project.save_buffer(buffer, cx))
                    .await?;
                return Ok(());
            }

            if paths.path == path {
                cx.update(|_, cx| GitCli::restore_worktree(&repo, &sha.to_string(), path, cx))?
                    .await?;
                return Ok(());
            }
            let (fs, abs_path) = this.update(cx, |this, cx| {
                let fs = this
                    .workspace
                    .upgrade()
                    .map(|workspace| workspace.read(cx).project().read(cx).fs().clone());
                let abs_path = repo
                    .read(cx)
                    .work_directory_abs_path
                    .join(path.as_std_path());
                (fs, abs_path)
            })?;
            fs.context("the workspace is gone")?
                .write(&abs_path, &content)
                .await?;
            Ok(())
        })
    }

    /// Runs an action on the selected commit; failures are also shown as a toast.
    fn run(
        &mut self,
        what: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
        action: impl AsyncFnOnce(
            WeakEntity<Self>,
            Entity<Repository>,
            Oid,
            &mut AsyncWindowContext,
        ) -> Result<()>
        + 'static,
    ) -> Task<Result<()>> {
        let selection = self.selection(cx);
        let workspace = self.workspace.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = match selection {
                Ok((repo, sha)) => action(this, repo, sha, cx).await,
                Err(error) => Err(error),
            };
            if let Err(error) = &result {
                let message = format!("{what} failed: {error:#}");
                log::warn!("{message}");
                workspace
                    .update(cx, |workspace, cx| {
                        workspace.show_toast(
                            Toast::new(NotificationId::unique::<FileHistoryView>(), message),
                            cx,
                        )
                    })
                    .ok();
            }
            result
        })
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = self.actions_enabled(cx);
        h_flex()
            .w_full()
            .px_2()
            .py_1()
            .gap_1()
            .border_b_1()
            .border_color(cx.theme().colors().border_variant)
            .child(
                Button::new("file-history-compare-with-local", COMPARE_WITH_LOCAL)
                    .label_size(LabelSize::Small)
                    .disabled(!enabled)
                    .tooltip(Tooltip::text(
                        "Compare the selected revision with the working copy",
                    ))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.compare_with_local(window, cx).detach()
                    })),
            )
            .child(
                Button::new("file-history-show-diff", SHOW_DIFF)
                    .label_size(LabelSize::Small)
                    .disabled(!enabled)
                    .tooltip(Tooltip::text(
                        "Compare the selected revision with its parent",
                    ))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.show_diff(window, cx).detach()),
                    ),
            )
            .child(
                Button::new("file-history-get", GET)
                    .label_size(LabelSize::Small)
                    .disabled(!enabled)
                    .tooltip(Tooltip::text(
                        "Replace the working copy with the selected revision",
                    ))
                    .on_click(cx.listener(|this, _, window, cx| this.get(window, cx).detach())),
            )
            .child(div().flex_1())
            .child(
                Label::new(self.path.as_unix_str().to_string())
                    .size(LabelSize::Small)
                    .color(Color::Muted)
                    .truncate(),
            )
    }
}

impl Render for FileHistoryView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(self.render_toolbar(cx))
            .child(div().flex_1().min_h_0().child(self.graph.clone()))
    }
}

impl Focusable for FileHistoryView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.graph.focus_handle(cx)
    }
}

#[cfg(test)]
#[path = "file_history_tests.rs"]
mod tests;
