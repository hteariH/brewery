//! The Database tool window: one row per data source, its schema below it, loaded lazily from
//! `db/introspect`, and a toolbar to add, edit, delete and refresh data sources.

use crate::ToggleDatabasePanel;
use crate::console::{QueryConsole, open_console};
use crate::data_source_modal::DataSourceModal;
use crate::schema_cache::SchemaCache;
use crate::statements::{qualified_name, quote_identifier, select_all_from, select_top_from};
use crate::tree::{DbTree, Fetch, Row, RowKey, node_label};
use anyhow::Result;
use brewery_daemon_client::{
    ConnectionInfo, DB_AUTH_FAILED, DaemonState, DbNodeKind, IntrospectParams, ProjectDaemon,
    ProjectDaemonEvent, daemon_error_code, daemon_error_message,
};
use gpui::{
    Action, AnyWindowHandle, App, AsyncWindowContext, ClickEvent, ClipboardItem, Context, Entity,
    EventEmitter, FocusHandle, Focusable, Pixels, PromptLevel, Render, ScrollStrategy,
    SharedString, Subscription, Task, UniformListScrollHandle, WeakEntity, Window, px,
    uniform_list,
};
use project::Project;
use settings::{RegisterSetting, Settings};
use std::ops::Range;
use std::sync::Arc;
use ui::{
    ContextMenu, Disclosure, ListItem, ListItemSpacing, Tooltip, prelude::*, right_click_menu,
};
use workspace::dock::{DockPosition, Panel, PanelEvent};
use workspace::{OpenLog, Workspace};

const DATABASE_PANEL_KEY: &str = "DatabasePanel";

pub const WAITING_FOR_DAEMON: &str = "Waiting for the Brewery daemon…";
/// Rows Select Top shows of a table, as IntelliJ's "Select Top 500".
pub const SELECT_TOP_ROWS: u32 = 500;
/// Shown in the data source dialog when the daemon could not sign in.
pub const ENTER_PASSWORD: &str = "Authentication failed: enter the password";

/// `database_panel` in the settings.
#[derive(Debug, Clone, Copy, PartialEq, RegisterSetting)]
pub struct DatabasePanelSettings {
    pub dock: settings::DockPosition,
}

impl Settings for DatabasePanelSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        Self {
            // Falls back like `assets/settings/default.json` for builds that read another
            // checkout's defaults.
            dock: content
                .database_panel
                .as_ref()
                .and_then(|panel| panel.dock)
                .unwrap_or(settings::DockPosition::Left),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Status {
    /// Not connected yet, or the daemon is starting.
    Waiting,
    Ready,
    Failed(SharedString),
}

pub struct DatabasePanel {
    workspace: WeakEntity<Workspace>,
    /// Held directly because `Panel` callbacks such as `set_active` run while the workspace is
    /// being updated, when reading the workspace entity panics.
    project: Entity<Project>,
    focus_handle: FocusHandle,
    scroll_handle: UniformListScrollHandle,
    /// The rows the list built last.
    last_rendered: Option<Range<usize>>,
    daemon: Option<Entity<ProjectDaemon>>,
    _daemon_events: Option<Subscription>,
    status: Status,
    tree: DbTree,
    /// The list of data sources has been fetched at least once from the current daemon.
    loaded: bool,
    list_task: Option<Task<()>>,
    /// The last failure of a panel-wide action (listing, deleting), shown under the toolbar.
    notice: Option<SharedString>,
    selected: Option<RowKey>,
    /// Selected once the list that contains it arrives (after a save).
    select_after_list: Option<String>,
    /// Also list system schemas and tables (`pg_catalog`, `information_schema`, `sqlite_*`).
    show_system_objects: bool,
    /// The panel's window, for dialogs the panel opens by itself (the password prompt).
    window: AnyWindowHandle,
}

impl DatabasePanel {
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
        Self {
            window: window.window_handle(),
            workspace,
            project,
            focus_handle: cx.focus_handle(),
            scroll_handle: UniformListScrollHandle::new(),
            last_rendered: None,
            daemon: None,
            _daemon_events: None,
            status: Status::Waiting,
            tree: DbTree::default(),
            loaded: false,
            list_task: None,
            notice: None,
            selected: None,
            select_after_list: None,
            show_system_objects: false,
        }
    }

    pub fn show_system_objects(&self) -> bool {
        self.show_system_objects
    }

    /// Lists the open parts of every data source again, with or without the system objects.
    pub fn set_show_system_objects(&mut self, show: bool, cx: &mut Context<Self>) {
        if self.show_system_objects == show {
            return;
        }
        self.show_system_objects = show;
        for fetch in self.tree.refresh_all() {
            self.introspect(fetch, cx);
        }
        cx.notify();
    }

    pub fn last_rendered_rows(&self) -> Option<Range<usize>> {
        self.last_rendered.clone()
    }

    pub fn tree(&self) -> &DbTree {
        &self.tree
    }

    /// The last failure of a panel-wide action, shown under the toolbar.
    pub fn notice(&self) -> Option<&SharedString> {
        self.notice.as_ref()
    }

    pub fn selected(&self) -> Option<&RowKey> {
        self.selected.as_ref()
    }

    pub fn select(&mut self, key: Option<RowKey>, cx: &mut Context<Self>) {
        self.selected = key;
        cx.notify();
    }

    /// The data source of the selected row (the row itself or the one it belongs to).
    pub fn selected_connection(&self) -> Option<&ConnectionInfo> {
        let key = self.selected.as_ref()?;
        self.tree.source(&key.connection_id)
    }

    /// The project's daemon, once the panel has been shown.
    pub fn daemon(&self) -> Option<&Entity<ProjectDaemon>> {
        self.daemon.as_ref()
    }

    /// Only touches the project and globals, so it is safe from `set_active`.
    fn connect(&mut self, cx: &mut Context<Self>) {
        let daemon = ProjectDaemon::for_project(&self.project, cx);
        self._daemon_events = Some(cx.subscribe(&daemon, Self::on_daemon_event));
        let state = daemon.read(cx).state().clone();
        self.daemon = Some(daemon);
        match state {
            DaemonState::Connecting => self.status = Status::Waiting,
            DaemonState::Ready => {
                self.status = Status::Ready;
                self.fetch_connections(cx);
            }
            DaemonState::Failed { error, .. } => self.status = Status::Failed(error),
        }
        cx.notify();
    }

    fn on_daemon_event(
        &mut self,
        daemon: Entity<ProjectDaemon>,
        event: &ProjectDaemonEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            ProjectDaemonEvent::Connecting => {
                self.status = Status::Waiting;
                self.list_task = None;
            }
            ProjectDaemonEvent::Ready => {
                self.status = Status::Ready;
                // A restarted daemon has no open sessions; list everything that is open again.
                if self.loaded {
                    for fetch in self.tree.refresh_all() {
                        self.introspect(fetch, cx);
                    }
                }
                self.fetch_connections(cx);
            }
            ProjectDaemonEvent::ProjectChanged | ProjectDaemonEvent::SpringChanged => {}
            ProjectDaemonEvent::Failed { .. } => {
                if let DaemonState::Failed { error, .. } = daemon.read(cx).state().clone() {
                    self.status = Status::Failed(error);
                    self.list_task = None;
                }
            }
        }
        cx.notify();
    }

    fn ready_daemon(&self, cx: &App) -> Option<Entity<ProjectDaemon>> {
        self.daemon
            .clone()
            .filter(|daemon| daemon.read(cx).is_ready())
    }

    pub fn fetch_connections(&mut self, cx: &mut Context<Self>) {
        let Some(daemon) = self.ready_daemon(cx) else {
            return;
        };
        let list = daemon.read(cx).db_connections(cx);
        self.list_task = Some(cx.spawn(async move |this, cx| {
            let list = list.await;
            this.update(cx, |this, cx| {
                match list {
                    Ok(sources) => {
                        this.loaded = true;
                        this.notice = None;
                        this.tree.set_sources(sources);
                        if let Some(id) = this.select_after_list.take()
                            && this.tree.source(&id).is_some()
                        {
                            this.selected = Some(RowKey::data_source(id));
                        }
                        let selected_exists = this.selected.as_ref().is_some_and(|key| {
                            this.tree
                                .rows()
                                .iter()
                                .any(|row| row.key().as_ref() == Some(key))
                        });
                        if !selected_exists {
                            this.selected = None;
                        }
                    }
                    Err(error) => {
                        this.notice = Some(
                            format!(
                                "Could not list the data sources: {}",
                                daemon_error_message(&error)
                            )
                            .into(),
                        )
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn introspect(&mut self, fetch: Fetch, cx: &mut Context<Self>) {
        let Fetch { key, generation } = fetch;
        let Some(daemon) = self.ready_daemon(cx) else {
            // Failed rather than Loading, so expanding again retries.
            self.tree
                .set_children(&key, generation, Err(WAITING_FOR_DAEMON.to_string()));
            return;
        };
        let children = daemon.read(cx).db_introspect(
            IntrospectParams {
                connection_id: key.connection_id.clone(),
                path: key.path.clone(),
                include_system: self.show_system_objects.then_some(true),
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            let children = children.await;
            let auth_failed = children
                .as_ref()
                .err()
                .is_some_and(|error| daemon_error_code(error) == Some(DB_AUTH_FAILED.into()));
            this.update(cx, |this, cx| {
                this.tree.set_children(
                    &key,
                    generation,
                    children.map_err(|error| daemon_error_message(&error)),
                );
                if auth_failed {
                    this.ask_for_password(&key.connection_id, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The daemon could not sign in to the data source: opens its dialog at the password, as
    /// IntelliJ asks for it, unless a data source dialog is open already.
    fn ask_for_password(&mut self, connection_id: &str, cx: &mut Context<Self>) {
        let Some(source) = self.tree.source(connection_id).cloned() else {
            return;
        };
        let panel = cx.weak_entity();
        // Opening a modal needs the window, and the workspace must not be in an update.
        let workspace = self.workspace.clone();
        let window = self.window;
        cx.defer(move |cx| {
            let dialog_open = workspace
                .read_with(cx, |workspace, cx| {
                    workspace.active_modal::<DataSourceModal>(cx).is_some()
                })
                .unwrap_or(true);
            if dialog_open {
                return;
            }
            window
                .update(cx, |_, window, cx| {
                    panel
                        .update(cx, |panel, cx| {
                            panel.open_modal_asking(Some(source), Some(ENTER_PASSWORD), window, cx)
                        })
                        .ok();
                })
                .ok();
        });
    }

    pub fn toggle(&mut self, key: RowKey, cx: &mut Context<Self>) {
        if let Some(fetch) = self.tree.toggle(key) {
            self.introspect(fetch, cx);
        }
        cx.notify();
    }

    /// Lists the data sources again and drops the cached schema of the selected one, or of all of
    /// them when nothing is selected.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if matches!(self.status, Status::Failed(_)) {
            if let Some(daemon) = self.daemon.clone() {
                daemon.update(cx, |daemon, cx| daemon.reload(cx));
            }
            return;
        }
        let selected = self.selected.as_ref().map(|key| key.connection_id.clone());
        // SQL completion in the consoles relies on the same schema.
        if let Some(daemon) = self.daemon.clone() {
            let ids: Vec<String> = match &selected {
                Some(id) => vec![id.clone()],
                None => self
                    .tree
                    .sources()
                    .iter()
                    .map(|source| source.id.clone())
                    .collect(),
            };
            for id in ids {
                SchemaCache::refresh_connection(&daemon, &id, cx);
            }
        }
        let fetches = match selected {
            Some(id) => self.tree.refresh(&id),
            None => self.tree.refresh_all(),
        };
        for fetch in fetches {
            self.introspect(fetch, cx);
        }
        self.fetch_connections(cx);
        cx.notify();
    }

    fn project_root(&self, cx: &App) -> Option<String> {
        let worktree = self.project.read(cx).visible_worktrees(cx).next()?;
        Some(worktree.read(cx).abs_path().to_string_lossy().into_owned())
    }

    fn open_modal(
        &mut self,
        editing: Option<ConnectionInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_modal_asking(editing, None, window, cx);
    }

    /// With `password_prompt`, the dialog opens at the password field, saying why.
    fn open_modal_asking(
        &mut self,
        editing: Option<ConnectionInfo>,
        password_prompt: Option<&'static str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(daemon) = self.ready_daemon(cx) else {
            return;
        };
        let panel = cx.weak_entity();
        let project_root = self.project_root(cx);
        let workspace = self.workspace.clone();
        workspace
            .update(cx, |workspace, cx| {
                let weak = workspace.weak_handle();
                workspace.toggle_modal(window, cx, move |window, cx| {
                    let mut modal = DataSourceModal::new(
                        weak,
                        panel,
                        daemon,
                        project_root,
                        editing,
                        window,
                        cx,
                    );
                    if let Some(prompt) = password_prompt {
                        modal.ask_for_password(prompt, window, cx);
                    }
                    modal
                });
            })
            .ok();
    }

    pub fn add_data_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_modal(None, window, cx);
    }

    pub fn edit_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(source) = self.selected_connection().cloned() {
            self.open_modal(Some(source), window, cx);
        }
    }

    /// Called by the dialog once the daemon stored the data source.
    pub fn data_source_saved(&mut self, saved: ConnectionInfo, cx: &mut Context<Self>) {
        // Open consoles of this data source show its new name. Deferred: the dialog that calls
        // this may still be in an update of the workspace's modal layer.
        let workspace = self.workspace.clone();
        let renamed = saved.clone();
        cx.defer(move |cx| {
            workspace
                .update(cx, |workspace, cx| {
                    let consoles: Vec<_> = workspace.items_of_type::<QueryConsole>(cx).collect();
                    for console in consoles {
                        console.update(cx, |console, cx| console.connection_saved(&renamed, cx));
                    }
                })
                .ok();
        });
        // The URL or the credentials may have changed: what was listed may no longer be right.
        if let Some(daemon) = self.daemon.clone() {
            SchemaCache::refresh_connection(&daemon, &saved.id, cx);
        }
        for fetch in self.tree.refresh(&saved.id) {
            self.introspect(fetch, cx);
        }
        self.select_after_list = Some(saved.id);
        self.fetch_connections(cx);
    }

    pub fn delete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = self.selected_connection().cloned() else {
            return;
        };
        let Some(daemon) = self.ready_daemon(cx) else {
            return;
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Delete data source \"{}\"?", source.name),
            Some("Its settings and stored password are removed from this project."),
            &["Delete", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await.ok() != Some(0) {
                return;
            }
            let id = source.id.clone();
            let delete = cx.update(|cx| daemon.read(cx).db_delete_connection(id, cx));
            let result = delete.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.selected = None;
                        this.fetch_connections(cx);
                    }
                    Err(error) => {
                        this.notice = Some(
                            format!(
                                "Could not delete \"{}\": {}",
                                source.name,
                                daemon_error_message(&error)
                            )
                            .into(),
                        )
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Opens a query console for the selected data source.
    pub fn new_query_console(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(connection) = self.selected_connection().cloned() {
            self.open_console(connection, None, window, cx);
        }
    }

    fn open_console(
        &mut self,
        connection: ConnectionInfo,
        run: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace = self.workspace.clone();
        let daemon = self.daemon.clone();
        // Adding the tab moves focus and may call back into the dock, so not while the panel is
        // being updated.
        window.defer(cx, move |window, cx| {
            workspace
                .update(cx, |workspace, cx| {
                    open_console(workspace, connection, daemon, run, window, cx);
                })
                .ok();
        });
    }

    /// The selected table or view node: its data source and path.
    fn selected_table(&self) -> Option<(ConnectionInfo, Vec<String>)> {
        let key = self.selected.as_ref()?;
        let table = self.rows().into_iter().any(|row| match row {
            Row::Node {
                connection_id,
                node,
                ..
            } => {
                connection_id == key.connection_id
                    && node.path == key.path
                    && matches!(node.kind, DbNodeKind::Table | DbNodeKind::View)
            }
            _ => false,
        });
        let connection = self.tree.source(&key.connection_id)?.clone();
        table.then(|| (connection, key.path.clone()))
    }

    /// Opens a console running `SELECT * ... LIMIT 500` on the selected table or view.
    pub fn select_top(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((connection, path)) = self.selected_table() {
            let sql = select_top_from(&connection.driver_id, &path, SELECT_TOP_ROWS);
            self.open_console(connection, Some(sql), window, cx);
        }
    }

    /// Copies the selected node's name: tables and views with their schema, quoted as SQL needs.
    pub fn copy_name(&mut self, cx: &mut Context<Self>) {
        let text = match self.selected_table() {
            Some((connection, path)) => qualified_name(&connection.driver_id, &path),
            None => {
                let Some(key) = self.selected.as_ref() else {
                    return;
                };
                let Some(connection) = self.tree.source(&key.connection_id) else {
                    return;
                };
                match key.path.last() {
                    Some(name) => quote_identifier(&connection.driver_id, name),
                    None => connection.name.clone(),
                }
            }
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    /// A double click: tables and views open a console showing their rows, other rows expand or
    /// collapse.
    pub fn open_row(&mut self, key: RowKey, window: &mut Window, cx: &mut Context<Self>) {
        let table = self.rows().into_iter().find_map(|row| match row {
            Row::Node {
                connection_id,
                node,
                ..
            } if connection_id == key.connection_id
                && node.path == key.path
                && matches!(node.kind, DbNodeKind::Table | DbNodeKind::View) =>
            {
                Some(node.path)
            }
            _ => None,
        });
        match (table, self.tree.source(&key.connection_id).cloned()) {
            (Some(path), Some(connection)) => {
                let sql = select_all_from(&connection.driver_id, &path);
                self.open_console(connection, Some(sql), window, cx);
            }
            _ => self.activate(key, cx),
        }
    }

    fn rows(&self) -> Vec<Row> {
        self.tree.rows()
    }

    fn activate(&mut self, key: RowKey, cx: &mut Context<Self>) {
        let expandable = if key.is_data_source() {
            true
        } else {
            self.rows().iter().any(|row| match row {
                Row::Node {
                    connection_id,
                    node,
                    ..
                } => {
                    *connection_id == key.connection_id
                        && node.path == key.path
                        && node.has_children
                }
                _ => false,
            })
        };
        if expandable {
            self.toggle(key, cx);
        }
    }

    fn select_by(&mut self, forward: bool, cx: &mut Context<Self>) {
        let keys: Vec<RowKey> = self.rows().iter().filter_map(Row::key).collect();
        let current = self
            .selected
            .as_ref()
            .and_then(|selected| keys.iter().position(|key| key == selected));
        let next = match current {
            None if forward => keys.first(),
            None => keys.last(),
            Some(ix) if forward => keys.get(ix + 1).or(keys.get(ix)),
            Some(ix) => keys.get(ix.saturating_sub(1)),
        };
        if let Some(next) = next.cloned() {
            if let Some(ix) = self
                .rows()
                .iter()
                .position(|row| row.key() == Some(next.clone()))
            {
                self.scroll_handle
                    .scroll_to_item(ix, ScrollStrategy::Nearest);
            }
            self.selected = Some(next);
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

    fn confirm(&mut self, _: &menu::Confirm, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(key) = self.selected.clone() {
            self.activate(key, cx);
        }
    }

    fn on_row_click(
        &mut self,
        key: RowKey,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        self.selected = Some(key.clone());
        // A double click acts once, not twice.
        if event.click_count() == 2 {
            self.open_row(key, window, cx);
        }
        cx.notify();
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = self.status == Status::Ready;
        let has_source = ready && self.selected_connection().is_some();
        let button = |id: &'static str, icon: IconName, tooltip: &'static str, enabled: bool| {
            IconButton::new(id, icon)
                .icon_size(IconSize::Small)
                .tooltip(Tooltip::text(tooltip))
                .disabled(!enabled)
        };
        h_flex()
            .h(px(32.))
            .px_2()
            .gap_1()
            .border_b_1()
            .border_color(cx.theme().colors().border)
            .child(Label::new("Database").size(LabelSize::Small))
            .child(div().flex_1())
            .child(
                button("db-add", IconName::Plus, "Add Data Source", ready)
                    .on_click(cx.listener(|this, _, window, cx| this.add_data_source(window, cx))),
            )
            .child(
                button("db-edit", IconName::Pencil, "Edit Data Source", has_source)
                    .on_click(cx.listener(|this, _, window, cx| this.edit_selected(window, cx))),
            )
            .child(
                button(
                    "db-delete",
                    IconName::Trash,
                    "Delete Data Source",
                    has_source,
                )
                .on_click(cx.listener(|this, _, window, cx| this.delete_selected(window, cx))),
            )
            .child(
                button(
                    "db-refresh",
                    IconName::RotateCw,
                    "Refresh",
                    ready || matches!(self.status, Status::Failed(_)),
                )
                .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
            .child(
                button(
                    "db-new-console",
                    IconName::Terminal,
                    "New Query Console",
                    has_source,
                )
                .on_click(cx.listener(|this, _, window, cx| this.new_query_console(window, cx))),
            )
    }

    fn render_status(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        match &self.status {
            Status::Ready => None,
            Status::Waiting => Some(
                div()
                    .p_2()
                    .child(
                        Label::new(WAITING_FOR_DAEMON)
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .into_any_element(),
            ),
            Status::Failed(error) => Some(
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
                            .child(Label::new("The Brewery daemon is not running")),
                    )
                    .child(
                        Label::new(error.clone())
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .child(
                        Button::new("db-reload", "Reload")
                            .style(ButtonStyle::Filled)
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    )
                    .child(
                        Button::new("db-open-log", "Open Log").on_click(|_, window, cx| {
                            window.dispatch_action(OpenLog.boxed_clone(), cx)
                        }),
                    )
                    .into_any_element(),
            ),
        }
    }

    fn data_source_menu(
        window: &mut Window,
        cx: &mut App,
        panel: WeakEntity<Self>,
    ) -> Entity<ContextMenu> {
        ContextMenu::build(window, cx, move |menu, _, cx| {
            let entry =
                |menu: ContextMenu,
                 label: &'static str,
                 action: fn(&mut Self, &mut Window, &mut Context<Self>)| {
                    let panel = panel.clone();
                    menu.entry(label, None, move |window, cx| {
                        panel.update(cx, |panel, cx| action(panel, window, cx)).ok();
                    })
                };
            let menu = entry(menu, "New Query Console", Self::new_query_console);
            let menu = entry(menu.separator(), "Add Data Source…", Self::add_data_source);
            let menu = entry(menu, "Edit Data Source…", Self::edit_selected);
            let menu = entry(menu, "Delete Data Source", Self::delete_selected);
            let menu = entry(menu.separator(), "Refresh", |panel, _, cx| {
                panel.refresh(cx)
            });
            Self::system_objects_entry(menu, &panel, cx)
        })
    }

    /// The menu of a schema node; tables and views also get Select Top and the qualified name.
    fn node_menu(
        window: &mut Window,
        cx: &mut App,
        panel: WeakEntity<Self>,
        table: bool,
    ) -> Entity<ContextMenu> {
        ContextMenu::build(window, cx, move |menu, _, cx| {
            let entry =
                |menu: ContextMenu,
                 label: &'static str,
                 action: fn(&mut Self, &mut Window, &mut Context<Self>)| {
                    let panel = panel.clone();
                    menu.entry(label, None, move |window, cx| {
                        panel.update(cx, |panel, cx| action(panel, window, cx)).ok();
                    })
                };
            let menu = entry(menu, "New Query Console", Self::new_query_console);
            let menu = if table {
                let menu = entry(menu, "Select Top 500", Self::select_top);
                entry(menu, "Copy Qualified Name", |panel, _, cx| {
                    panel.copy_name(cx)
                })
            } else {
                entry(menu, "Copy Name", |panel, _, cx| panel.copy_name(cx))
            };
            let menu = entry(menu.separator(), "Refresh", |panel, _, cx| {
                panel.refresh(cx)
            });
            Self::system_objects_entry(menu, &panel, cx)
        })
    }

    fn system_objects_entry(menu: ContextMenu, panel: &WeakEntity<Self>, cx: &App) -> ContextMenu {
        let shown = panel
            .read_with(cx, |panel, _| panel.show_system_objects)
            .unwrap_or(false);
        let panel = panel.clone();
        menu.separator().toggleable_entry(
            "Show System Objects",
            shown,
            IconPosition::Start,
            None,
            move |_, cx| {
                panel
                    .update(cx, |panel, cx| panel.set_show_system_objects(!shown, cx))
                    .ok();
            },
        )
    }

    fn row_icon(row: &Row) -> IconName {
        match row {
            Row::DataSource { .. } => IconName::DatabaseZap,
            Row::Node { node, .. } => match node.kind {
                DbNodeKind::Catalog | DbNodeKind::Schema => IconName::ListTree,
                DbNodeKind::Table => IconName::Table,
                DbNodeKind::View => IconName::Eye,
                DbNodeKind::Sequence => IconName::Hash,
                DbNodeKind::Folder => IconName::Folder,
                DbNodeKind::Column => IconName::Dash,
                DbNodeKind::Index => IconName::Bookmark,
                DbNodeKind::ForeignKey => IconName::Link,
                DbNodeKind::Other => IconName::Circle,
            },
            Row::Loading { .. } | Row::Empty { .. } | Row::Error { .. } => IconName::Circle,
        }
    }

    /// The tree rows in `range` (clamped to the rows there are); the list renders only those
    /// that are on screen.
    pub(crate) fn render_rows(
        &self,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let panel = cx.weak_entity();
        self.rows()
            .into_iter()
            .enumerate()
            .skip(range.start)
            .take(range.len())
            .map(|(ix, row)| {
                let icon = Self::row_icon(&row);
                match &row {
                    Row::DataSource {
                        connection,
                        expanded: _,
                        error,
                    } => {
                        let key = RowKey::data_source(connection.id.clone());
                        let selected = self.selected.as_ref() == Some(&key);
                        let item = self
                            .tree_item(ix, key, 0, row_chevron(&row), icon, selected, cx)
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(Label::new(connection.name.clone()))
                                    .child(
                                        Label::new(driver_label(&connection.driver_id))
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                    ),
                            )
                            .when_some(error.clone(), |item, error| {
                                item.end_slot(
                                    Icon::new(IconName::Warning)
                                        .size(IconSize::Small)
                                        .color(Color::Error),
                                )
                                .tooltip(Tooltip::text(error))
                            });
                        let panel = panel.clone();
                        let source_key = RowKey::data_source(connection.id.clone());
                        right_click_menu(("db-source-menu", ix))
                            .trigger(move |_, _, _| item)
                            .menu(move |window, cx| {
                                // The menu acts on the row it was opened on.
                                let key = source_key.clone();
                                panel
                                    .update(cx, |panel, cx| panel.select(Some(key), cx))
                                    .ok();
                                Self::data_source_menu(window, cx, panel.clone())
                            })
                            .into_any_element()
                    }
                    Row::Node {
                        connection_id,
                        node,
                        depth,
                        expanded: _,
                    } => {
                        let key = RowKey::new(connection_id.clone(), node.path.clone());
                        let selected = self.selected.as_ref() == Some(&key);
                        let item = self
                            .tree_item(
                                ix,
                                key.clone(),
                                *depth,
                                row_chevron(&row),
                                icon,
                                selected,
                                cx,
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(Label::new(node_label(node).to_string()))
                                    .children(node.detail.clone().map(|detail| {
                                        Label::new(detail)
                                            .size(LabelSize::Small)
                                            .color(Color::Muted)
                                    })),
                            );
                        let panel = panel.clone();
                        let table = matches!(node.kind, DbNodeKind::Table | DbNodeKind::View);
                        right_click_menu(("db-node-menu", ix))
                            .trigger(move |_, _, _| item)
                            .menu(move |window, cx| {
                                // The menu acts on the row it was opened on.
                                let key = key.clone();
                                panel
                                    .update(cx, |panel, cx| panel.select(Some(key), cx))
                                    .ok();
                                Self::node_menu(window, cx, panel.clone(), table)
                            })
                            .into_any_element()
                    }
                    Row::Loading { depth, .. } => {
                        message_item(("db-loading", ix), *depth, "Loading…", Color::Muted)
                            .into_any_element()
                    }
                    Row::Empty { depth, .. } => {
                        message_item(("db-empty", ix), *depth, "Empty", Color::Muted)
                            .into_any_element()
                    }
                    Row::Error { depth, message, .. } => {
                        message_item(("db-error", ix), *depth, message, Color::Error)
                            .into_any_element()
                    }
                }
            })
            .collect()
    }

    fn tree_item(
        &self,
        ix: usize,
        key: RowKey,
        depth: usize,
        chevron: Option<IconName>,
        icon: IconName,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> ListItem {
        // ListItem's own disclosure sits left of the row (outside it at depth 0) and hides
        // when expanded, so the chevron is drawn in the row. Leaves keep the same width empty.
        let disclosure = div()
            .w(px(16.))
            .flex_none()
            .children(chevron.map(|chevron| {
                let toggled = key.clone();
                let on_toggle: Arc<dyn Fn(&ClickEvent, &mut Window, &mut App)> =
                    Arc::new(cx.listener(move |this, _, _, cx| this.toggle(toggled.clone(), cx)));
                Disclosure::new(("db-disclosure", ix), chevron == IconName::ChevronDown)
                    .on_toggle_expanded(on_toggle)
            }));
        ListItem::new(("db-row", ix))
            .indent_level(depth)
            .spacing(ListItemSpacing::Dense)
            .toggle_state(selected)
            .start_slot(
                h_flex()
                    .gap_0p5()
                    .child(disclosure)
                    .child(Icon::new(icon).size(IconSize::Small).color(Color::Muted)),
            )
            .on_click(cx.listener(move |this, event, window, cx| {
                this.on_row_click(key.clone(), event, window, cx)
            }))
    }
}

fn message_item(id: (&'static str, usize), depth: usize, text: &str, color: Color) -> ListItem {
    ListItem::new(id)
        .indent_level(depth)
        .spacing(ListItemSpacing::Dense)
        .selectable(false)
        // Lines up with the icons of the sibling rows, behind the chevron column.
        .start_slot(div().w(px(16.)))
        .child(
            Label::new(text.to_string())
                .size(LabelSize::Small)
                .color(color),
        )
}

/// The disclosure chevron of a row: right when collapsed, down when expanded, none for leaves.
pub fn row_chevron(row: &Row) -> Option<IconName> {
    let expanded = match row {
        Row::DataSource { expanded, .. } => *expanded,
        Row::Node { node, expanded, .. } if node.has_children => *expanded,
        Row::Node { .. } | Row::Loading { .. } | Row::Empty { .. } | Row::Error { .. } => {
            return None;
        }
    };
    Some(if expanded {
        IconName::ChevronDown
    } else {
        IconName::ChevronRight
    })
}

/// A short driver label shown after the data source name.
pub fn driver_label(driver_id: &str) -> &str {
    match driver_id {
        "postgresql" => "PostgreSQL",
        "mysql" => "MySQL",
        "mariadb" => "MariaDB",
        "sqlite" => "SQLite",
        other => other,
    }
}

impl Render for DatabasePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body =
            match self.render_status(cx) {
                Some(status) => status,
                None if self.loaded && self.tree.sources().is_empty() => v_flex()
                    .p_2()
                    .gap_2()
                    .child(
                        Label::new("No data sources")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .child(Button::new("db-empty-add", "Add Data Source…").on_click(
                        cx.listener(|this, _, window, cx| this.add_data_source(window, cx)),
                    ))
                    .into_any_element(),
                // Uniform `ListItem`s: only the rows on screen are built, so a schema with
                // thousands of tables stays quick.
                None => uniform_list(
                    "db-panel-rows",
                    self.tree.rows().len(),
                    cx.processor(|this, range: Range<usize>, _window, cx| {
                        // `uniform_list` first renders row 0 alone to measure it.
                        if range != (0..1) || this.tree.rows().len() == 1 {
                            this.last_rendered = Some(range.clone());
                        }
                        this.render_rows(range, cx)
                    }),
                )
                .flex_1()
                .track_scroll(&self.scroll_handle)
                .into_any_element(),
            };
        v_flex()
            .key_context("DatabasePanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().colors().panel_background)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::confirm))
            .child(self.render_toolbar(cx))
            .children(self.notice.clone().map(|notice| {
                div().px_2().py_1().child(
                    Label::new(notice)
                        .size(LabelSize::Small)
                        .color(Color::Error),
                )
            }))
            .child(body)
    }
}

impl Focusable for DatabasePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for DatabasePanel {}

impl Panel for DatabasePanel {
    fn persistent_name() -> &'static str {
        "Database Panel"
    }

    fn panel_key() -> &'static str {
        DATABASE_PANEL_KEY
    }

    /// Read from the settings: the dock moves a panel when the settings change, so a position kept
    /// only in the panel would never move it.
    fn position(&self, _: &Window, cx: &App) -> DockPosition {
        DatabasePanelSettings::get_global(cx).dock.into()
    }

    fn position_is_valid(&self, _: DockPosition) -> bool {
        true
    }

    fn set_position(&mut self, position: DockPosition, _: &mut Window, cx: &mut Context<Self>) {
        let fs = self.project.read(cx).fs().clone();
        settings::update_settings_file(fs, cx, move |settings, _| {
            settings.database_panel.get_or_insert_default().dock = Some(position.into());
        });
    }

    fn default_size(&self, _: &Window, _: &App) -> Pixels {
        px(300.)
    }

    fn icon(&self, _: &Window, _: &App) -> Option<IconName> {
        Some(IconName::DatabaseZap)
    }

    fn icon_tooltip(&self, _: &Window, _: &App) -> Option<&'static str> {
        Some("Database")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleDatabasePanel)
    }

    fn set_active(&mut self, active: bool, _: &mut Window, cx: &mut Context<Self>) {
        // The JVM is only worth starting once someone looks at the panel.
        if active && self.daemon.is_none() {
            self.connect(cx);
        }
    }

    fn activation_priority(&self) -> u32 {
        10
    }
}

#[cfg(test)]
#[path = "panel_tests.rs"]
mod tests;
