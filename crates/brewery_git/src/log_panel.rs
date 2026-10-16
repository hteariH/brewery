//! The Log tool window: Zed's commit graph (`git_ui::git_graph::GitGraph`) in the bottom dock,
//! for the active repository of the project.
//!
//! The graph is hosted as a plain child view, never as a pane item: it gets no tab, no nav
//! history and is not serialised with the workspace. Focus given to the panel is forwarded to the
//! graph, so its `GitGraph` key context (and every action bound there) works inside the panel.
//!
//! The filter bar (`LogFilterBar`) sits above the graph. A filter maps to
//! `LogSource::Args(LogFilter::to_git_args())`; applying one replaces the hosted graph with one
//! for the new source and drops the replaced filtered graph's cached data
//! (`Repository::forget_graph_data`), so at most the current filtered graph and the unfiltered
//! one stay cached. A text that is a commit hash goes to that commit instead of filtering.
//! Filters are local only: in remote projects the bar is disabled.
//!
//! Large repositories: without a commit-graph file `git log --date-order` walks the whole
//! history before printing anything. When a graph's first chunk takes longer than a second, the
//! panel checks once per repository for a commit-graph file and, if there is none, offers to
//! write one (`git commit-graph write --reachable`).
//!
//! File history (`file_history`) opens further tabs, one per file: the panel then shows a tab
//! strip (Log first) and remembers the active tab while it runs. A history tab's graph data is
//! dropped when the tab closes.

use crate::GitCli;
use crate::cli::RepoPath;
use crate::file_history::FileHistoryView;
use crate::log_filter_bar::{FilterStatus, LogFilterBar, LogFilterChanged};
use anyhow::Result;
use git::Oid;
use git::repository::{LogOrder, LogSource};
use git_ui::git_graph::GitGraph;
use gpui::{
    Action, App, AsyncWindowContext, Context, Entity, EntityId, EventEmitter, FocusHandle,
    Focusable, Pixels, Render, Subscription, WeakEntity, Window, actions, px,
};
use project::Project;
use project::git_store::{
    GitGraphEvent, GitStore, GitStoreEvent, Repository, RepositoryEvent, RepositoryId,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};
use ui::{Tooltip, prelude::*};
use workspace::Workspace;
use workspace::dock::{DockPosition, Panel, PanelEvent};

actions!(
    brewery,
    [
        /// Shows or hides the Log tool window (the commit graph of the current repository).
        ToggleLogPanel,
    ]
);

/// Opens the Log tool window and selects a commit by its hash.
#[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = brewery)]
pub struct ShowCommitInLog {
    pub sha: String,
}

const LOG_PANEL_KEY: &str = "LogPanel";
pub const LOG_TAB: &str = "Log";
pub const NO_REPOSITORY: &str = "No Git repository";
pub const COMMIT_GRAPH_NOTICE: &str = "This repository has no commit-graph file, so the log is \
     slow to start. Writing one makes it load faster.";
const SLOW_FIRST_CHUNK: Duration = Duration::from_secs(1);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleLogPanel, window, cx| {
            workspace.toggle_panel_focus::<LogPanel>(window, cx);
        });
        workspace.register_action(|workspace, action: &ShowCommitInLog, window, cx| {
            LogPanel::show_commit(workspace, action.sha.clone(), window, cx);
        });
    })
    .detach();
}

/// The graph of one repository; replaced when the active repository or the filter changes.
struct HostedGraph {
    repo_id: RepositoryId,
    /// `None` is the unfiltered log (`LogSource::All`).
    source: Option<LogSource>,
    graph: Entity<GitGraph>,
}

/// A graph whose first chunk is still on its way, to time it.
struct PendingFirstChunk {
    repo_id: RepositoryId,
    source: LogSource,
    started: Instant,
}

/// The offer to write a commit-graph file, shown once per repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitGraphNotice {
    Offered,
    Writing,
    Failed(SharedString),
}

pub struct LogPanel {
    workspace: WeakEntity<Workspace>,
    /// Held directly because `Panel` callbacks such as `set_active` run while the workspace is
    /// being updated, when reading the workspace entity panics.
    project: Entity<Project>,
    focus_handle: FocusHandle,
    position: DockPosition,
    /// Created when the panel is first shown (a full `git log` is not worth running for a panel
    /// nobody looks at) and kept while it is hidden.
    log: Option<HostedGraph>,
    /// File-history tabs, after the Log tab, in the order they were opened.
    histories: Vec<Entity<FileHistoryView>>,
    /// The active file-history tab; `None` is the Log tab.
    active_history: Option<EntityId>,
    filter_bar: Entity<LogFilterBar>,
    active: bool,
    /// A first chunk slower than this, in a repository without a commit-graph file, offers to
    /// write one.
    slow_first_chunk: Duration,
    /// Repositories whose commit-graph state has been checked: the notice is offered once.
    commit_graph_checked: HashSet<RepositoryId>,
    first_chunk: Option<PendingFirstChunk>,
    notice: Option<(RepositoryId, CommitGraphNotice)>,
    /// A commit to show, asked for while the filtered log was loading: once it has loaded
    /// without it, the filter is reset to show it.
    pending_show: Option<Oid>,
    _git_store_events: Subscription,
    _filter_events: Subscription,
}

impl LogPanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
        workspace.update_in(&mut cx, |workspace, window, cx| {
            let weak = workspace.weak_handle();
            let project = workspace.project().clone();
            cx.new(|cx| Self::new(weak, project, window, cx))
        })
    }

    pub fn new(
        workspace: WeakEntity<Workspace>,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        // The panel itself has nothing to focus: hand focus to the graph, whose key context
        // carries the graph's actions.
        cx.on_focus(&focus_handle, window, |this, window, cx| {
            if let Some(history) = this.active_history() {
                history.focus_handle(cx).focus(window, cx);
            } else if let Some(log) = &this.log {
                log.graph.focus_handle(cx).focus(window, cx);
            }
        })
        .detach();
        let git_store = project.read(cx).git_store().clone();
        let git_store_events = cx.subscribe_in(&git_store, window, Self::on_git_store_event);
        let is_local = project.read(cx).is_local();
        let filter_bar = cx.new(|cx| {
            let mut bar = LogFilterBar::new(window, cx);
            bar.set_enabled(is_local, cx);
            bar
        });
        let filter_events = cx.subscribe_in(
            &filter_bar,
            window,
            |this, _, _: &LogFilterChanged, window, cx| this.apply_filter(window, cx),
        );
        Self {
            workspace,
            project,
            focus_handle,
            position: DockPosition::Bottom,
            log: None,
            histories: Vec::new(),
            active_history: None,
            filter_bar,
            active: false,
            slow_first_chunk: SLOW_FIRST_CHUNK,
            commit_graph_checked: HashSet::default(),
            first_chunk: None,
            notice: None,
            pending_show: None,
            _git_store_events: git_store_events,
            _filter_events: filter_events,
        }
    }

    pub fn filter_bar(&self) -> &Entity<LogFilterBar> {
        &self.filter_bar
    }

    /// The source the Log tab shows: `None` unfiltered.
    pub fn log_source(&self) -> Option<&LogSource> {
        self.log.as_ref().and_then(|log| log.source.as_ref())
    }

    /// The source for the bar's current filter. Errors go to the bar; filters are ignored
    /// where they cannot work (remote projects).
    fn filter_source(&self, cx: &mut Context<Self>) -> Option<LogSource> {
        let bar = self.filter_bar.read(cx);
        if !bar.is_enabled() {
            return None;
        }
        match bar.filter().log_source() {
            Ok(source) => source,
            Err(error) => {
                let error = SharedString::from(error.to_string());
                self.filter_bar
                    .update(cx, |bar, cx| bar.set_error(Some(error), cx));
                None
            }
        }
    }

    fn apply_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(log) = &self.log else {
            // Applied when the graph is first created.
            return;
        };
        let repo_id = log.repo_id;
        let current = log.source.clone();
        let source = self.filter_source(cx);
        if current != source {
            self.replace_graph(repo_id, source, window, cx);
        }
        let hash = self
            .filter_bar
            .read(cx)
            .filter()
            .commit_hash()
            .map(str::to_string);
        if let Some(hash) = hash {
            self.go_to_hash(hash, window, cx);
        }
        self.refresh_status(false, cx);
    }

    fn repository(&self, repo_id: RepositoryId, cx: &App) -> Option<Entity<Repository>> {
        self.git_store(cx)
            .read(cx)
            .repositories()
            .get(&repo_id)
            .cloned()
    }

    /// Replaces the hosted graph, keeping focus, and forgets the replaced filtered graph's data.
    fn replace_graph(
        &mut self,
        repo_id: RepositoryId,
        source: Option<LogSource>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_show = None;
        let had_focus = self.focus_handle.contains_focused(window, cx);
        self.time_first_chunk(repo_id, source.clone().unwrap_or_default(), cx);
        let git_store = self.git_store(cx);
        let workspace = self.workspace.clone();
        let graph =
            cx.new(|cx| GitGraph::new(repo_id, git_store, workspace, source.clone(), window, cx));
        let old = self.log.replace(HostedGraph {
            repo_id,
            source,
            graph,
        });
        if let Some(HostedGraph {
            repo_id: old_repo_id,
            source: Some(old_source),
            graph: old_graph,
        }) = old
        {
            drop(old_graph);
            let still_used = self.log.as_ref().is_some_and(|log| {
                log.repo_id == old_repo_id && log.source.as_ref() == Some(&old_source)
            });
            if !still_used && let Some(repo) = self.repository(old_repo_id, cx) {
                repo.update(cx, |repo, _| {
                    repo.forget_graph_data(&old_source, LogOrder::default())
                });
            }
        }
        if had_focus {
            self.focus_handle.focus(window, cx);
        }
        cx.notify();
    }

    /// Resolves a hash prefix with `git rev-parse` and selects the commit in the Log.
    fn go_to_hash(&mut self, hash: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self
            .log
            .as_ref()
            .and_then(|log| self.repository(log.repo_id, cx))
        else {
            return;
        };
        let resolve = GitCli::run(
            &repo,
            vec![
                "rev-parse".into(),
                "--verify".into(),
                "--quiet".into(),
                "--end-of-options".into(),
                format!("{hash}^{{commit}}"),
            ],
            Some(Duration::from_secs(20)),
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let sha = match resolve.await {
                Ok(output) if output.success() => Some(output.stdout.trim().to_string()),
                Ok(_) => None,
                Err(error) => {
                    log::debug!("rev-parse {hash}: {error:#}");
                    None
                }
            };
            this.update_in(cx, |this, window, cx| match sha {
                Some(sha) => this.select_commit(&sha, window, cx),
                // Not a commit (a date, a ticket number): search the messages for it.
                None => this
                    .filter_bar
                    .update(cx, |bar, cx| bar.set_text_not_a_commit(&hash, cx)),
            })
            .ok();
        })
        .detach();
    }

    /// Tells the bar how many commits the filtered graph has, or clears it when unfiltered.
    fn refresh_status(&mut self, finished: bool, cx: &mut Context<Self>) {
        let status = match &self.log {
            Some(HostedGraph {
                repo_id,
                source: Some(source),
                ..
            }) => self.repository(*repo_id, cx).map(|repo| {
                repo.update(cx, |repo, cx| {
                    let loading = !finished
                        && repo
                            .graph_data(source.clone(), LogOrder::default(), 0..0, cx)
                            .is_loading;
                    let count = repo
                        .get_graph_data(source.clone(), LogOrder::default())
                        .map_or(0, |data| data.commit_data.len());
                    FilterStatus { count, loading }
                })
            }),
            _ => None,
        };
        self.filter_bar
            .update(cx, |bar, cx| bar.set_status(status, cx));
    }

    /// The commit-graph notice for the repository the Log shows.
    pub fn commit_graph_notice(&self) -> Option<&CommitGraphNotice> {
        let (repo_id, notice) = self.notice.as_ref()?;
        (self.log.as_ref()?.repo_id == *repo_id).then_some(notice)
    }

    /// Runs `git commit-graph write --reachable` for the repository the notice is about.
    pub fn write_commit_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((repo_id, notice)) = &self.notice else {
            return;
        };
        if *notice == CommitGraphNotice::Writing {
            return;
        }
        let repo_id = *repo_id;
        let Some(repo) = self.repository(repo_id, cx) else {
            self.notice = None;
            return;
        };
        self.notice = Some((repo_id, CommitGraphNotice::Writing));
        let write = GitCli::run(
            &repo,
            vec!["commit-graph".into(), "write".into(), "--reachable".into()],
            Some(Duration::from_secs(600)),
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let failure = match write.await {
                Ok(output) if output.success() => None,
                Ok(output) => Some(output.message()),
                Err(error) => Some(format!("{error:#}")),
            };
            this.update(cx, |this, cx| {
                if this.notice.as_ref().map(|(id, _)| *id) == Some(repo_id) {
                    this.notice = failure.map(|message| {
                        let message = format!("git commit-graph write failed: {message}");
                        (repo_id, CommitGraphNotice::Failed(message.into()))
                    });
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub fn dismiss_commit_graph_notice(&mut self, cx: &mut Context<Self>) {
        if self.notice.take().is_some() {
            cx.notify();
        }
    }

    /// Times the first chunk of a graph that has to be loaded from git (not one in the cache).
    fn time_first_chunk(&mut self, repo_id: RepositoryId, source: LogSource, cx: &App) {
        self.first_chunk = None;
        if self.commit_graph_checked.contains(&repo_id) || !self.project.read(cx).is_local() {
            return;
        }
        let cached = self.repository(repo_id, cx).is_some_and(|repo| {
            repo.read(cx)
                .get_graph_data(source.clone(), LogOrder::default())
                .is_some()
        });
        if !cached {
            self.first_chunk = Some(PendingFirstChunk {
                repo_id,
                source,
                started: Instant::now(),
            });
        }
    }

    /// On the first chunk of a timed graph: if it was slow, checks once whether the repository
    /// lacks a commit-graph file (and has not turned them off), and if so offers to write one.
    fn first_chunk_arrived(
        &mut self,
        repo_id: RepositoryId,
        source: &LogSource,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self
            .first_chunk
            .take_if(|pending| pending.repo_id == repo_id && pending.source == *source)
        else {
            return;
        };
        if pending.started.elapsed() < self.slow_first_chunk
            || !self.commit_graph_checked.insert(repo_id)
        {
            return;
        }
        let Some(repo) = self.repository(repo_id, cx) else {
            return;
        };
        let work_dir = repo.read(cx).work_directory_abs_path.clone();
        let enabled = GitCli::run(
            &repo,
            vec!["config".into(), "--bool".into(), "core.commitGraph".into()],
            Some(Duration::from_secs(20)),
            cx,
        );
        let paths = GitCli::run(
            &repo,
            vec![
                "rev-parse".into(),
                "--git-path".into(),
                "objects/info/commit-graph".into(),
                "--git-path".into(),
                "objects/info/commit-graphs".into(),
            ],
            Some(Duration::from_secs(20)),
            cx,
        );
        cx.spawn(async move |this, cx| {
            // Unset means on (exit status 1, no output).
            let disabled = matches!(enabled.await, Ok(output) if output.stdout.trim() == "false");
            let missing = match paths.await {
                Ok(output) if output.success() => !output
                    .stdout
                    .lines()
                    .filter(|line| !line.is_empty())
                    .any(|line| work_dir.join(Path::new(line)).exists()),
                Ok(output) => {
                    log::debug!("rev-parse --git-path: {}", output.message());
                    false
                }
                Err(error) => {
                    log::debug!("rev-parse --git-path: {error:#}");
                    false
                }
            };
            if missing && !disabled {
                this.update(cx, |this, cx| {
                    this.notice = Some((repo_id, CommitGraphNotice::Offered));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn render_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let notice = self.commit_graph_notice()?;
        let (message, color) = match notice {
            CommitGraphNotice::Offered => (SharedString::from(COMMIT_GRAPH_NOTICE), Color::Default),
            CommitGraphNotice::Writing => (
                SharedString::from("Writing the commit-graph file\u{2026}"),
                Color::Muted,
            ),
            CommitGraphNotice::Failed(message) => (message.clone(), Color::Error),
        };
        let offered = matches!(notice, CommitGraphNotice::Offered);
        let writing = matches!(notice, CommitGraphNotice::Writing);
        Some(
            h_flex()
                .w_full()
                .px_2()
                .py_1()
                .gap_2()
                .border_b_1()
                .border_color(cx.theme().colors().border_variant)
                .child(
                    Icon::new(IconName::Info)
                        .size(IconSize::Small)
                        .color(Color::Muted),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Label::new(message).size(LabelSize::Small).color(color)),
                )
                .when(offered, |row| {
                    row.child(
                        Button::new("log-write-commit-graph", "Write commit-graph")
                            .label_size(LabelSize::Small)
                            .tooltip(Tooltip::text("git commit-graph write --reachable"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.write_commit_graph(window, cx)
                            })),
                    )
                })
                .when(!writing, |row| {
                    row.child(
                        IconButton::new("log-dismiss-commit-graph", IconName::Close)
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::text("Dismiss"))
                            .on_click(
                                cx.listener(|this, _, _, cx| this.dismiss_commit_graph_notice(cx)),
                            ),
                    )
                })
                .into_any_element(),
        )
    }

    /// The hosted graph of the Log tab, if the panel has been shown and there is a repository.
    pub fn log_graph(&self) -> Option<&Entity<GitGraph>> {
        self.log.as_ref().map(|log| &log.graph)
    }

    /// Selects `sha` in the Log tab, once the graph has loaded it. When the filter hides it,
    /// the filter is reset (as IntelliJ does).
    pub fn select_commit(&mut self, sha: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_history.is_some() {
            self.activate_tab(None, window, cx);
        }
        self.sync_graph(window, cx);
        let Ok(oid) = sha.parse::<Oid>() else {
            log::warn!("not a commit hash: {sha}");
            return;
        };
        self.pending_show = None;
        match self.filter_hides(oid, false, cx) {
            Some(true) => return self.show_hidden_commit(oid, window, cx),
            Some(false) => {}
            None if self.log.as_ref().is_some_and(|log| log.source.is_some()) => {
                self.pending_show = Some(oid);
            }
            None => {}
        }
        if let Some(log) = &self.log {
            log.graph
                .update(cx, |graph, cx| graph.select_commit_by_sha(oid, cx));
        }
    }

    /// Whether the filtered Log has loaded without `oid`: `None` when the Log is unfiltered or
    /// (unless `loaded`) still loading.
    fn filter_hides(&self, oid: Oid, loaded: bool, cx: &mut Context<Self>) -> Option<bool> {
        let log = self.log.as_ref()?;
        let source = log.source.clone()?;
        let repo = self.repository(log.repo_id, cx)?;
        repo.update(cx, |repo, cx| {
            if !loaded
                && repo
                    .graph_data(source.clone(), LogOrder::default(), 0..0, cx)
                    .is_loading
            {
                return None;
            }
            let data = repo.get_graph_data(source, LogOrder::default())?;
            Some(!data.commit_oid_to_index.contains_key(&oid))
        })
    }

    /// Resets the filter that hides `oid` and selects it in the unfiltered Log.
    fn show_hidden_commit(&mut self, oid: Oid, window: &mut Window, cx: &mut Context<Self>) {
        self.filter_bar.update(cx, |bar, cx| bar.reset(window, cx));
        self.apply_filter(window, cx);
        if let Some(log) = &self.log {
            log.graph
                .update(cx, |graph, cx| graph.select_commit_by_sha(oid, cx));
        }
    }

    /// The file-history tabs, in tab order (after the Log tab).
    pub fn histories(&self) -> &[Entity<FileHistoryView>] {
        &self.histories
    }

    /// The active file-history tab; `None` when the Log tab is active.
    pub fn active_history(&self) -> Option<&Entity<FileHistoryView>> {
        let id = self.active_history?;
        self.histories
            .iter()
            .find(|history| history.entity_id() == id)
    }

    /// The tab strip's titles (`Log`, then `History: <file>` per history tab).
    pub fn tab_titles(&self, cx: &App) -> Vec<SharedString> {
        std::iter::once(SharedString::from(LOG_TAB))
            .chain(
                self.histories
                    .iter()
                    .map(|history| history.read(cx).title()),
            )
            .collect()
    }

    /// The strip is shown only when there is more than the Log tab.
    pub fn shows_tab_strip(&self) -> bool {
        !self.histories.is_empty()
    }

    /// Opens (or activates) the history tab of `path` in repository `repo_id`.
    pub fn open_file_history(
        &mut self,
        repo_id: RepositoryId,
        path: RepoPath,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<FileHistoryView> {
        let existing = self.histories.iter().find(|history| {
            let history = history.read(cx);
            history.repo_id() == repo_id && *history.path() == path
        });
        let history = match existing {
            Some(history) => history.clone(),
            None => {
                let workspace = self.workspace.clone();
                let git_store = self.git_store(cx);
                let history = cx.new(|cx| {
                    FileHistoryView::new(workspace, git_store, repo_id, path, window, cx)
                });
                self.histories.push(history.clone());
                history
            }
        };
        self.activate_tab(Some(history.entity_id()), window, cx);
        history
    }

    /// Activates a history tab, or the Log tab for `None`.
    pub fn activate_tab(
        &mut self,
        history: Option<EntityId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let history = history.filter(|id| {
            self.histories
                .iter()
                .any(|history| history.entity_id() == *id)
        });
        let had_focus = self.focus_handle.contains_focused(window, cx);
        self.active_history = history;
        if history.is_none() && self.active {
            self.sync_graph(window, cx);
        }
        if had_focus {
            self.focus_handle.focus(window, cx);
        }
        cx.notify();
    }

    /// Closes a history tab and drops its graph data. Closing the active tab activates the
    /// one before it.
    pub fn close_file_history(
        &mut self,
        history: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self
            .histories
            .iter()
            .position(|tab| tab.entity_id() == history)
        else {
            return;
        };
        let closed = self.histories.remove(index);
        let (repo_id, source) = {
            let closed = closed.read(cx);
            (closed.repo_id(), closed.log_source())
        };
        drop(closed);
        let still_used = self.histories.iter().any(|history| {
            let history = history.read(cx);
            history.repo_id() == repo_id && history.log_source() == source
        });
        if !still_used && let Some(repo) = self.repository(repo_id, cx) {
            repo.update(cx, |repo, _| {
                repo.forget_graph_data(&source, LogOrder::default())
            });
        }
        if self.active_history == Some(history) {
            let previous = index
                .checked_sub(1)
                .and_then(|index| self.histories.get(index))
                .map(|history| history.entity_id());
            self.activate_tab(previous, window, cx);
        }
        cx.notify();
    }

    /// Opens the history of `path` in the workspace's Log panel and focuses it; the repository
    /// root opens the Log tab. `false` when the workspace has no Log panel.
    pub fn show_file_history(
        workspace: &mut Workspace,
        repo_id: RepositoryId,
        path: RepoPath,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> bool {
        let Some(panel) = workspace.panel::<LogPanel>(cx) else {
            return false;
        };
        // Before the panel is shown, so that showing it does not load the Log tab's graph.
        panel.update(cx, |panel, cx| {
            if path.is_empty() {
                panel.activate_tab(None, window, cx);
            } else {
                panel.open_file_history(repo_id, path, window, cx);
            }
        });
        workspace.focus_panel::<LogPanel>(window, cx);
        true
    }

    /// Opens and focuses the panel, then selects `sha` in it.
    pub fn show_commit(
        workspace: &mut Workspace,
        sha: String,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let Some(panel) = workspace.focus_panel::<LogPanel>(window, cx) else {
            return;
        };
        panel.update(cx, |panel, cx| panel.select_commit(&sha, window, cx));
    }

    fn active_repository_id(&self, cx: &App) -> Option<RepositoryId> {
        self.project
            .read(cx)
            .active_repository(cx)
            .map(|repository| repository.read(cx).id)
    }

    fn git_store(&self, cx: &App) -> Entity<GitStore> {
        self.project.read(cx).git_store().clone()
    }

    /// Makes the Log tab show the active repository: keeps the graph when it already does,
    /// replaces it otherwise.
    fn sync_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let repo_id = self.active_repository_id(cx);
        if self.log.as_ref().map(|log| log.repo_id) == repo_id {
            return;
        }
        // A different repository starts unfiltered (its branches and paths differ).
        let repo = repo_id.and_then(|repo_id| self.repository(repo_id, cx));
        let project_dir = repo.as_ref().and_then(|repo| {
            let work_dir = repo.read(cx).work_directory_abs_path.clone();
            self.project
                .read(cx)
                .visible_worktrees(cx)
                .find_map(|tree| {
                    let root = tree.read(cx).abs_path();
                    let dir = root.strip_prefix(&work_dir).ok()?;
                    Some(dir.to_string_lossy().replace('\\', "/"))
                })
        });
        self.filter_bar.update(cx, |bar, cx| {
            bar.set_repository(repo, project_dir, window, cx)
        });
        match repo_id {
            Some(repo_id) => {
                let source = self.filter_source(cx);
                self.replace_graph(repo_id, source, window, cx);
                self.refresh_status(false, cx);
            }
            None => {
                self.forget_filtered_graph(cx);
                self.log = None;
                cx.notify();
            }
        }
    }

    fn forget_filtered_graph(&mut self, cx: &mut Context<Self>) {
        if let Some(HostedGraph {
            repo_id,
            source: Some(source),
            ..
        }) = &self.log
            && let Some(repo) = self.repository(*repo_id, cx)
        {
            repo.update(cx, |repo, _| {
                repo.forget_graph_data(source, LogOrder::default())
            });
        }
    }

    fn on_git_store_event(
        &mut self,
        _: &Entity<GitStore>,
        event: &GitStoreEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            GitStoreEvent::ActiveRepositoryChanged(_)
            | GitStoreEvent::RepositoryAdded
            | GitStoreEvent::RepositoryRemoved(_) => {
                if let GitStoreEvent::RepositoryRemoved(removed) = event {
                    let gone = self
                        .histories
                        .iter()
                        .filter(|history| history.read(cx).repo_id() == *removed)
                        .map(|history| history.entity_id())
                        .collect::<Vec<_>>();
                    for history in gone {
                        self.close_file_history(history, window, cx);
                    }
                }
                if self.active && self.active_history.is_none() {
                    self.sync_graph(window, cx);
                } else if self.log.as_ref().map(|log| log.repo_id) != self.active_repository_id(cx)
                {
                    // Rebuilt when the panel is shown again.
                    self.forget_filtered_graph(cx);
                    self.log = None;
                }
            }
            GitStoreEvent::RepositoryUpdated(
                repo_id,
                RepositoryEvent::GraphEvent((source, _), graph_event),
                _,
            ) => {
                if !matches!(graph_event, GitGraphEvent::LoadingError) {
                    self.first_chunk_arrived(*repo_id, source, cx);
                }
                if let Some(log) = &self.log
                    && log.repo_id == *repo_id
                    && log.source.as_ref() == Some(source)
                {
                    // The fetch task emits its last event before it finishes.
                    let finished = matches!(
                        graph_event,
                        GitGraphEvent::FullyLoaded | GitGraphEvent::LoadingError
                    );
                    self.refresh_status(finished, cx);
                    if matches!(graph_event, GitGraphEvent::FullyLoaded)
                        && let Some(oid) = self.pending_show.take()
                        && self.filter_hides(oid, true, cx) == Some(true)
                    {
                        self.show_hidden_commit(oid, window, cx);
                    }
                }
            }
            _ => {}
        }
    }
}

impl LogPanel {
    fn render_tab_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_history;
        let tab = |id: SharedString, label: SharedString, selected: bool| {
            h_flex()
                .id(id)
                .h_full()
                .px_2()
                .gap_1()
                .border_r_1()
                .border_color(cx.theme().colors().border_variant)
                .cursor_pointer()
                .when(selected, |tab| {
                    tab.bg(cx.theme().colors().tab_active_background)
                })
                .hover(|tab| tab.bg(cx.theme().colors().ghost_element_hover))
                .child(Label::new(label).size(LabelSize::Small).color(if selected {
                    Color::Default
                } else {
                    Color::Muted
                }))
        };
        h_flex()
            .w_full()
            .h_7()
            .flex_none()
            .border_b_1()
            .border_color(cx.theme().colors().border_variant)
            .bg(cx.theme().colors().tab_bar_background)
            .child(
                tab("log-tab".into(), LOG_TAB.into(), active.is_none()).on_click(
                    cx.listener(|this, _, window, cx| this.activate_tab(None, window, cx)),
                ),
            )
            .children(self.histories.iter().map(|history| {
                let id = history.entity_id();
                let title = history.read(cx).title();
                let tooltip = history.read(cx).path().as_unix_str().to_string();
                tab(
                    format!("log-history-tab-{id}").into(),
                    title,
                    active == Some(id),
                )
                .tooltip(Tooltip::text(tooltip))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.activate_tab(Some(id), window, cx)),
                )
                .child(
                    IconButton::new(format!("log-history-close-{id}"), IconName::Close)
                        .icon_size(IconSize::XSmall)
                        .tooltip(Tooltip::text("Close"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.close_file_history(id, window, cx)
                        })),
                )
            }))
    }
}

impl Render for LogPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match (self.active_history(), &self.log) {
            (Some(history), _) => history.clone().into_any_element(),
            (None, Some(log)) => v_flex()
                .size_full()
                .child(self.filter_bar.clone())
                .children(self.render_notice(cx))
                .child(div().flex_1().min_h_0().child(log.graph.clone()))
                .into_any_element(),
            (None, None) => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(Label::new(NO_REPOSITORY).color(Color::Muted))
                .into_any_element(),
        };
        v_flex()
            .key_context("LogPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().panel_background)
            .when(self.shows_tab_strip(), |panel| {
                panel.child(self.render_tab_strip(cx))
            })
            .child(div().flex_1().min_h_0().child(body))
    }
}

impl Focusable for LogPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for LogPanel {}

impl Panel for LogPanel {
    fn persistent_name() -> &'static str {
        "Log Panel"
    }

    fn panel_key() -> &'static str {
        LOG_PANEL_KEY
    }

    fn position(&self, _: &Window, _: &App) -> DockPosition {
        self.position
    }

    fn position_is_valid(&self, _: DockPosition) -> bool {
        true
    }

    fn set_position(&mut self, position: DockPosition, _: &mut Window, cx: &mut Context<Self>) {
        self.position = position;
        cx.notify();
    }

    fn default_size(&self, _: &Window, _: &App) -> Pixels {
        px(360.)
    }

    fn icon(&self, _: &Window, _: &App) -> Option<IconName> {
        Some(IconName::GitGraph)
    }

    fn icon_tooltip(&self, _: &Window, _: &App) -> Option<&'static str> {
        Some("Log")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleLogPanel)
    }

    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.active = active;
        // A history tab needs nothing; the Log tab's graph is built when first shown.
        if active && self.active_history.is_none() {
            self.sync_graph(window, cx);
        }
    }

    fn activation_priority(&self) -> u32 {
        11
    }
}

#[cfg(test)]
#[path = "log_panel_tests.rs"]
mod tests;
