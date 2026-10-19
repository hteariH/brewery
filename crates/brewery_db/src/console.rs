//! The query console: a workspace tab with an SQL editor bound to one data source and, below it,
//! the result of the last statement (a paged grid, an update count or the daemon's error).

use crate::completion::{Dialect, SqlCompletionProvider};
use crate::console_store::{self, QueryConsoleDb, SAVE_DELAY, SavedConsole};
use crate::panel::WAITING_FOR_DAEMON;
use crate::result_grid::ResultGrid;
use crate::schema_cache::SchemaCache;
use crate::sql_context::is_schema_change;
use crate::statements::statements_to_run;
use crate::{CancelStatement, RunStatement};
use anyhow::Context as _;
use brewery_daemon_client::{
    CancelParams, ConnectionInfo, ExecuteParams, ExecuteResult, ProjectDaemon, daemon_error_message,
};
use editor::actions::ShowCompletions;
use editor::{Editor, MultiBufferOffset};
use fs::Fs;
use futures::StreamExt as _;
use gpui::{
    AnyEntity, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Render, SharedString,
    Subscription, Task, WeakEntity, Window, px, relative,
};
use language::{Buffer, BufferEvent};
use project::Project;
use std::any::TypeId;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use ui::{Tooltip, prelude::*};
use workspace::item::{Item, ItemEvent};
use workspace::searchable::SearchableItemHandle;
use util::ResultExt as _;
use workspace::{ItemId, SerializableItem, Workspace, WorkspaceId, delete_unloaded_items};

pub const NOTHING_TO_RUN: &str = "Nothing to run";
/// How long a cancelled statement may keep the console busy before it is left to finish alone.
pub const CANCEL_GIVE_UP: std::time::Duration = std::time::Duration::from_secs(10);
pub const CANCEL_IGNORED: &str =
    "The statement did not stop; it is left to finish and its result will not be shown";
const SQL_LANGUAGE: &str = "SQL";
/// How the run key reads on this platform (see the keymap overrides).
const RUN_KEY: &str = if cfg!(target_os = "macos") {
    "Cmd+Enter"
} else {
    "Ctrl+Enter"
};

/// What the result area shows.
enum Outcome {
    Empty,
    Notice(SharedString),
    Rows {
        grid: Entity<ResultGrid>,
        elapsed_ms: u64,
        note: Option<SharedString>,
        _observation: Subscription,
    },
    Updated {
        count: i64,
        elapsed_ms: u64,
        note: Option<SharedString>,
    },
    Failed(SharedString),
}

pub struct QueryConsole {
    connection: ConnectionInfo,
    daemon: Entity<ProjectDaemon>,
    editor: Entity<Editor>,
    outcome: Outcome,
    /// `Some` while a statement runs; `true` once cancelling was asked for.
    running: Option<bool>,
    /// Numbers the runs, so the late answer of an abandoned statement is not shown.
    run_id: u64,
    /// Gives up waiting for a cancelled statement that keeps running.
    cancel_timeout: Option<Task<()>>,
    /// What SQL completion knows about the data source's schema.
    schema_cache: Entity<SchemaCache>,
    _schema_arrived: Subscription,
    /// The file that keeps the console's SQL (see `console_store`).
    file: PathBuf,
    fs: Arc<dyn Fs>,
    /// The file has been read; until then nothing is written, so it cannot be overwritten.
    loaded: bool,
    /// Edits not written to the file yet.
    unsaved: bool,
    save_task: Option<Task<()>>,
    _load_task: Task<()>,
    _buffer_edits: Subscription,
}

impl QueryConsole {
    /// A console backed by `file`, or by the first console file of the data source that no open
    /// console uses. What the file holds is put before any text the console already has.
    pub fn new(
        connection: ConnectionInfo,
        daemon: Entity<ProjectDaemon>,
        project: Entity<Project>,
        file: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let file = match file {
            Some(file) => {
                console_store::claim_file(&file, cx);
                file
            }
            None => console_store::claim_free_file(&connection.id, cx),
        };
        let fs = project.read(cx).fs().clone();
        let languages = project.read(cx).languages().clone();
        let buffer = cx.new(|cx| {
            let buffer = Buffer::local("", cx);
            buffer.set_language_registry(languages.clone());
            buffer
        });
        // Highlighting comes from the SQL extension. On a first launch it is still being
        // installed, so look again whenever the registry changes; plain text until then.
        buffer.update(cx, |_, cx| {
            let mut changes = languages.subscribe();
            cx.spawn(async move |buffer, cx| {
                loop {
                    if let Ok(sql) = languages.language_for_name(SQL_LANGUAGE).await {
                        buffer
                            .update(cx, |buffer, cx| buffer.set_language(Some(sql), cx))
                            .ok();
                        return;
                    }
                    if changes.next().await.is_none() || buffer.upgrade().is_none() {
                        return;
                    }
                }
            })
            .detach();
        });
        let buffer_edits = cx.subscribe(&buffer, |this, _, event: &BufferEvent, cx| {
            if matches!(event, BufferEvent::Edited { .. }) {
                this.schedule_save(cx);
            }
        });
        let load_task = {
            let fs = fs.clone();
            let file = file.clone();
            cx.spawn_in(window, async move |this, cx| {
                let text = fs.load(&file).await.ok();
                this.update_in(cx, |this, window, cx| this.file_loaded(text, window, cx))
                    .ok();
            })
        };
        let schema_cache = SchemaCache::for_connection(&daemon, &connection.id, cx);
        let completion =
            SqlCompletionProvider::new(schema_cache.clone(), Dialect::for_connection(&connection));
        let waiting = completion.waiting();
        let editor = cx.new(|cx| {
            let mut editor = Editor::for_buffer(buffer, Some(project), window, cx);
            editor.set_placeholder_text(
                &format!("SQL; {RUN_KEY} runs the statement at the cursor"),
                window,
                cx,
            );
            editor.set_completion_provider(Some(Rc::new(completion)));
            editor
        });
        // The first completion request only starts fetching the schema; ask again once it is
        // there, unless the user has moved on to another view.
        let schema_arrived = cx.observe_in(&schema_cache, window, move |this, _, window, cx| {
            if !waiting.replace(false) {
                return;
            }
            this.editor.update(cx, |editor, cx| {
                if editor.focus_handle(cx).is_focused(window) {
                    editor.show_completions(&ShowCompletions, window, cx);
                }
            });
        });
        // Closing the tab while a statement runs, or with a result open, must not leave either
        // behind in the daemon.
        cx.on_release(|this, cx| this.shut_down(cx)).detach();
        Self {
            connection,
            daemon,
            editor,
            outcome: Outcome::Empty,
            running: None,
            run_id: 0,
            cancel_timeout: None,
            schema_cache,
            _schema_arrived: schema_arrived,
            file,
            fs,
            loaded: false,
            unsaved: false,
            save_task: None,
            _load_task: load_task,
            _buffer_edits: buffer_edits,
        }
    }

    /// The file that keeps this console's SQL.
    pub fn file(&self) -> &Path {
        &self.file
    }

    fn file_loaded(&mut self, text: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.loaded = true;
        let Some(text) = text.filter(|text| !text.is_empty()) else {
            if self.unsaved {
                self.schedule_save(cx);
            }
            return;
        };
        self.editor.update(cx, |editor, cx| {
            if editor.text(cx).is_empty() {
                editor.set_text(text, window, cx);
                // That is what the file holds already.
                self.unsaved = false;
                self.save_task = None;
            } else {
                // Opened to run a statement (a double-clicked table): it stays below the old SQL.
                let text = format!("{text}\n\n");
                editor.buffer().update(cx, |buffer, cx| {
                    buffer.edit([(MultiBufferOffset(0)..MultiBufferOffset(0), text)], None, cx)
                });
            }
        });
    }

    /// Writes the SQL to the file shortly after the last edit.
    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        self.unsaved = true;
        if !self.loaded {
            return;
        }
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            let Some(write) = this.update(cx, |this, cx| this.write_file(cx)).ok().flatten() else {
                return;
            };
            write.await;
        }));
    }

    /// Starts writing the SQL to the file if there are unsaved edits.
    fn write_file(&mut self, cx: &mut App) -> Option<Task<()>> {
        if !self.loaded || !self.unsaved {
            return None;
        }
        // A pending `save_task` (maybe the caller) finds nothing to write any more.
        self.unsaved = false;
        let text = self.editor.read(cx).text(cx);
        let fs = self.fs.clone();
        let file = self.file.clone();
        Some(cx.background_spawn(async move {
            if let Some(dir) = file.parent() {
                fs.create_dir(dir).await.log_err();
            }
            fs.atomic_write(file, text).await.log_err();
        }))
    }

    pub fn connection(&self) -> &ConnectionInfo {
        &self.connection
    }

    /// The data source was saved again; a new name shows in the tab and the toolbar.
    pub fn connection_saved(&mut self, saved: &ConnectionInfo, cx: &mut Context<Self>) {
        if saved.id != self.connection.id || saved.name == self.connection.name {
            return;
        }
        self.connection.name = saved.name.clone();
        cx.emit(ConsoleEvent::TitleChanged);
        cx.notify();
    }

    pub fn editor(&self) -> &Entity<Editor> {
        &self.editor
    }

    pub fn grid(&self) -> Option<&Entity<ResultGrid>> {
        match &self.outcome {
            Outcome::Rows { grid, .. } => Some(grid),
            _ => None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    pub fn set_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |editor, cx| editor.set_text(text, window, cx));
    }

    /// Runs the selection, or the statement at the cursor.
    pub fn run(&mut self, _: &RunStatement, _: &mut Window, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        let (text, selection) = self.editor.update(cx, |editor, cx| {
            let snapshot = editor.display_snapshot(cx);
            let selection = editor.selections.newest::<MultiBufferOffset>(&snapshot);
            (editor.text(cx), selection.start.0..selection.end.0)
        });
        let statements = statements_to_run(&text, selection);
        let Some(first) = statements.first().and_then(|range| text.get(range.clone())) else {
            self.outcome = Outcome::Notice(NOTHING_TO_RUN.into());
            cx.notify();
            return;
        };
        let note = (statements.len() > 1).then(|| {
            SharedString::from(format!(
                "ran the first of {} selected statements",
                statements.len()
            ))
        });
        self.run_sql(first.to_string(), note, cx);
    }

    /// Runs `sql` as one statement. Ignored while another statement runs.
    pub fn run_sql(&mut self, sql: String, note: Option<SharedString>, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        if !self.daemon.read(cx).is_ready() {
            self.outcome = Outcome::Notice(WAITING_FOR_DAEMON.into());
            cx.notify();
            return;
        }
        self.close_cursor(cx);
        let changes_schema = is_schema_change(&sql);
        self.outcome = Outcome::Empty;
        self.running = Some(false);
        self.run_id += 1;
        let run_id = self.run_id;
        let execute = self.daemon.read(cx).db_execute(
            ExecuteParams {
                connection_id: self.connection.id.clone(),
                sql,
            },
            cx,
        );
        let daemon = self.daemon.clone();
        let connection_id = self.connection.id.clone();
        // Detached rather than owned, so that a cursor opened after the tab was closed (or after
        // the run was abandoned) is still closed.
        cx.spawn(async move |this, cx| {
            let result = execute.await;
            let cursor = result
                .as_ref()
                .ok()
                .and_then(|result| result.cursor_id.clone());
            // The schema changed even when nobody waits for this run any more.
            if changes_schema && result.is_ok() {
                cx.update(|cx| SchemaCache::refresh_connection(&daemon, &connection_id, cx));
            }
            let shown = this
                .update(cx, |this, cx| this.finish_run(run_id, result, note, cx))
                .unwrap_or(false);
            if !shown && let Some(cursor) = cursor {
                let close = cx.update(|cx| daemon.read(cx).db_close(cursor, cx));
                close.await.ok();
            }
        })
        .detach();
        cx.notify();
    }

    /// Shows the answer of run `run_id`; `false` when that run was abandoned meanwhile.
    fn finish_run(
        &mut self,
        run_id: u64,
        result: anyhow::Result<ExecuteResult>,
        note: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> bool {
        if run_id != self.run_id || self.running.is_none() {
            return false;
        }
        self.running = None;
        self.cancel_timeout = None;
        if result.is_ok() {
            self.schema_cache
                .update(cx, |cache, _| cache.retry_after_failure());
        }
        self.outcome = match result {
            Ok(ExecuteResult {
                cursor_id: Some(cursor_id),
                columns,
                elapsed_ms,
                ..
            }) => {
                let daemon = self.daemon.clone();
                let grid = cx.new(|cx| ResultGrid::new(daemon, cursor_id, columns, cx));
                let observation = cx.observe(&grid, |_, _, cx| cx.notify());
                Outcome::Rows {
                    grid,
                    elapsed_ms,
                    note,
                    _observation: observation,
                }
            }
            Ok(result) => Outcome::Updated {
                count: result.update_count.unwrap_or_default(),
                elapsed_ms: result.elapsed_ms,
                note,
            },
            Err(error) => Outcome::Failed(daemon_error_message(&error).into()),
        };
        cx.notify();
        true
    }

    pub fn cancel(&mut self, _: &CancelStatement, _: &mut Window, cx: &mut Context<Self>) {
        if self.running != Some(false) {
            return;
        }
        self.running = Some(true);
        self.send_cancel(cx);
        // Some drivers ignore cancel; do not keep the console blocked until the statement ends.
        let run_id = self.run_id;
        self.cancel_timeout = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CANCEL_GIVE_UP).await;
            this.update(cx, |this, cx| {
                if this.run_id == run_id && this.running == Some(true) {
                    this.running = None;
                    this.outcome = Outcome::Notice(CANCEL_IGNORED.into());
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn send_cancel(&self, cx: &mut App) {
        self.daemon
            .read(cx)
            .db_cancel(
                CancelParams {
                    connection_id: Some(self.connection.id.clone()),
                    cursor_id: None,
                },
                cx,
            )
            .detach();
    }

    /// Closes the cursor of the result shown, if it is still open. The daemon may have dropped
    /// it already (expired); that answer does not matter.
    fn close_cursor(&mut self, cx: &mut App) {
        let Outcome::Rows { grid, .. } = &self.outcome else {
            return;
        };
        if let Some(cursor) = grid.update(cx, |grid, _| grid.take_cursor()) {
            self.daemon.read(cx).db_close(cursor, cx).detach();
        }
    }

    fn shut_down(&mut self, cx: &mut App) {
        if self.running == Some(false) {
            self.send_cancel(cx);
        }
        self.running = None;
        self.cancel_timeout = None;
        self.close_cursor(cx);
        // Closing the tab keeps the SQL: the next console of the data source opens it again.
        if let Some(write) = self.write_file(cx) {
            write.detach();
        }
        console_store::release_file(&self.file, cx);
    }

    /// The text of the status line and its colour.
    pub fn status_line(&self, cx: &App) -> (SharedString, Color) {
        let with_note = |text: String, note: &Option<SharedString>| -> SharedString {
            match note {
                Some(note) => format!("{text} ({note})").into(),
                None => text.into(),
            }
        };
        match (self.running, &self.outcome) {
            (Some(true), _) => ("Cancelling…".into(), Color::Muted),
            (Some(false), _) => ("Running…".into(), Color::Muted),
            (None, Outcome::Empty) => (
                format!("{RUN_KEY} runs the selection or the statement at the cursor").into(),
                Color::Muted,
            ),
            (None, Outcome::Notice(text)) => (text.clone(), Color::Muted),
            (
                None,
                Outcome::Rows {
                    grid,
                    elapsed_ms,
                    note,
                    ..
                },
            ) => {
                let pages = grid.read(cx).pages();
                let status = pages.status(*elapsed_ms);
                match pages.problem() {
                    Some(problem) => (
                        with_note(format!("{status} · {problem}"), note),
                        Color::Warning,
                    ),
                    None => (with_note(status, note), Color::Muted),
                }
            }
            (
                None,
                Outcome::Updated {
                    count,
                    elapsed_ms,
                    note,
                },
            ) => {
                let noun = if *count == 1 { "row" } else { "rows" };
                (
                    with_note(format!("{count} {noun} affected in {elapsed_ms} ms"), note),
                    Color::Muted,
                )
            }
            (None, Outcome::Failed(message)) => (message.clone(), Color::Error),
        }
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let running = self.running.is_some();
        h_flex()
            .h(px(32.))
            .flex_none()
            .px_2()
            .gap_1()
            .border_b_1()
            .border_color(cx.theme().colors().border)
            .child(
                IconButton::new("console-run", IconName::PlayFilled)
                    .icon_size(IconSize::Small)
                    .icon_color(Color::Success)
                    .disabled(running)
                    .tooltip(Tooltip::for_action_title("Run Statement", &RunStatement))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.run(&RunStatement, window, cx)),
                    ),
            )
            .child(
                IconButton::new("console-stop", IconName::Stop)
                    .icon_size(IconSize::Small)
                    .icon_color(if running { Color::Error } else { Color::Muted })
                    .disabled(self.running != Some(false))
                    .tooltip(Tooltip::for_action_title(
                        "Cancel Statement",
                        &CancelStatement,
                    ))
                    .on_click(
                        cx.listener(|this, _, window, cx| {
                            this.cancel(&CancelStatement, window, cx)
                        }),
                    ),
            )
            .child(div().flex_1())
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Icon::new(IconName::DatabaseZap)
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                    .child(
                        Label::new(self.connection.name.clone())
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
            )
    }
}

/// Opens a console for `connection` in the active pane; with `run`, puts that SQL in it and runs
/// it right away.
pub fn open_console(
    workspace: &mut Workspace,
    connection: ConnectionInfo,
    daemon: Option<Entity<ProjectDaemon>>,
    run: Option<String>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Entity<QueryConsole> {
    let project = workspace.project().clone();
    let daemon = daemon.unwrap_or_else(|| ProjectDaemon::for_project(&project, cx));
    let console = cx.new(|cx| QueryConsole::new(connection, daemon, project, None, window, cx));
    workspace.add_item_to_active_pane(Box::new(console.clone()), None, true, window, cx);
    if let Some(sql) = run {
        console.update(cx, |console, cx| {
            console.set_text(&sql, window, cx);
            console.run_sql(sql, None, cx);
        });
    }
    console
}

impl Render for QueryConsole {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (status, status_color) = self.status_line(cx);
        let grid = self.grid().cloned();
        let toolbar = self.render_toolbar(cx);
        let colors = cx.theme().colors();
        v_flex()
            .key_context("QueryConsole")
            .size_full()
            .bg(colors.editor_background)
            .on_action(cx.listener(Self::run))
            .on_action(cx.listener(Self::cancel))
            .child(toolbar)
            .child(div().flex_1().min_h(px(60.)).child(self.editor.clone()))
            .child(
                v_flex()
                    .h(relative(0.45))
                    .flex_none()
                    .border_t_1()
                    .border_color(colors.border)
                    .bg(colors.panel_background)
                    .child(
                        h_flex()
                            .h(px(26.))
                            .flex_none()
                            .px_2()
                            .border_b_1()
                            .border_color(colors.border_variant)
                            .child(
                                Label::new(status)
                                    .size(LabelSize::Small)
                                    .color(status_color)
                                    .single_line(),
                            ),
                    )
                    .child(div().flex_1().min_h_0().overflow_hidden().children(grid)),
            )
    }
}

impl Focusable for QueryConsole {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.focus_handle(cx)
    }
}

/// Reopens a console a workspace remembered, on its file.
pub fn restore_console(
    saved: SavedConsole,
    project: Entity<Project>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<QueryConsole> {
    let daemon = ProjectDaemon::for_project(&project, cx);
    cx.new(|cx| {
        QueryConsole::new(
            saved.connection,
            daemon,
            project,
            Some(saved.file),
            window,
            cx,
        )
    })
}

impl SerializableItem for QueryConsole {
    fn serialized_item_kind() -> &'static str {
        "BreweryQueryConsole"
    }

    fn cleanup(
        workspace_id: WorkspaceId,
        alive_items: Vec<ItemId>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<()>> {
        let db = QueryConsoleDb::global(cx);
        delete_unloaded_items(alive_items, workspace_id, "brewery_query_consoles", &db, cx)
    }

    fn deserialize(
        project: Entity<Project>,
        _workspace: WeakEntity<Workspace>,
        workspace_id: WorkspaceId,
        item_id: ItemId,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Entity<Self>>> {
        let db = QueryConsoleDb::global(cx);
        window.spawn(cx, async move |cx| {
            let saved = db
                .get_console(item_id, workspace_id)?
                .context("no saved query console")?;
            cx.update(|window, cx| restore_console(saved, project, window, cx))
        })
    }

    fn serialize(
        &mut self,
        workspace: &mut Workspace,
        item_id: ItemId,
        _closing: bool,
        cx: &mut Context<Self>,
    ) -> Option<Task<anyhow::Result<()>>> {
        // The app may be quitting: write what was typed in the last moments as well.
        if let Some(write) = self.write_file(cx) {
            write.detach();
        }
        let workspace_id = workspace.database_id()?;
        let saved = SavedConsole {
            connection: self.connection.clone(),
            file: self.file.clone(),
        };
        let db = QueryConsoleDb::global(cx);
        Some(cx.background_spawn(async move {
            db.save_console(item_id, workspace_id, saved).await
        }))
    }

    fn should_serialize(&self, event: &Self::Event) -> bool {
        // A renamed data source is reopened under its new name.
        matches!(event, ConsoleEvent::TitleChanged)
    }
}

/// Events of a console tab.
pub enum ConsoleEvent {
    /// The data source was renamed; the tab title follows.
    TitleChanged,
}

impl EventEmitter<ConsoleEvent> for QueryConsole {}

impl Item for QueryConsole {
    type Event = ConsoleEvent;

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(ItemEvent)) {
        match event {
            ConsoleEvent::TitleChanged => f(ItemEvent::UpdateTab),
        }
    }

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        format!("{} console", self.connection.name).into()
    }

    fn tab_icon(&self, _: &Window, _: &App) -> Option<Icon> {
        Some(Icon::new(IconName::DatabaseZap))
    }

    fn tab_tooltip_text(&self, _: &App) -> Option<SharedString> {
        Some(format!("Query console for {}", self.connection.name).into())
    }

    fn telemetry_event_text(&self) -> Option<&'static str> {
        None
    }

    /// Lets editor-bound features (go to line, the buffer search bar) reach the SQL editor.
    fn act_as_type<'a>(
        &'a self,
        type_id: TypeId,
        self_handle: &'a Entity<Self>,
        _: &'a App,
    ) -> Option<AnyEntity> {
        if type_id == TypeId::of::<Self>() {
            Some(self_handle.clone().into())
        } else if type_id == TypeId::of::<Editor>() {
            Some(self.editor.clone().into())
        } else {
            None
        }
    }

    fn as_searchable(&self, _: &Entity<Self>, _: &App) -> Option<Box<dyn SearchableItemHandle>> {
        Some(Box::new(self.editor.clone()))
    }
}

#[cfg(test)]
#[path = "console_tests.rs"]
mod tests;
