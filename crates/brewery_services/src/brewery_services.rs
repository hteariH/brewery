//! The Services tool window: the Spring Boot applications, HTTP endpoints and beans that
//! `brewery-daemon` finds in the Java sources, with a Run button per application and
//! navigation to the declaration of every row.

use anyhow::Result;
use brewery_daemon_client::{
    DAEMON_KEEPS_STOPPING, DaemonErrorKind, DaemonState, DaemonStoppedNotification, HttpMethod,
    ProjectDaemon, ProjectDaemonEvent, RunConfiguration, RunConfigurationKind, SpringApp,
    SpringBean, SpringBeanKind, SpringEndpoint, SpringModel,
};
use editor::Editor;
use gpui::{
    Action, App, AsyncWindowContext, ClickEvent, Context, Entity, EventEmitter, FocusHandle,
    Focusable, Pixels, Render, ScrollHandle, SharedString, Subscription, Task, WeakEntity, Window,
    actions, px,
};
use project::{Project, TaskSourceKind};
use std::collections::HashSet;
use std::path::PathBuf;
use task::{TaskContext, TaskTemplate};
use text::Point;
use ui::{ListItem, ListItemSpacing, Tooltip, prelude::*};
use workspace::dock::{DockPosition, Panel, PanelEvent};
use workspace::notifications::NotificationId;
use workspace::{OpenLog, OpenOptions, OpenVisible, Toast, Workspace};

actions!(
    brewery,
    [
        /// Shows or hides the Services tool window (Spring Boot applications, endpoints and beans).
        ToggleServicesPanel,
    ]
);

const SERVICES_PANEL_KEY: &str = "ServicesPanel";

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleServicesPanel, window, cx| {
            workspace.toggle_panel_focus::<ServicesPanel>(window, cx);
        });
    })
    .detach();
}

/// The controller part of a `SimpleClass#method` handler.
pub fn controller_name(handler: &str) -> &str {
    handler
        .split_once('#')
        .map_or(handler, |(controller, _)| controller)
}

/// Endpoint indices grouped by controller, controllers in alphabetical order and the
/// endpoints of each by path, then method, so that the verbs of one path sit together.
pub fn endpoints_by_controller(endpoints: &[SpringEndpoint]) -> Vec<(&str, Vec<usize>)> {
    let mut groups: Vec<(&str, Vec<usize>)> = Vec::new();
    for (ix, endpoint) in endpoints.iter().enumerate() {
        let controller = controller_name(&endpoint.handler);
        match groups.iter_mut().find(|(name, _)| *name == controller) {
            Some((_, members)) => members.push(ix),
            None => groups.push((controller, vec![ix])),
        }
    }
    groups.sort_by_key(|(name, _)| *name);
    for (_, members) in &mut groups {
        members.sort_by_key(|&ix| {
            let endpoint = &endpoints[ix];
            (endpoint.path.as_str(), method_label(endpoint.method))
        });
    }
    groups
}

/// Bean indices grouped by kind in declaration order of `SpringBeanKind`, beans by name.
pub fn beans_by_kind(beans: &[SpringBean]) -> Vec<(SpringBeanKind, Vec<usize>)> {
    let mut groups: Vec<(SpringBeanKind, Vec<usize>)> = Vec::new();
    for (ix, bean) in beans.iter().enumerate() {
        match groups.iter_mut().find(|(kind, _)| *kind == bean.kind) {
            Some((_, members)) => members.push(ix),
            None => groups.push((bean.kind, vec![ix])),
        }
    }
    groups.sort_by_key(|(kind, _)| *kind);
    for (_, members) in &mut groups {
        members.sort_by(|&a, &b| beans[a].name.cmp(&beans[b].name));
    }
    groups
}

pub fn method_label(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Delete => "DELETE",
        HttpMethod::Patch => "PATCH",
        HttpMethod::Any => "ANY",
    }
}

/// Reads as "safe / creates / changes / removes", the way HTTP clients usually colour verbs.
fn method_color(method: HttpMethod) -> Color {
    match method {
        HttpMethod::Get => Color::Success,
        HttpMethod::Post => Color::Accent,
        HttpMethod::Put | HttpMethod::Patch => Color::Warning,
        HttpMethod::Delete => Color::Error,
        HttpMethod::Any => Color::Muted,
    }
}

pub fn kind_label(kind: SpringBeanKind) -> &'static str {
    match kind {
        SpringBeanKind::Component => "Components",
        SpringBeanKind::Service => "Services",
        SpringBeanKind::Repository => "Repositories",
        SpringBeanKind::Controller => "Controllers",
        SpringBeanKind::RestController => "REST controllers",
        SpringBeanKind::Configuration => "Configurations",
        SpringBeanKind::Bean => "@Bean methods",
    }
}

/// The daemon builds one configuration per application it can start; the main class pairs them
/// (simple names can collide across packages).
pub fn run_configuration_for<'a>(
    app: &SpringApp,
    configurations: &'a [RunConfiguration],
) -> Option<&'a RunConfiguration> {
    configurations
        .iter()
        .find(|configuration| configuration.main_class == app.main_class)
}

/// A Spring Boot label ends in "(Spring Boot)" so that the task stands out in the task picker and
/// in the Run widget's Rerun, which repeats the last scheduled task.
pub fn run_template(configuration: &RunConfiguration) -> TaskTemplate {
    let label = match configuration.kind {
        RunConfigurationKind::SpringBoot => format!("{} (Spring Boot)", configuration.label),
        RunConfigurationKind::Application => configuration.label.clone(),
    };
    TaskTemplate {
        label,
        command: configuration.command.clone(),
        args: configuration.args.clone(),
        cwd: Some(configuration.cwd.clone()),
        ..TaskTemplate::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Section {
    SpringBoot,
    Endpoints,
    Beans,
}

impl Section {
    const ALL: [Section; 3] = [Section::SpringBoot, Section::Endpoints, Section::Beans];

    fn title(self) -> &'static str {
        match self {
            Section::SpringBoot => "Spring Boot",
            Section::Endpoints => "Endpoints",
            Section::Beans => "Beans",
        }
    }

    fn empty_message(self) -> &'static str {
        match self {
            Section::SpringBoot => "No Spring Boot application found",
            Section::Endpoints => "No endpoints found",
            Section::Beans => "No beans found",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Group {
    Controller(SharedString),
    Kind(SpringBeanKind),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Entry {
    Section(Section),
    Empty(Section),
    App(usize),
    Group(Group),
    Endpoint(usize),
    Bean(usize),
}

struct PanelError {
    kind: DaemonErrorKind,
    message: SharedString,
}

pub struct ServicesPanel {
    workspace: WeakEntity<Workspace>,
    /// Held directly because `Panel` callbacks such as `set_active` run while the workspace is
    /// being updated, when reading the workspace entity panics.
    project: Entity<Project>,
    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
    position: DockPosition,
    /// Shared with the other Brewery panels of the project; requested on first show because the
    /// JVM is only worth starting once someone looks at the panel.
    daemon: Option<Entity<ProjectDaemon>>,
    _daemon_events: Option<Subscription>,
    fetch_task: Option<Task<()>>,
    model: Option<SpringModel>,
    run_configurations: Vec<RunConfiguration>,
    error: Option<PanelError>,
    busy: Option<SharedString>,
    /// Holds `Entry::Section` and `Entry::Group` values.
    collapsed: HashSet<Entry>,
    selected: Option<usize>,
    active: bool,
    /// The sources changed while the panel was hidden; refetch when it is shown again.
    stale: bool,
}

impl ServicesPanel {
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            workspace,
            project,
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
            position: DockPosition::Bottom,
            daemon: None,
            _daemon_events: None,
            fetch_task: None,
            model: None,
            run_configurations: Vec::new(),
            error: None,
            busy: None,
            collapsed: HashSet::new(),
            selected: None,
            active: false,
            stale: false,
        }
    }

    /// Restarts the daemon after it could not be reached (the JDK or the jar may have appeared
    /// since), otherwise asks the running daemon to scan again.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon.clone() else {
            self.connect(cx);
            return;
        };
        let unreachable = self
            .error
            .as_ref()
            .is_some_and(|error| error.kind == DaemonErrorKind::Connection);
        if daemon.read(cx).is_ready() && !unreachable {
            self.fetch_model(cx);
        } else {
            daemon.update(cx, |daemon, cx| daemon.reload(cx));
        }
    }

    /// Only touches the project and globals, so it is safe from `set_active`, which runs while
    /// the workspace is being updated.
    fn connect(&mut self, cx: &mut Context<Self>) {
        let daemon = ProjectDaemon::for_project(&self.project, cx);
        self._daemon_events = Some(cx.subscribe(&daemon, Self::on_daemon_event));
        let state = daemon.read(cx).state().clone();
        self.daemon = Some(daemon);
        // The daemon may already be running for another panel, so catch up with its state.
        match state {
            DaemonState::Connecting => self.show_connecting(cx),
            DaemonState::Ready => self.fetch_model(cx),
            DaemonState::Failed { error, kind } => self.show_failure(error, kind, cx),
        }
    }

    fn on_daemon_event(
        &mut self,
        daemon: Entity<ProjectDaemon>,
        event: &ProjectDaemonEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            ProjectDaemonEvent::Connecting => self.show_connecting(cx),
            // Run configurations depend on the build files as well as on the sources.
            ProjectDaemonEvent::Ready
            | ProjectDaemonEvent::ProjectChanged
            | ProjectDaemonEvent::SpringChanged => {
                // Scanning for a hidden panel is wasted work; catch up when it is shown.
                if self.active {
                    self.fetch_model(cx);
                } else {
                    self.stale = true;
                }
            }
            ProjectDaemonEvent::Failed { gave_up } => {
                if let DaemonState::Failed { error, kind } = daemon.read(cx).state().clone() {
                    self.show_failure(error, kind, cx);
                }
                if *gave_up {
                    self.notify_daemon_gave_up(cx);
                }
            }
        }
    }

    fn show_connecting(&mut self, cx: &mut Context<Self>) {
        self.fetch_task = None;
        self.error = None;
        self.busy = Some("Starting brewery-daemon…".into());
        cx.notify();
    }

    fn show_failure(&mut self, error: SharedString, kind: DaemonErrorKind, cx: &mut Context<Self>) {
        self.fetch_task = None;
        self.busy = None;
        self.error = Some(PanelError {
            kind,
            message: error,
        });
        cx.notify();
    }

    /// The panel may be hidden when the daemon gives up, so say it in a toast too. Deferred because
    /// this runs inside a panel update that may itself be nested in a workspace update.
    fn notify_daemon_gave_up(&self, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        cx.defer(move |cx| {
            workspace
                .update(cx, |workspace, cx| {
                    workspace.show_toast(
                        Toast::new(
                            NotificationId::unique::<DaemonStoppedNotification>(),
                            DAEMON_KEEPS_STOPPING,
                        )
                        .on_click("Open Log", |window, cx| {
                            window.dispatch_action(OpenLog.boxed_clone(), cx)
                        }),
                        cx,
                    );
                })
                .ok();
        });
    }

    /// While the daemon is not ready this waits: its next event says what to show.
    fn fetch_model(&mut self, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon.clone() else {
            self.connect(cx);
            return;
        };
        let daemon = daemon.read(cx);
        if !daemon.is_ready() {
            return;
        }
        self.stale = false;
        let model = daemon.spring_model(cx);
        let configurations = daemon.run_configurations(cx);
        self.busy = Some("Scanning sources…".into());
        cx.notify();
        self.fetch_task = Some(cx.spawn(async move |this, cx| {
            let model = model.await;
            let configurations = configurations.await;
            this.update(cx, |this, cx| {
                this.busy = None;
                match model {
                    Ok(model) => {
                        this.model = Some(model);
                        this.error = None;
                    }
                    Err(error) => {
                        this.error = Some(PanelError {
                            kind: DaemonErrorKind::of(&error),
                            message: format!("{error:#}").into(),
                        })
                    }
                }
                // Without configurations the tree is still worth showing; only Run is disabled.
                this.run_configurations = configurations.unwrap_or_else(|error| {
                    log::warn!("brewery-daemon run/configurations failed: {error:#}");
                    Vec::new()
                });
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
        for section in Section::ALL {
            entries.push(Entry::Section(section));
            if self.collapsed.contains(&Entry::Section(section)) {
                continue;
            }
            let start = entries.len();
            match section {
                Section::SpringBoot => {
                    entries.extend((0..model.applications.len()).map(Entry::App));
                }
                Section::Endpoints => {
                    for (controller, members) in endpoints_by_controller(&model.endpoints) {
                        let group = Entry::Group(Group::Controller(controller.to_string().into()));
                        let collapsed = self.collapsed.contains(&group);
                        entries.push(group);
                        if !collapsed {
                            entries.extend(members.into_iter().map(Entry::Endpoint));
                        }
                    }
                }
                Section::Beans => {
                    for (kind, members) in beans_by_kind(&model.beans) {
                        let group = Entry::Group(Group::Kind(kind));
                        let collapsed = self.collapsed.contains(&group);
                        entries.push(group);
                        if !collapsed {
                            entries.extend(members.into_iter().map(Entry::Bean));
                        }
                    }
                }
            }
            if entries.len() == start {
                entries.push(Entry::Empty(section));
            }
        }
        entries
    }

    fn toggle_collapsed(&mut self, entry: Entry, cx: &mut Context<Self>) {
        if !self.collapsed.remove(&entry) {
            self.collapsed.insert(entry.clone());
        }
        // Collapsing shifts the rows below; keep the selection on the toggled row.
        self.selected = self.entries().iter().position(|other| *other == entry);
        cx.notify();
    }

    fn run_app(&mut self, app_ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(app) = self
            .model
            .as_ref()
            .and_then(|model| model.applications.get(app_ix))
        else {
            return;
        };
        let Some(configuration) = run_configuration_for(app, &self.run_configurations) else {
            return;
        };
        let template = run_template(configuration);
        let task_cx = TaskContext {
            cwd: Some(PathBuf::from(&configuration.cwd)),
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

    fn location(&self, entry: &Entry) -> Option<(&str, u32)> {
        let model = self.model.as_ref()?;
        match entry {
            Entry::App(ix) => model
                .applications
                .get(*ix)
                .map(|app| (app.file.as_str(), app.line)),
            Entry::Endpoint(ix) => model
                .endpoints
                .get(*ix)
                .map(|endpoint| (endpoint.file.as_str(), endpoint.line)),
            Entry::Bean(ix) => model
                .beans
                .get(*ix)
                .map(|bean| (bean.file.as_str(), bean.line)),
            Entry::Section(_) | Entry::Empty(_) | Entry::Group(_) => None,
        }
    }

    /// Runs from click and keyboard handlers, never inside a workspace update, so the workspace
    /// can be updated here; the cursor is moved once the editor has opened.
    fn open_location(&self, file: &str, line: u32, window: &mut Window, cx: &mut Context<Self>) {
        let path = PathBuf::from(file);
        // The daemon's lines are 1-based.
        let point = Point::new(line.saturating_sub(1), 0);
        let options = OpenOptions {
            // A declaration outside the project's folders must not add a folder to the project.
            visible: Some(OpenVisible::None),
            focus: Some(true),
            ..OpenOptions::default()
        };
        let Ok(open) = self.workspace.update(cx, |workspace, cx| {
            workspace.open_abs_path(path, options, window, cx)
        }) else {
            return;
        };
        cx.spawn_in(window, async move |_, cx| {
            let item = open.await?;
            if let Some(editor) = item.downcast::<Editor>() {
                editor.update_in(cx, |editor, window, cx| {
                    editor.go_to_singleton_buffer_point(point, window, cx);
                })?;
            }
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    fn activate_entry(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.entries().get(ix).cloned() else {
            return;
        };
        match entry {
            Entry::Section(_) | Entry::Group(_) => self.toggle_collapsed(entry, cx),
            Entry::Empty(_) => {}
            Entry::App(_) | Entry::Endpoint(_) | Entry::Bean(_) => {
                if let Some((file, line)) = self.location(&entry) {
                    let file = file.to_string();
                    self.open_location(&file, line, window, cx);
                }
            }
        }
    }

    fn select_by(&mut self, forward: bool, cx: &mut Context<Self>) {
        let selectable: Vec<usize> = self
            .entries()
            .iter()
            .enumerate()
            .filter(|(_, entry)| !matches!(entry, Entry::Empty(_)))
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
        // Only the first click acts: the second click of a double click would toggle a
        // section back or open the same file again.
        if event.click_count() <= 1 {
            self.activate_entry(ix, window, cx);
        }
        cx.notify();
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .child(Label::new("Services").size(LabelSize::Small))
                    .children(
                        self.busy.clone().map(|busy| {
                            Label::new(busy).size(LabelSize::Small).color(Color::Muted)
                        }),
                    ),
            )
            .child(
                IconButton::new("services-panel-reload", IconName::RotateCw)
                    .icon_size(IconSize::Small)
                    .tooltip(Tooltip::text("Reload"))
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
                    .child(Label::new("Could not read the Spring model")),
            )
            .child(Label::new(error).size(LabelSize::Small).color(Color::Muted))
            .child(
                Button::new("services-panel-error-reload", "Reload")
                    .style(ButtonStyle::Filled)
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            )
            .child(
                Button::new("services-panel-error-open-log", "Open Log")
                    .on_click(|_, window, cx| window.dispatch_action(OpenLog.boxed_clone(), cx)),
            )
    }

    fn render_app(
        &self,
        ix: usize,
        app_ix: usize,
        app: &SpringApp,
        cx: &mut Context<Self>,
    ) -> ListItem {
        let run = match run_configuration_for(app, &self.run_configurations) {
            Some(_) => IconButton::new(("services-run", ix), IconName::PlayFilled)
                .icon_size(IconSize::Small)
                .icon_color(Color::Success)
                .tooltip(Tooltip::text("Run"))
                .on_click(cx.listener(move |this, _, window, cx| this.run_app(app_ix, window, cx))),
            None => IconButton::new(("services-run", ix), IconName::PlayFilled)
                .icon_size(IconSize::Small)
                .disabled(true)
                .tooltip(Tooltip::text(
                    "No run configuration: the build has no bootRun, run or spring-boot:run",
                )),
        };
        ListItem::new(("services-app", ix))
            .indent_level(1)
            .start_slot(
                Icon::new(IconName::Server)
                    .size(IconSize::Small)
                    .color(Color::Muted),
            )
            .child(Label::new(app.name.clone()))
            .end_slot(run)
            .tooltip(Tooltip::text(app.main_class.clone()))
    }

    fn render_endpoint(&self, ix: usize, endpoint: &SpringEndpoint) -> ListItem {
        let method = endpoint
            .handler
            .split_once('#')
            .map(|(_, method)| method.to_string());
        ListItem::new(("services-endpoint", ix))
            .indent_level(3)
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Label::new(method_label(endpoint.method))
                            .size(LabelSize::Small)
                            .color(method_color(endpoint.method)),
                    )
                    .child(Label::new(endpoint.path.clone()))
                    .children(method.map(|method| {
                        Label::new(method)
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                    })),
            )
            .tooltip(Tooltip::text(endpoint.handler.clone()))
    }

    fn render_bean(&self, ix: usize, bean: &SpringBean) -> ListItem {
        ListItem::new(("services-bean", ix)).indent_level(3).child(
            h_flex().gap_1().child(Label::new(bean.name.clone())).child(
                Label::new(bean.type_name.clone())
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            ),
        )
    }

    fn render_entries(&self, model: &SpringModel, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.entries()
            .into_iter()
            .enumerate()
            .filter_map(|(ix, entry)| {
                let selected = self.selected == Some(ix);
                let item = match &entry {
                    Entry::Section(section) => {
                        let expanded = !self.collapsed.contains(&entry);
                        let toggled = entry.clone();
                        ListItem::new(("services-section", ix))
                            .toggle(expanded)
                            .on_toggle(cx.listener(move |this, _, _, cx| {
                                this.toggle_collapsed(toggled.clone(), cx)
                            }))
                            .child(Label::new(section.title()))
                    }
                    Entry::Empty(section) => ListItem::new(("services-empty", ix))
                        .indent_level(1)
                        .selectable(false)
                        .child(
                            Label::new(section.empty_message())
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                    Entry::Group(group) => {
                        let title: SharedString = match group {
                            Group::Controller(name) => name.clone(),
                            Group::Kind(kind) => kind_label(*kind).into(),
                        };
                        let expanded = !self.collapsed.contains(&entry);
                        let toggled = entry.clone();
                        ListItem::new(("services-group", ix))
                            .indent_level(1)
                            .toggle(expanded)
                            .on_toggle(cx.listener(move |this, _, _, cx| {
                                this.toggle_collapsed(toggled.clone(), cx)
                            }))
                            .child(Label::new(title).color(Color::Muted))
                    }
                    Entry::App(app_ix) => {
                        self.render_app(ix, *app_ix, model.applications.get(*app_ix)?, cx)
                    }
                    Entry::Endpoint(endpoint_ix) => {
                        self.render_endpoint(ix, model.endpoints.get(*endpoint_ix)?)
                    }
                    Entry::Bean(bean_ix) => self.render_bean(ix, model.beans.get(*bean_ix)?),
                };
                let item = item
                    .spacing(ListItemSpacing::Dense)
                    .toggle_state(selected)
                    .on_click(cx.listener(move |this, event, window, cx| {
                        this.on_entry_click(ix, event, window, cx)
                    }));
                Some(item.into_any_element())
            })
            .collect()
    }
}

impl Render for ServicesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if let Some(error) = self.error.as_ref().map(|error| error.message.clone()) {
            self.render_error(error, cx).into_any_element()
        } else if let Some(model) = &self.model {
            v_flex()
                .id("services-panel-entries")
                .flex_1()
                .overflow_y_scroll()
                .track_scroll(&self.scroll_handle)
                .children(self.render_entries(model, cx))
                .into_any_element()
        } else {
            div().into_any_element()
        };

        v_flex()
            .key_context("ServicesPanel")
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

impl Focusable for ServicesPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for ServicesPanel {}

impl Panel for ServicesPanel {
    fn persistent_name() -> &'static str {
        "Services Panel"
    }

    fn panel_key() -> &'static str {
        SERVICES_PANEL_KEY
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
        px(300.)
    }

    fn icon(&self, _: &Window, _: &App) -> Option<IconName> {
        Some(IconName::Server)
    }

    fn icon_tooltip(&self, _: &Window, _: &App) -> Option<&'static str> {
        Some("Services")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleServicesPanel)
    }

    fn set_active(&mut self, active: bool, _: &mut Window, cx: &mut Context<Self>) {
        // The JVM is only worth starting once someone looks at the panel.
        self.active = active;
        if !active {
            return;
        }
        if self.daemon.is_none() {
            self.connect(cx);
        } else if self.stale {
            self.fetch_model(cx);
        }
    }

    fn activation_priority(&self) -> u32 {
        9
    }
}

#[cfg(test)]
#[path = "brewery_services_tests.rs"]
mod tests;
