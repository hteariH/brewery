//! The query console: a workspace tab with an SQL editor bound to one data source and, below it,
//! the result of the last statement (a paged grid, an update count or the daemon's error).

use crate::panel::WAITING_FOR_DAEMON;
use crate::result_grid::ResultGrid;
use crate::statements::statements_to_run;
use crate::{CancelStatement, RunStatement};
use brewery_daemon_client::{
    CancelParams, ConnectionInfo, ExecuteParams, ExecuteResult, ProjectDaemon, daemon_error_message,
};
use editor::{Editor, MultiBufferOffset};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, Render, SharedString, Subscription,
    Window, px, relative,
};
use language::Buffer;
use project::Project;
use ui::{Tooltip, prelude::*};
use workspace::Workspace;
use workspace::item::Item;

pub const NOTHING_TO_RUN: &str = "Nothing to run";
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
}

impl QueryConsole {
    pub fn new(
        connection: ConnectionInfo,
        daemon: Entity<ProjectDaemon>,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let languages = project.read(cx).languages().clone();
        let buffer = cx.new(|cx| {
            let buffer = Buffer::local("", cx);
            buffer.set_language_registry(languages.clone());
            buffer
        });
        // Highlighting comes from the SQL extension once it is installed; plain text until then.
        let sql = languages.language_for_name(SQL_LANGUAGE);
        buffer.update(cx, |_, cx| {
            cx.spawn(async move |buffer, cx| {
                if let Ok(sql) = sql.await {
                    buffer
                        .update(cx, |buffer, cx| buffer.set_language(Some(sql), cx))
                        .ok();
                }
            })
            .detach();
        });
        let editor = cx.new(|cx| {
            let mut editor = Editor::for_buffer(buffer, Some(project), window, cx);
            editor.set_placeholder_text(
                &format!("SQL; {RUN_KEY} runs the statement at the cursor"),
                window,
                cx,
            );
            editor
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
        }
    }

    pub fn connection(&self) -> &ConnectionInfo {
        &self.connection
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
        self.outcome = Outcome::Empty;
        self.running = Some(false);
        let execute = self.daemon.read(cx).db_execute(
            ExecuteParams {
                connection_id: self.connection.id.clone(),
                sql,
            },
            cx,
        );
        let daemon = self.daemon.clone();
        // Detached rather than owned, so that a cursor opened after the tab was closed is still
        // closed.
        cx.spawn(async move |this, cx| {
            let result = execute.await;
            let cursor = result
                .as_ref()
                .ok()
                .and_then(|result| result.cursor_id.clone());
            let shown = this
                .update(cx, |this, cx| this.finish_run(result, note, cx))
                .is_ok();
            if !shown && let Some(cursor) = cursor {
                let close = cx.update(|cx| daemon.read(cx).db_close(cursor, cx));
                close.await.ok();
            }
        })
        .detach();
        cx.notify();
    }

    fn finish_run(
        &mut self,
        result: anyhow::Result<ExecuteResult>,
        note: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.running = None;
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
    }

    pub fn cancel(&mut self, _: &CancelStatement, _: &mut Window, cx: &mut Context<Self>) {
        if self.running != Some(false) {
            return;
        }
        self.running = Some(true);
        self.send_cancel(cx);
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
        self.close_cursor(cx);
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
    let console = cx.new(|cx| QueryConsole::new(connection, daemon, project, window, cx));
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

impl EventEmitter<()> for QueryConsole {}

impl Item for QueryConsole {
    type Event = ();

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
}

#[cfg(test)]
#[path = "console_tests.rs"]
mod tests;
