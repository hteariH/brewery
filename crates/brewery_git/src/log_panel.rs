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

use crate::GitCli;
use crate::log_filter_bar::{FilterStatus, LogFilterBar, LogFilterChanged};
use anyhow::Result;
use git::Oid;
use git::repository::{LogOrder, LogSource};
use git_ui::git_graph::GitGraph;
use gpui::{
    Action, App, AsyncWindowContext, Context, Entity, EventEmitter, FocusHandle, Focusable, Pixels,
    Render, Subscription, WeakEntity, Window, actions, px,
};
use project::Project;
use project::git_store::{
    GitGraphEvent, GitStore, GitStoreEvent, Repository, RepositoryEvent, RepositoryId,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::time::Duration;
use ui::prelude::*;
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
pub const NO_REPOSITORY: &str = "No Git repository";

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
    filter_bar: Entity<LogFilterBar>,
    active: bool,
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
            if let Some(log) = &this.log {
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
            filter_bar,
            active: false,
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
        let had_focus = self.focus_handle.contains_focused(window, cx);
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
                None => this.filter_bar.update(cx, |bar, cx| {
                    bar.set_error(Some(format!("No commit {hash}").into()), cx)
                }),
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

    /// The hosted graph of the Log tab, if the panel has been shown and there is a repository.
    pub fn log_graph(&self) -> Option<&Entity<GitGraph>> {
        self.log.as_ref().map(|log| &log.graph)
    }

    /// Selects `sha` in the Log tab, once the graph has loaded it.
    pub fn select_commit(&mut self, sha: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_graph(window, cx);
        let Ok(oid) = sha.parse::<Oid>() else {
            log::warn!("not a commit hash: {sha}");
            return;
        };
        if let Some(log) = &self.log {
            log.graph
                .update(cx, |graph, cx| graph.select_commit_by_sha(oid, cx));
        }
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
        self.filter_bar
            .update(cx, |bar, cx| bar.set_repository(repo, window, cx));
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
                if self.active {
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
                }
            }
            _ => {}
        }
    }
}

impl Render for LogPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.log {
            Some(log) => v_flex()
                .size_full()
                .child(self.filter_bar.clone())
                .child(div().flex_1().min_h_0().child(log.graph.clone()))
                .into_any_element(),
            None => v_flex()
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
            .child(body)
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
        if active {
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
