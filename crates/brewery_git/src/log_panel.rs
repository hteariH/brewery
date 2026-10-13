//! The Log tool window: Zed's commit graph (`git_ui::git_graph::GitGraph`) in the bottom dock,
//! for the active repository of the project.
//!
//! The graph is hosted as a plain child view, never as a pane item: it gets no tab, no nav
//! history and is not serialised with the workspace. Focus given to the panel is forwarded to the
//! graph, so its `GitGraph` key context (and every action bound there) works inside the panel.

use anyhow::Result;
use git::Oid;
use git_ui::git_graph::GitGraph;
use gpui::{
    Action, App, AsyncWindowContext, Context, Entity, EventEmitter, FocusHandle, Focusable, Pixels,
    Render, Subscription, WeakEntity, Window, actions, px,
};
use project::Project;
use project::git_store::{GitStore, GitStoreEvent, RepositoryId};
use schemars::JsonSchema;
use serde::Deserialize;
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

/// The graph of one repository; replaced when the active repository changes.
struct HostedGraph {
    repo_id: RepositoryId,
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
    active: bool,
    _git_store_events: Subscription,
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
        Self {
            workspace,
            project,
            focus_handle,
            position: DockPosition::Bottom,
            log: None,
            active: false,
            _git_store_events: git_store_events,
        }
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
        let had_focus = self.focus_handle.contains_focused(window, cx);
        self.log = repo_id.map(|repo_id| {
            let git_store = self.git_store(cx);
            let workspace = self.workspace.clone();
            let graph = cx.new(|cx| GitGraph::new(repo_id, git_store, workspace, None, window, cx));
            HostedGraph { repo_id, graph }
        });
        if had_focus {
            self.focus_handle.focus(window, cx);
        }
        cx.notify();
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
                    self.log = None;
                }
            }
            _ => {}
        }
    }
}

impl Render for LogPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.log {
            Some(log) => div()
                .size_full()
                .child(log.graph.clone())
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
