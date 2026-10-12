use super::*;
use crate::NewQueryConsole;
use crate::result_grid::RESULT_EXPIRED;
use crate::test_support::*;
use crate::tree::RowKey;
use brewery_daemon_client::{DB_NOT_FOUND, DB_QUERY_FAILED};
use editor::SelectionEffects;
use gpui::{
    Modifiers, ScrollDelta, ScrollStrategy, ScrollWheelEvent, TestAppContext, TouchPhase,
    VisualTestContext, point, px, size,
};
use language::{Language, LanguageConfig};
use serde_json::{Value, json};
use settings::{BaseKeymap, DEFAULT_KEYMAP_PATH, KeymapFile, SPECIFIC_OVERRIDES_KEYMAP_PATH};
use std::sync::Arc;
use std::time::Duration;
use workspace::item::ItemHandle as _;

struct Console {
    fixture: Fixture,
    console: Entity<QueryConsole>,
}

impl Console {
    fn executed(&self) -> Vec<Value> {
        self.fixture.calls_of("db/execute")
    }

    fn fetched_offsets(&self) -> Vec<u64> {
        self.fixture
            .calls_of("db/fetch")
            .iter()
            .filter_map(|params| params["offset"].as_u64())
            .collect()
    }

    fn status(&self, cx: &mut VisualTestContext) -> String {
        self.console
            .read_with(cx, |console, cx| console.status_line(cx).0.to_string())
    }

    fn grid(&self, cx: &mut VisualTestContext) -> Entity<ResultGrid> {
        self.console
            .read_with(cx, |console, _| console.grid().cloned())
            .expect("a result grid")
    }

    /// Replaces the text; `|` marks the cursor.
    fn type_sql(&self, marked: &str, cx: &mut VisualTestContext) {
        let offset = marked.find('|').unwrap_or(marked.len());
        let text = marked.replacen('|', "", 1);
        let editor = self
            .console
            .read_with(cx, |console, _| console.editor().clone());
        editor.update_in(cx, |editor, window, cx| {
            editor.set_text(text, window, cx);
            editor.change_selections(SelectionEffects::no_scroll(), window, cx, |selections| {
                selections.select_ranges([MultiBufferOffset(offset)..MultiBufferOffset(offset)])
            });
        });
    }

    fn run(&self, cx: &mut VisualTestContext) {
        self.console.update_in(cx, |console, window, cx| {
            console.run(&RunStatement, window, cx)
        });
        cx.run_until_parked();
    }

    /// Scrolls the grid so that `row` is at the top and lets it render.
    fn scroll_grid_to(&self, row: usize, cx: &mut VisualTestContext) {
        let grid = self.grid(cx);
        grid.update(cx, |grid, cx| {
            grid.scroll_handle()
                .scroll_to_item(row, ScrollStrategy::Top);
            cx.notify();
        });
        cx.run_until_parked();
    }
}

async fn console(cx: &mut TestAppContext) -> (Console, &mut VisualTestContext) {
    let (fixture, cx) = shown_panel(cx).await;
    fixture.panel.update(cx, |panel, cx| {
        panel.select(Some(RowKey::data_source("c1")), cx)
    });
    fixture
        .panel
        .update_in(cx, |panel, window, cx| panel.new_query_console(window, cx));
    cx.run_until_parked();
    let console = fixture
        .workspace
        .read_with(cx, |workspace, cx| {
            workspace.active_item_as::<QueryConsole>(cx)
        })
        .expect("a console should be the active item");
    (Console { fixture, console }, cx)
}

#[gpui::test]
async fn new_query_console_opens_a_tab_for_the_selected_data_source(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    console.console.read_with(cx, |console, cx| {
        assert_eq!(console.tab_content_text(0, cx), "Local Postgres console");
        assert_eq!(console.connection().id, "c1");
        assert_eq!(console.editor().read(cx).text(cx), "");
    });
    assert!(console.executed().is_empty());

    // The action from the command palette opens one too, for the selection in the panel.
    let fixture = &console.fixture;
    fixture.panel.update(cx, |panel, cx| {
        panel.select(Some(RowKey::data_source("c2")), cx)
    });
    cx.dispatch_action(NewQueryConsole);
    cx.run_until_parked();
    let title = fixture.workspace.read_with(cx, |workspace, cx| {
        let item = workspace
            .active_item_as::<QueryConsole>(cx)
            .expect("a second console");
        item.read(cx).tab_content_text(0, cx)
    });
    assert_eq!(title, "Reports console");
}

#[gpui::test]
async fn run_executes_the_statement_at_the_cursor_and_shows_the_rows(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    console.type_sql("select 1;\nselect * from| owners;\nselect 'a;b'", cx);
    console.run(cx);

    assert_eq!(
        console.executed(),
        [json!({ "connectionId": "c1", "sql": "select * from owners" })]
    );
    assert_eq!(
        console.fixture.calls_of("db/fetch"),
        [json!({ "cursorId": "k1", "offset": 0, "limit": 500 })]
    );
    let grid = console.grid(cx);
    grid.read_with(cx, |grid, _| {
        let pages = grid.pages();
        assert_eq!(pages.row_count(), 500);
        assert_eq!(pages.columns()[0].name, "id");
        assert_eq!(pages.cell(1, 1), Some("name 1"));
        assert_eq!(pages.cell(0, 1), None, "row 0 has a NULL name");
    });
    assert_eq!(console.status(cx), "500 rows fetched+ in 12 ms");

    // The grid rendered its first rows, and they are near the top, so no second page yet.
    let visible = grid.read_with(cx, |grid, _| grid.last_visible_rows());
    let visible = visible.expect("the grid should have rendered rows");
    assert_eq!(visible.start, 0);
    assert!(visible.end > 0);
    assert_eq!(console.fetched_offsets(), [0]);
}

#[gpui::test]
async fn a_selection_runs_the_selected_text(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    console.type_sql("select 1, 2, 3", cx);
    let editor = console
        .console
        .read_with(cx, |console, _| console.editor().clone());
    editor.update_in(cx, |editor, window, cx| {
        editor.change_selections(SelectionEffects::no_scroll(), window, cx, |selections| {
            selections.select_ranges([MultiBufferOffset(0)..MultiBufferOffset(9)])
        });
    });
    console.run(cx);
    assert_eq!(
        console.executed(),
        [json!({ "connectionId": "c1", "sql": "select 1," })]
    );
}

#[gpui::test]
async fn nothing_to_run_does_not_call_the_daemon(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    console.type_sql("  -- just a note\n|", cx);
    console.run(cx);
    assert!(console.executed().is_empty());
    assert_eq!(console.status(cx), NOTHING_TO_RUN);
}

#[gpui::test]
async fn scrolling_near_the_end_fetches_the_next_page(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    console.type_sql("select * from owners", cx);
    console.run(cx);
    assert_eq!(console.fetched_offsets(), [0]);

    console.scroll_grid_to(420, cx);
    assert_eq!(console.fetched_offsets(), [0, 500]);
    assert_eq!(console.status(cx), "1000 rows fetched+ in 12 ms");

    // Scrolling within what is loaded does not fetch again.
    console.scroll_grid_to(600, cx);
    assert_eq!(console.fetched_offsets(), [0, 500]);

    // The last page (200 rows of 1200) ends the result.
    console.scroll_grid_to(950, cx);
    assert_eq!(console.fetched_offsets(), [0, 500, 1000]);
    assert_eq!(console.status(cx), "1200 rows fetched in 12 ms");
    console.scroll_grid_to(1150, cx);
    assert_eq!(console.fetched_offsets(), [0, 500, 1000]);
}

#[gpui::test]
async fn an_expired_cursor_says_so_and_stops_paging(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    console.type_sql("select * from owners", cx);
    console.run(cx);
    console
        .fixture
        .state
        .fail_with_code("db/fetch", DB_NOT_FOUND, "Unknown cursor k1");

    console.scroll_grid_to(450, cx);
    assert_eq!(console.fetched_offsets(), [0, 500]);
    assert_eq!(
        console.status(cx),
        format!("500 rows fetched+ in 12 ms · {RESULT_EXPIRED}")
    );
    // The rows that came stay; no more requests.
    console.fixture.state.succeed("db/fetch");
    console.scroll_grid_to(480, cx);
    assert_eq!(console.fetched_offsets(), [0, 500]);
    let rows = console
        .grid(cx)
        .read_with(cx, |grid, _| grid.pages().row_count());
    assert_eq!(rows, 500);
}

#[gpui::test]
async fn an_update_shows_the_affected_row_count(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    console.type_sql("update owners set name = 'x'", cx);
    console.run(cx);
    assert_eq!(console.status(cx), "3 rows affected in 4 ms");
    assert!(console.fixture.calls_of("db/fetch").is_empty());
    console
        .console
        .read_with(cx, |console, _| assert!(console.grid().is_none()));
}

#[gpui::test]
async fn a_failed_statement_shows_the_daemon_message(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    let message = "ERROR: relation \"nope\" does not exist (SQLState 42P01)";
    console
        .fixture
        .state
        .fail_with_code("db/execute", DB_QUERY_FAILED, message);
    console.type_sql("select * from nope", cx);
    console.run(cx);
    assert_eq!(console.status(cx), message);
    console.console.read_with(cx, |console, cx| {
        assert_eq!(console.status_line(cx).1, Color::Error);
        assert!(console.grid().is_none());
        assert!(!console.is_running());
    });
}

#[gpui::test]
async fn cancel_asks_the_daemon_to_cancel_the_running_statement(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    console.fixture.state.hold("db/execute");
    console.type_sql("select pg_sleep(60)", cx);
    console.run(cx);
    console
        .console
        .read_with(cx, |console, _| assert!(console.is_running()));
    assert_eq!(console.status(cx), "Running…");

    // A second run while one is running is ignored.
    console.run(cx);
    assert_eq!(console.executed().len(), 1);

    console.console.update_in(cx, |console, window, cx| {
        console.cancel(&CancelStatement, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        console.fixture.calls_of("db/cancel"),
        [json!({ "connectionId": "c1" })]
    );
    assert_eq!(console.status(cx), "Cancelling…");

    console.fixture.state.release("db/execute");
    cx.run_until_parked();
    console
        .console
        .read_with(cx, |console, _| assert!(!console.is_running()));
}

#[gpui::test]
async fn a_second_run_closes_the_first_cursor(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    console.type_sql("select * from owners", cx);
    console.run(cx);
    assert!(console.fixture.calls_of("db/close").is_empty());

    console.run(cx);
    assert_eq!(
        console.fixture.calls_of("db/close"),
        [json!({ "cursorId": "k1" })]
    );
    assert_eq!(console.executed().len(), 2);
    let cursor = console
        .grid(cx)
        .read_with(cx, |grid, _| grid.cursor_id().map(str::to_string));
    assert_eq!(cursor.as_deref(), Some("k2"));
}

#[gpui::test]
async fn closing_the_console_closes_its_cursor(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    console.type_sql("select * from owners", cx);
    console.run(cx);
    let Console { fixture, console } = console;
    close_console(&fixture, console, cx);
    assert_eq!(fixture.calls_of("db/close"), [json!({ "cursorId": "k1" })]);
}

#[gpui::test]
async fn closing_the_console_while_running_cancels_the_statement(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    console.fixture.state.hold("db/execute");
    console.type_sql("select * from owners", cx);
    console.run(cx);
    let Console { fixture, console } = console;
    close_console(&fixture, console, cx);
    assert_eq!(
        fixture.calls_of("db/cancel"),
        [json!({ "connectionId": "c1" })]
    );

    // The statement finishes after all: the cursor it opened is closed, nobody shows it.
    fixture.state.release("db/execute");
    cx.run_until_parked();
    assert_eq!(fixture.calls_of("db/close"), [json!({ "cursorId": "k1" })]);
    assert!(fixture.calls_of("db/fetch").is_empty());
}

/// Closes the console's tab and drops the test's handle, so the entity is released.
fn close_console(fixture: &Fixture, console: Entity<QueryConsole>, cx: &mut VisualTestContext) {
    let item_id = console.item_id();
    drop(console);
    let pane = fixture
        .workspace
        .read_with(cx, |workspace, _| workspace.active_pane().clone());
    pane.update_in(cx, |pane, window, cx| {
        pane.close_item_by_id(item_id, workspace::SaveIntent::Skip, window, cx)
    })
    .detach();
    cx.run_until_parked();
}

#[gpui::test]
async fn double_clicking_a_table_runs_select_star_in_a_new_console(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    fixture
        .panel
        .update(cx, |panel, cx| panel.toggle(RowKey::data_source("c1"), cx));
    cx.run_until_parked();
    fixture.panel.update(cx, |panel, cx| {
        panel.toggle(RowKey::new("c1", vec!["public".to_string()]), cx)
    });
    cx.run_until_parked();

    let table = RowKey::new("c1", vec!["public".to_string(), "owners".to_string()]);
    fixture
        .panel
        .update_in(cx, |panel, window, cx| panel.open_row(table, window, cx));
    cx.run_until_parked();

    let console = fixture
        .workspace
        .read_with(cx, |workspace, cx| {
            workspace.active_item_as::<QueryConsole>(cx)
        })
        .expect("a console for the table");
    console.read_with(cx, |console, cx| {
        assert_eq!(
            console.editor().read(cx).text(cx),
            "SELECT * FROM public.owners"
        );
        assert_eq!(console.tab_content_text(0, cx), "Local Postgres console");
    });
    assert_eq!(
        fixture.calls_of("db/execute"),
        [json!({ "connectionId": "c1", "sql": "SELECT * FROM public.owners" })]
    );
    // The table was not expanded by the double click.
    let expanded = fixture.panel.read_with(cx, |panel, _| {
        panel.tree().rows().iter().any(|row| {
            matches!(row, crate::tree::Row::Node { node, expanded: true, .. } if node.name == "owners")
        })
    });
    assert!(!expanded);
}

#[gpui::test]
async fn keystrokes_run_cancel_and_copy(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    cx.update(|_, cx| {
        let base = BaseKeymap::JetBrains
            .asset_path()
            .expect("JetBrains has a keymap file");
        // Other crates' actions are not linked into this binary; only ours matter here.
        for path in [DEFAULT_KEYMAP_PATH, base, SPECIFIC_OVERRIDES_KEYMAP_PATH] {
            cx.bind_keys(KeymapFile::load_asset_allow_partial_failure(path, cx).unwrap());
        }
    });
    let (run, cancel, copy) = if cfg!(target_os = "macos") {
        ("cmd-enter", "cmd-f2", "cmd-c")
    } else {
        ("ctrl-enter", "ctrl-f2", "ctrl-c")
    };

    console.type_sql("select * from owners", cx);
    let editor = console
        .console
        .read_with(cx, |console, _| console.editor().clone());
    editor.update_in(cx, |editor, window, cx| {
        window.focus(&editor.focus_handle(cx), cx)
    });
    console.fixture.state.hold("db/execute");
    cx.simulate_keystrokes(run);
    cx.run_until_parked();
    assert_eq!(console.executed().len(), 1, "{run} runs the statement");
    assert_eq!(
        console
            .console
            .read_with(cx, |console, cx| console.editor().read(cx).text(cx)),
        "select * from owners",
        "{run} does not insert a newline"
    );

    cx.simulate_keystrokes(cancel);
    cx.run_until_parked();
    assert_eq!(
        console.fixture.calls_of("db/cancel").len(),
        1,
        "{cancel} cancels"
    );
    console.fixture.state.release("db/execute");
    cx.run_until_parked();

    let grid = console.grid(cx);
    grid.update_in(cx, |grid, window, cx| {
        grid.select_cell(3, 1, cx);
        window.focus(&grid.focus_handle(cx), cx);
    });
    cx.simulate_keystrokes(copy);
    cx.run_until_parked();
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("name 3".to_string())
    );

    // Run also works with the focus in the result grid.
    cx.simulate_keystrokes(run);
    cx.run_until_parked();
    assert_eq!(console.executed().len(), 2, "{run} from the grid runs too");
}

#[gpui::test]
async fn sql_highlighting_arrives_when_the_extension_registers_the_language(
    cx: &mut TestAppContext,
) {
    let (console, cx) = console(cx).await;
    let language_name = |console: &Console, cx: &mut VisualTestContext| {
        console.console.read_with(cx, |console, cx| {
            let buffer = console.editor().read(cx).buffer().read(cx).as_singleton()?;
            Some(buffer.read(cx).language()?.name().to_string())
        })
    };
    assert_eq!(language_name(&console, cx), None, "plain text without SQL");

    // The `sql` extension installs later (first launch) and registers its language.
    let languages = console.fixture.workspace.read_with(cx, |workspace, cx| {
        workspace.project().read(cx).languages().clone()
    });
    languages.add(Arc::new(Language::new(
        LanguageConfig {
            name: "SQL".into(),
            ..LanguageConfig::default()
        },
        None,
    )));
    cx.run_until_parked();
    assert_eq!(language_name(&console, cx).as_deref(), Some("SQL"));
}

#[gpui::test]
async fn the_console_exposes_its_editor_to_the_search_bar(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    let item: Box<dyn workspace::item::ItemHandle> = Box::new(console.console.clone());
    let editor = console
        .console
        .read_with(cx, |console, _| console.editor().clone());
    cx.update(|_, cx| {
        assert_eq!(
            item.act_as::<Editor>(cx).map(|found| found.entity_id()),
            Some(editor.entity_id())
        );
        assert!(item.to_searchable_item_handle(cx).is_some());
    });
}

#[gpui::test]
async fn a_cancel_the_driver_ignores_frees_the_console_after_a_while(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    console.fixture.state.hold("db/execute");
    console.type_sql("select * from owners", cx);
    console.run(cx);
    console.console.update_in(cx, |console, window, cx| {
        console.cancel(&CancelStatement, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(console.status(cx), "Cancelling…");

    cx.executor()
        .advance_clock(CANCEL_GIVE_UP + Duration::from_millis(1));
    cx.run_until_parked();
    console
        .console
        .read_with(cx, |console, _| assert!(!console.is_running()));
    assert_eq!(console.status(cx), CANCEL_IGNORED);

    // A new run works; when the abandoned statement finally ends, its result is not shown and
    // its cursor is closed.
    console.run(cx);
    assert_eq!(console.executed().len(), 2);
    console.fixture.state.release("db/execute");
    cx.run_until_parked();
    assert_eq!(
        console.fixture.calls_of("db/close"),
        [json!({ "cursorId": "k1" })]
    );
    console.fixture.state.release("db/execute");
    cx.run_until_parked();
    let cursor = console
        .grid(cx)
        .read_with(cx, |grid, _| grid.cursor_id().map(str::to_string));
    assert_eq!(cursor.as_deref(), Some("k2"));
}

fn wheel(position: gpui::Point<gpui::Pixels>, x: f32, y: f32) -> ScrollWheelEvent {
    ScrollWheelEvent {
        position,
        delta: ScrollDelta::Pixels(point(px(x), px(y))),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
    }
}

#[gpui::test]
async fn the_wheel_scrolls_the_grid_along_one_axis_only(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    // Narrow enough that the two columns do not fit next to the Database panel.
    cx.simulate_resize(size(px(380.), px(900.)));
    console.type_sql("select * from owners", cx);
    console.run(cx);
    let grid = console.grid(cx);
    let offsets = |cx: &mut VisualTestContext| {
        grid.read_with(cx, |grid, _| {
            (grid.horizontal_offset().x, grid.vertical_offset().y)
        })
    };
    let center = grid.read_with(cx, |grid, _| grid.bounds().center());
    let content_width = grid.read_with(cx, |grid, _| grid.content_width());
    let view_width = grid.read_with(cx, |grid, _| grid.bounds().size.width);
    assert!(
        content_width > view_width,
        "the grid should be wider than its view"
    );
    assert_eq!(offsets(cx), (px(0.), px(0.)));

    // A vertical wheel moves the rows, not the columns.
    cx.simulate_event(wheel(center, 0., -100.));
    cx.run_until_parked();
    let (x, y) = offsets(cx);
    assert_eq!(x, px(0.), "a vertical wheel must not scroll sideways");
    assert!(y < px(0.), "the rows should have scrolled");

    // A horizontal swipe moves the columns, not the rows.
    cx.simulate_event(wheel(center, -40., 0.));
    cx.run_until_parked();
    let (x2, y2) = offsets(cx);
    assert!(x2 < px(0.), "the columns should have scrolled");
    assert_eq!(y2, y, "a horizontal swipe must not scroll the rows");
}

#[gpui::test]
async fn renaming_the_data_source_renames_its_consoles(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    let mut renamed = console
        .console
        .read_with(cx, |console, _| console.connection().clone());
    renamed.name = "Main Postgres".to_string();
    console
        .fixture
        .panel
        .update(cx, |panel, cx| panel.data_source_saved(renamed, cx));
    cx.run_until_parked();
    console.console.read_with(cx, |console, cx| {
        assert_eq!(console.tab_content_text(0, cx), "Main Postgres console");
    });
}
