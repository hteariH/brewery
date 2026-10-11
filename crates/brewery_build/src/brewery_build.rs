//! The Gradle/Maven tool window: shows the modules and tasks that `brewery-daemon`
//! reads from the build files and runs a task in the terminal on double click/Enter.

use anyhow::Result;
use brewery_daemon_client::{BuildModule, BuildTool, DaemonClient, ProjectModel};
use futures::StreamExt as _;
use gpui::{
    Action, App, AsyncWindowContext, ClickEvent, Context, Entity, EventEmitter, FocusHandle,
    Focusable, Pixels, Render, ScrollHandle, SharedString, Task, WeakEntity, Window, actions, px,
};
use project::TaskSourceKind;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use task::{TaskContext, TaskTemplate};
use ui::{ListItem, ListItemSpacing, Tooltip, prelude::*};
use workspace::Workspace;
use workspace::dock::{DockPosition, Panel, PanelEvent};

actions!(
    brewery,
    [
        /// Shows or hides the Gradle/Maven tool window.
        ToggleBuildPanel,
        /// Re-reads the Gradle/Maven project model.
        ReloadBuildProject,
    ]
);

const BUILD_PANEL_KEY: &str = "BuildToolPanel";

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleBuildPanel, window, cx| {
            workspace.toggle_panel_focus::<BuildPanel>(window, cx);
        });
        workspace.register_action(|workspace, _: &ReloadBuildProject, _, cx| {
            if let Some(panel) = workspace.panel::<BuildPanel>(cx) {
                panel.update(cx, |panel, cx| panel.reload(cx));
            }
        });
    })
    .detach();
}

/// The command line that runs `task` of the module at `module_path`.
///
/// The project's wrapper is preferred because it pins the build tool version the
/// project was written for; the wrapper is addressed by absolute path because the
/// terminal's shell does not look for programs in the current directory on Windows
/// (PowerShell) and never on Unix.
pub fn task_command(
    model: &ProjectModel,
    module_path: &str,
    task: &str,
    os_windows: bool,
) -> (String, Vec<String>) {
    let root = Path::new(&model.root_dir);
    match model.build_tool {
        BuildTool::Maven => {
            let wrapper = if os_windows { "mvnw.cmd" } else { "mvnw" };
            let command = wrapper_or(root, wrapper, "mvn");
            let args = if is_maven_root(module_path) {
                vec![task.to_string()]
            } else {
                vec!["-pl".to_string(), module_path.to_string(), task.to_string()]
            };
            (command, args)
        }
        // A project without build files has no tasks, so this arm only matters for
        // completeness; Gradle's spelling is as good as any.
        BuildTool::Gradle | BuildTool::None => {
            let wrapper = if os_windows { "gradlew.bat" } else { "gradlew" };
            let command = wrapper_or(root, wrapper, "gradle");
            let task_path = if module_path.is_empty() || module_path == ":" {
                task.to_string()
            } else {
                format!("{}:{task}", module_path.trim_end_matches(':'))
            };
            (command, vec![task_path])
        }
    }
}

fn is_maven_root(module_path: &str) -> bool {
    module_path.is_empty() || module_path == "."
}

fn wrapper_or(root: &Path, wrapper: &str, fallback: &str) -> String {
    let wrapper = root.join(wrapper);
    if wrapper.is_file() {
        wrapper.to_string_lossy().into_owned()
    } else {
        fallback.to_string()
    }
}

/// Task indices of `module` grouped by their Gradle group (Maven: "lifecycle").
/// Named groups come first in alphabetical order; ungrouped tasks go last.
pub fn grouped_tasks(module: &BuildModule) -> Vec<(Option<&str>, Vec<usize>)> {
    let mut groups: Vec<(Option<&str>, Vec<usize>)> = Vec::new();
    for (ix, task) in module.tasks.iter().enumerate() {
        let group = task.group.as_deref().filter(|group| !group.is_empty());
        match groups.iter_mut().find(|(name, _)| *name == group) {
            Some((_, tasks)) => tasks.push(ix),
            None => groups.push((group, vec![ix])),
        }
    }
    groups.sort_by(|(a, _), (b, _)| match (a, b) {
        (Some(a), Some(b)) => a.cmp(b),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    groups
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Entry {
    Module(usize),
    Group(usize, SharedString),
    Task(usize, usize),
}

pub struct BuildPanel {
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
    position: DockPosition,
    /// Tests turn this off so that showing the panel does not launch a JVM.
    connect_on_show: bool,
    connected_once: bool,
    /// Dropping the client stops the daemon process.
    client: Option<Arc<DaemonClient>>,
    _project_changed: Option<lsp::Subscription>,
    _watch_changes: Option<Task<()>>,
    connect_task: Option<Task<()>>,
    fetch_task: Option<Task<()>>,
    model: Option<ProjectModel>,
    error: Option<SharedString>,
    busy: Option<SharedString>,
    collapsed: HashSet<String>,
    selected: Option<usize>,
}

impl BuildPanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
        workspace.update_in(&mut cx, |workspace, window, cx| {
            let weak = workspace.weak_handle();
            cx.new(|cx| Self::new(weak, true, window, cx))
        })
    }

    pub fn new(
        workspace: WeakEntity<Workspace>,
        connect_on_show: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            workspace,
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
            position: DockPosition::Right,
            connect_on_show,
            connected_once: false,
            client: None,
            _project_changed: None,
            _watch_changes: None,
            connect_task: None,
            fetch_task: None,
            model: None,
            error: None,
            busy: None,
            collapsed: HashSet::new(),
            selected: None,
        }
    }

    fn project_root(&self, cx: &App) -> Option<PathBuf> {
        let workspace = self.workspace.upgrade()?;
        let project = workspace.read(cx).project().read(cx);
        let worktree = project.visible_worktrees(cx).next()?;
        Some(worktree.read(cx).abs_path().to_path_buf())
    }

    /// Restarts the daemon after a failure (the JDK or the jar may have appeared
    /// since), otherwise asks the running daemon for a fresh model.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if self.client.is_some() && self.error.is_none() {
            self.fetch_model(cx);
        } else {
            self.connect(cx);
        }
    }

    fn disconnect(&mut self) {
        self.fetch_task = None;
        self.connect_task = None;
        self._watch_changes = None;
        self._project_changed = None;
        self.client = None;
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        self.connected_once = true;
        self.disconnect();
        let Some(root) = self.project_root(cx) else {
            self.error = Some("Open a folder to see its Gradle or Maven project.".into());
            cx.notify();
            return;
        };
        self.error = None;
        self.busy = Some("Starting brewery-daemon…".into());
        cx.notify();
        self.connect_task = Some(cx.spawn(async move |this, cx| {
            let result = DaemonClient::start(&root, cx).await;
            this.update(cx, |this, cx| match result {
                Ok(client) => this.attach(Arc::new(client), cx),
                Err(error) => {
                    this.busy = None;
                    this.error = Some(format!("{error:#}").into());
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn attach(&mut self, client: Arc<DaemonClient>, cx: &mut Context<Self>) {
        // The daemon notifies from its I/O thread; a channel brings that back to
        // the panel's foreground task.
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        self._project_changed = Some(client.on_project_changed(move || {
            tx.unbounded_send(()).ok();
        }));
        self._watch_changes = Some(cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                if this.update(cx, |this, cx| this.fetch_model(cx)).is_err() {
                    break;
                }
            }
        }));
        self.client = Some(client);
        self.fetch_model(cx);
    }

    fn fetch_model(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            self.connect(cx);
            return;
        };
        self.busy = Some("Importing the project…".into());
        cx.notify();
        self.fetch_task = Some(cx.spawn(async move |this, cx| {
            let result = client.project_model().await;
            this.update(cx, |this, cx| {
                this.busy = None;
                match result {
                    Ok(model) => {
                        this.model = Some(model);
                        this.error = None;
                    }
                    Err(error) => this.error = Some(format!("{error:#}").into()),
                }
                this.selected = None;
                cx.notify();
            })
            .ok();
        }));
    }

    fn entries(&self) -> Vec<Entry> {
        let Some(model) = &self.model else {
            return Vec::new();
        };
        let mut entries = Vec::new();
        for (module_ix, module) in model.modules.iter().enumerate() {
            entries.push(Entry::Module(module_ix));
            if self.collapsed.contains(&module.path) {
                continue;
            }
            for (group, tasks) in grouped_tasks(module) {
                let name = group.unwrap_or("other").to_string();
                entries.push(Entry::Group(module_ix, name.into()));
                entries.extend(tasks.into_iter().map(|task| Entry::Task(module_ix, task)));
            }
        }
        entries
    }

    fn toggle_module(&mut self, module_ix: usize, cx: &mut Context<Self>) {
        let Some(module) = self
            .model
            .as_ref()
            .and_then(|model| model.modules.get(module_ix))
        else {
            return;
        };
        if !self.collapsed.remove(&module.path) {
            self.collapsed.insert(module.path.clone());
        }
        // Collapsing shifts the rows below; keep the selection on the module.
        self.selected = self
            .entries()
            .iter()
            .position(|entry| *entry == Entry::Module(module_ix));
        cx.notify();
    }

    fn run_task(
        &mut self,
        module_ix: usize,
        task_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(model) = &self.model else {
            return;
        };
        let Some(module) = model.modules.get(module_ix) else {
            return;
        };
        let Some(task) = module.tasks.get(task_ix) else {
            return;
        };
        let (command, args) = task_command(model, &module.path, &task.name, cfg!(windows));
        let template = TaskTemplate {
            label: format!("{}: {}", module.name, task.name),
            command,
            args,
            cwd: Some(model.root_dir.clone()),
            ..TaskTemplate::default()
        };
        let task_cx = TaskContext {
            cwd: Some(PathBuf::from(&model.root_dir)),
            ..TaskContext::default()
        };
        self.workspace
            .update(cx, |workspace, cx| {
                workspace.schedule_task(
                    TaskSourceKind::UserInput,
                    &template,
                    &task_cx,
                    false,
                    window,
                    cx,
                );
            })
            .ok();
    }

    fn activate_entry(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        match self.entries().get(ix) {
            Some(Entry::Module(module_ix)) => self.toggle_module(*module_ix, cx),
            Some(Entry::Task(module_ix, task_ix)) => {
                self.run_task(*module_ix, *task_ix, window, cx)
            }
            Some(Entry::Group(..)) | None => {}
        }
    }

    fn select_by(&mut self, forward: bool, cx: &mut Context<Self>) {
        let entries = self.entries();
        let selectable: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| !matches!(entry, Entry::Group(..)))
            .map(|(ix, _)| ix)
            .collect();
        let next = match self.selected {
            None if forward => selectable.first().copied(),
            None => selectable.last().copied(),
            Some(current) if forward => selectable
                .iter()
                .copied()
                .find(|ix| *ix > current)
                .or(Some(current)),
            Some(current) => selectable
                .iter()
                .rev()
                .copied()
                .find(|ix| *ix < current)
                .or(Some(current)),
        };
        if let Some(next) = next {
            self.selected = Some(next);
            self.scroll_handle.scroll_to_item(next);
            cx.notify();
        }
    }

    fn select_next(&mut self, _: &menu::SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.select_by(true, cx);
    }

    fn select_previous(
        &mut self,
        _: &menu::SelectPrevious,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_by(false, cx);
    }

    fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.selected {
            self.activate_entry(ix, window, cx);
        }
    }

    fn on_entry_click(
        &mut self,
        ix: usize,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = Some(ix);
        window.focus(&self.focus_handle, cx);
        if event.click_count() >= 2 {
            self.activate_entry(ix, window, cx);
        }
        cx.notify();
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let title = match self.model.as_ref().map(|model| model.build_tool) {
            Some(BuildTool::Gradle) => "Gradle",
            Some(BuildTool::Maven) => "Maven",
            Some(BuildTool::None) => "No Gradle or Maven build",
            None => "Gradle/Maven",
        };
        h_flex()
            .h(px(32.))
            .px_2()
            .gap_2()
            .justify_between()
            .border_b_1()
            .border_color(cx.theme().colors().border)
            .child(
                h_flex()
                    .gap_2()
                    .child(Label::new(title).size(LabelSize::Small))
                    .children(
                        self.busy.clone().map(|busy| {
                            Label::new(busy).size(LabelSize::Small).color(Color::Muted)
                        }),
                    ),
            )
            .child(
                IconButton::new("build-panel-reload", IconName::RotateCw)
                    .icon_size(IconSize::Small)
                    .tooltip(move |_, cx| {
                        Tooltip::for_action("Reload Project", &ReloadBuildProject, cx)
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            )
    }

    fn render_error(&self, error: SharedString, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .p_2()
            .gap_2()
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Icon::new(IconName::Warning)
                            .size(IconSize::Small)
                            .color(Color::Warning),
                    )
                    .child(Label::new("Could not load the build project")),
            )
            .child(Label::new(error).size(LabelSize::Small).color(Color::Muted))
            .child(
                Button::new("build-panel-error-reload", "Reload")
                    .style(ButtonStyle::Filled)
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            )
    }

    fn render_entries(&self, model: &ProjectModel, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.entries()
            .into_iter()
            .enumerate()
            .filter_map(|(ix, entry)| {
                let selected = self.selected == Some(ix);
                let item =
                    match entry {
                        Entry::Module(module_ix) => {
                            let module = model.modules.get(module_ix)?;
                            let expanded = !self.collapsed.contains(&module.path);
                            ListItem::new(("build-module", ix))
                                .spacing(ListItemSpacing::Dense)
                                .toggle(expanded)
                                .on_toggle(cx.listener(move |this, _, _, cx| {
                                    this.toggle_module(module_ix, cx)
                                }))
                                .toggle_state(selected)
                                .start_slot(
                                    Icon::new(IconName::Folder)
                                        .size(IconSize::Small)
                                        .color(Color::Muted),
                                )
                                .child(Label::new(module.name.clone()))
                        }
                        Entry::Group(_, name) => ListItem::new(("build-group", ix))
                            .spacing(ListItemSpacing::Dense)
                            .indent_level(1)
                            .selectable(false)
                            .child(Label::new(name).size(LabelSize::Small).color(Color::Muted)),
                        Entry::Task(module_ix, task_ix) => {
                            let task = model.modules.get(module_ix)?.tasks.get(task_ix)?;
                            let item = ListItem::new(("build-task", ix))
                                .spacing(ListItemSpacing::Dense)
                                .indent_level(2)
                                .toggle_state(selected)
                                .start_slot(
                                    Icon::new(IconName::PlayOutlined)
                                        .size(IconSize::XSmall)
                                        .color(Color::Muted),
                                )
                                .child(Label::new(task.name.clone()));
                            match task.description.clone() {
                                Some(description) => item.tooltip(Tooltip::text(description)),
                                None => item,
                            }
                        }
                    };
                let item = item.on_click(cx.listener(move |this, event, window, cx| {
                    this.on_entry_click(ix, event, window, cx)
                }));
                Some(item.into_any_element())
            })
            .collect()
    }
}

impl Render for BuildPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if let Some(error) = self.error.clone() {
            self.render_error(error, cx).into_any_element()
        } else if let Some(model) = &self.model {
            let rows = self.render_entries(model, cx);
            if rows.is_empty() {
                div()
                    .p_2()
                    .child(
                        Label::new("No Gradle or Maven tasks found.")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .into_any_element()
            } else {
                v_flex()
                    .id("build-panel-entries")
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .children(rows)
                    .into_any_element()
            }
        } else {
            div().into_any_element()
        };

        v_flex()
            .key_context("BuildPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().panel_background)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::confirm))
            .child(self.render_header(cx))
            .child(body)
    }
}

impl Focusable for BuildPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for BuildPanel {}

impl Panel for BuildPanel {
    fn persistent_name() -> &'static str {
        "Build Tool Panel"
    }

    fn panel_key() -> &'static str {
        BUILD_PANEL_KEY
    }

    fn position(&self, _: &Window, _: &App) -> DockPosition {
        self.position
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(&mut self, position: DockPosition, _: &mut Window, cx: &mut Context<Self>) {
        self.position = position;
        cx.notify();
    }

    fn default_size(&self, _: &Window, _: &App) -> Pixels {
        px(300.)
    }

    fn icon(&self, _: &Window, _: &App) -> Option<IconName> {
        Some(IconName::ToolHammer)
    }

    fn icon_tooltip(&self, _: &Window, _: &App) -> Option<&'static str> {
        Some("Gradle/Maven")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleBuildPanel)
    }

    fn set_active(&mut self, active: bool, _: &mut Window, cx: &mut Context<Self>) {
        // The JVM is only worth starting once someone looks at the panel.
        if active && self.connect_on_show && !self.connected_once {
            self.connect(cx);
        }
    }

    fn activation_priority(&self) -> u32 {
        8
    }
}

#[cfg(test)]
#[path = "brewery_build_tests.rs"]
mod tests;
