use super::*;
use crate::NewQueryConsole;
use crate::result_grid::{GridSelection, RESULT_EXPIRED};
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
use settings::{BaseKeymap, DEFAULT_KEYMAP_PATH, KeymapFile};
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
    bind_brewery_keys(cx);
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

/// The default and JetBrains keymaps, then this checkout's overrides. Dev builds read assets from
/// the checkout the test binary sits in, which is another one when the target directory is
/// shared, so the overrides are compiled in from next to this file.
fn bind_brewery_keys(cx: &mut VisualTestContext) {
    let overrides = if cfg!(target_os = "macos") {
        include_str!("../../../assets/keymaps/specific-overrides-macos.json")
    } else {
        include_str!("../../../assets/keymaps/specific-overrides.json")
    };
    cx.update(|_, cx| {
        let base = BaseKeymap::JetBrains
            .asset_path()
            .expect("JetBrains has a keymap file");
        for path in [DEFAULT_KEYMAP_PATH, base] {
            cx.bind_keys(KeymapFile::load_asset_allow_partial_failure(path, cx).unwrap());
        }
        match KeymapFile::load(overrides, cx) {
            settings::KeymapFileLoadResult::Success { key_bindings }
            | settings::KeymapFileLoadResult::SomeFailedToLoad { key_bindings, .. } => {
                cx.bind_keys(key_bindings)
            }
            settings::KeymapFileLoadResult::JsonParseFailure { error } => {
                panic!("the overrides keymap does not parse: {error}")
            }
        }
    });
}

#[gpui::test]
async fn arrow_keys_move_the_selected_cell_and_shift_extends_a_range(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    bind_brewery_keys(cx);
    let (copy, select_all, last) = if cfg!(target_os = "macos") {
        ("cmd-c", "cmd-a", "cmd-down")
    } else {
        ("ctrl-c", "ctrl-a", "ctrl-end")
    };
    console.type_sql("select * from owners", cx);
    console.run(cx);
    let grid = console.grid(cx);
    let selected = |cx: &mut VisualTestContext| grid.read_with(cx, |grid, _| grid.selected_cell());
    grid.update_in(cx, |grid, window, cx| {
        window.focus(&grid.focus_handle(cx), cx);
    });

    // With nothing selected, the first move selects the first cell.
    cx.simulate_keystrokes("down");
    assert_eq!(selected(cx), Some((0, 0)));
    cx.simulate_keystrokes("down down right");
    assert_eq!(selected(cx), Some((2, 1)));
    cx.simulate_keystrokes("right left up");
    assert_eq!(selected(cx), Some((1, 0)));
    cx.simulate_keystrokes("end");
    assert_eq!(selected(cx), Some((1, 1)));
    cx.simulate_keystrokes("home");
    assert_eq!(selected(cx), Some((1, 0)));

    // Shift extends from where the selection started; copying gives TSV rows.
    cx.simulate_keystrokes("shift-down shift-right");
    cx.simulate_keystrokes(copy);
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("1\tname 1\n2\tname 2".to_string())
    );
    // A plain move collapses the range again.
    cx.simulate_keystrokes("up");
    let selection = grid.read_with(cx, |grid, _| grid.selection());
    assert_eq!(selection, Some(GridSelection::cell(1, 1)));

    // Moving to the last fetched row scrolls it into view, which fetches the next page.
    cx.simulate_keystrokes(last);
    cx.run_until_parked();
    assert_eq!(selected(cx), Some((499, 1)));
    let visible = grid.read_with(cx, |grid, _| grid.last_visible_rows());
    assert!(visible.is_some_and(|rows| rows.contains(&499)));
    assert_eq!(console.fetched_offsets(), [0, 500]);

    cx.simulate_keystrokes(select_all);
    let selection = grid.read_with(cx, |grid, _| grid.selection());
    assert_eq!(
        selection,
        Some(GridSelection {
            anchor: (0, 0),
            head: (999, 1)
        })
    );

    // Shift-click extends from the selected cell, as a drag does.
    grid.update(cx, |grid, cx| {
        grid.select_cell(4, 0, cx);
        grid.extend_selection_to(5, 1, cx);
    });
    cx.simulate_keystrokes(copy);
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("4\tname 4\n5\tname 5".to_string())
    );
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

impl Console {
    fn editor(&self, cx: &mut VisualTestContext) -> Entity<Editor> {
        self.console
            .read_with(cx, |console, _| console.editor().clone())
    }

    /// Puts the cursor at the `|` of `marked`, focuses the editor and types `typed` there.
    fn type_at(&self, marked: &str, typed: &str, cx: &mut VisualTestContext) {
        self.type_sql(marked, cx);
        let editor = self.editor(cx);
        editor.update_in(cx, |editor, window, cx| {
            window.focus(&editor.focus_handle(cx), cx)
        });
        cx.simulate_input(typed);
        cx.run_until_parked();
    }

    /// The items the open completion menu shows, in order, as `label detail`.
    fn menu(&self, cx: &mut VisualTestContext) -> Vec<String> {
        self.editor(cx).read_with(cx, |editor, _| {
            let menu = editor.context_menu().borrow();
            let Some(editor::code_context_menus::CodeContextMenu::Completions(menu)) =
                menu.as_ref()
            else {
                return Vec::new();
            };
            let completions = menu.completions.borrow();
            menu.entries
                .borrow()
                .iter()
                .filter_map(|entry| entry.as_match())
                .map(|entry| {
                    let completion = &completions[entry.candidate_id];
                    let detail = match &completion.documentation {
                        Some(project::lsp_store::CompletionDocumentation::SingleLine(detail)) => {
                            detail.to_string()
                        }
                        _ => String::new(),
                    };
                    format!("{} {detail}", completion.label.text)
                })
                .collect()
        })
    }

    /// Types `typed` where the cursor is, one keystroke at a time.
    fn keep_typing(&self, typed: &str, cx: &mut VisualTestContext) {
        cx.simulate_input(typed);
        cx.run_until_parked();
    }

    fn accept(&self, cx: &mut VisualTestContext) {
        let editor = self.editor(cx);
        editor.update_in(cx, |editor, window, cx| {
            editor
                .confirm_completion(&editor::actions::ConfirmCompletion::default(), window, cx)
                .expect("a completion to accept")
                .detach();
        });
        cx.run_until_parked();
    }

    fn text(&self, cx: &mut VisualTestContext) -> String {
        self.editor(cx).read_with(cx, |editor, cx| editor.text(cx))
    }
}

#[gpui::test]
async fn typing_a_table_name_offers_the_tables(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    // The schema is fetched on the first request; the menu shows once it arrives.
    console.type_at("SELECT * FROM |", "ow", cx);
    assert_eq!(
        console.fixture.calls_of("db/schema"),
        vec![json!({"connectionId": "c1"})]
    );
    assert_eq!(
        console.menu(cx),
        vec!["owners table · public", "owner_names view · public"]
    );

    console.accept(cx);
    assert_eq!(console.text(cx), "SELECT * FROM owners");
    assert_eq!(console.fixture.calls_of("db/schema").len(), 1);
}

#[gpui::test]
async fn a_dot_after_an_alias_offers_its_columns(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    console.type_at("SELECT | FROM owners o", "o.", cx);
    assert_eq!(
        console.menu(cx),
        vec!["id serial", "email varchar(50)", "First Name text"]
    );

    // A name that needs quotes is inserted quoted.
    console.type_at("SELECT | FROM owners o", "o.Fi", cx);
    console.accept(cx);
    assert_eq!(console.text(cx), "SELECT o.\"First Name\" FROM owners o");
}

#[gpui::test]
async fn a_schema_change_refreshes_the_completion(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    console.type_at("SELECT * FROM |", "ow", cx);
    assert_eq!(console.fixture.calls_of("db/schema").len(), 1);

    console.type_sql("CREATE TABLE vets (id int)", cx);
    console.run(cx);
    assert_eq!(
        console.fixture.calls_of("db/schema").last(),
        Some(&json!({"connectionId": "c1", "refresh": true}))
    );

    // So does Refresh in the Database tool window.
    console.fixture.panel.update(cx, |panel, cx| {
        panel.select(Some(RowKey::data_source("c1")), cx);
        panel.refresh(cx);
    });
    cx.run_until_parked();
    assert_eq!(console.fixture.calls_of("db/schema").len(), 3);

    // A plain query does not.
    console.type_sql("SELECT 1", cx);
    console.run(cx);
    assert_eq!(console.fixture.calls_of("db/schema").len(), 3);
}

#[gpui::test]
async fn an_abandoned_schema_change_still_refreshes_the_completion(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    console.type_at("SELECT * FROM |", "ow", cx);
    assert_eq!(console.fixture.calls_of("db/schema").len(), 1);

    // Cancelled, the driver ignores it and the console gives up waiting; the DDL ends later.
    console.fixture.state.hold("db/execute");
    console.type_sql("CREATE TABLE vets (id int)", cx);
    console.run(cx);
    console.console.update_in(cx, |console, window, cx| {
        console.cancel(&CancelStatement, window, cx)
    });
    cx.executor()
        .advance_clock(CANCEL_GIVE_UP + Duration::from_millis(1));
    cx.run_until_parked();
    console.fixture.state.release("db/execute");
    cx.run_until_parked();
    assert_eq!(
        console.fixture.calls_of("db/schema").last(),
        Some(&json!({"connectionId": "c1", "refresh": true}))
    );

    // The tab is closed while the DDL runs: the next console of the data source rebuilds.
    console.type_sql("DROP TABLE vets", cx);
    console.run(cx);
    let Console { fixture, console } = console;
    close_console(&fixture, console, cx);
    fixture.state.release("db/execute");
    cx.run_until_parked();
    fixture.panel.update_in(cx, |panel, window, cx| {
        panel.select(Some(RowKey::data_source("c1")), cx);
        panel.new_query_console(window, cx)
    });
    cx.run_until_parked();
    let console = Console {
        console: fixture
            .workspace
            .read_with(cx, |workspace, cx| {
                workspace.active_item_as::<QueryConsole>(cx)
            })
            .expect("a new console"),
        fixture,
    };
    console.type_at("SELECT * FROM |", "ow", cx);
    assert_eq!(
        console.fixture.calls_of("db/schema").last(),
        Some(&json!({"connectionId": "c1", "refresh": true}))
    );
    assert_eq!(console.fixture.calls_of("db/schema").len(), 3);
}

#[gpui::test]
async fn completion_without_a_schema_still_offers_keywords(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    console
        .fixture
        .state
        .fail("db/schema", "connection refused");
    console.type_at("|", "SEL", cx);
    assert_eq!(console.menu(cx), vec!["SELECT keyword"]);
    // Inside a string literal there is nothing to complete.
    console.type_at("SELECT 'ow|'", "n", cx);
    assert!(console.menu(cx).is_empty());
}

#[gpui::test]
async fn the_menu_narrows_as_the_name_is_typed(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    let mut schema = schema();
    schema["schemas"][0]["tables"] = json!([
        { "name": "orders", "kind": "table", "columns": [] },
        { "name": "owls", "kind": "table", "columns": [] },
        { "name": "owners", "kind": "table", "columns": [] },
        { "name": "owner_names", "kind": "view", "columns": [] }
    ]);
    schema["schemas"].as_array_mut().unwrap().truncate(1);
    *console.fixture.state.schema_reply.borrow_mut() = Some(schema);

    console.type_at("SELECT * FROM |", "o", cx);
    assert_eq!(
        console.menu(cx),
        [
            "orders table",
            "owls table",
            "owners table",
            "owner_names view"
        ]
    );
    console.keep_typing("w", cx);
    assert_eq!(
        console.menu(cx),
        ["owls table", "owners table", "owner_names view"]
    );
    console.keep_typing("n", cx);
    assert_eq!(console.menu(cx), ["owners table", "owner_names view"]);
    console.keep_typing("ers", cx);
    assert_eq!(console.menu(cx), ["owners table"]);
    assert_eq!(console.fixture.calls_of("db/schema").len(), 1);
}

#[gpui::test]
async fn completing_inside_a_quoted_name_replaces_it_whole(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    // The closing quote is there already (typed, or added by autoclose).
    console.type_at("SELECT o.\"F|\" FROM owners o", "i", cx);
    assert_eq!(console.menu(cx), ["First Name text"]);
    console.accept(cx);
    assert_eq!(console.text(cx), "SELECT o.\"First Name\" FROM owners o");

    console.type_at("SELECT o.\"F|rst\" FROM owners o", "i", cx);
    console.accept(cx);
    assert_eq!(console.text(cx), "SELECT o.\"First Name\" FROM owners o");
}

#[gpui::test]
async fn completing_inside_a_word_replaces_the_rest_of_it(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    console.type_at("SELECT * FROM o|ners", "w", cx);
    assert_eq!(console.menu(cx)[0], "owners table · public");
    console.accept(cx);
    assert_eq!(console.text(cx), "SELECT * FROM owners");
}

#[gpui::test]
async fn tables_of_other_schemas_are_inserted_with_their_schema(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    console.type_at("SELECT * FROM |", "inv", cx);
    assert_eq!(console.menu(cx), ["invoices table · billing"]);
    console.accept(cx);
    assert_eq!(console.text(cx), "SELECT * FROM billing.invoices");
}

fn fake_fs(fixture: &Fixture, cx: &mut VisualTestContext) -> Arc<dyn fs::Fs> {
    fixture.workspace.read_with(cx, |workspace, cx| {
        workspace.project().read(cx).fs().clone()
    })
}

fn file_text(fs: &Arc<dyn fs::Fs>, path: &std::path::Path) -> Option<String> {
    let bytes = fs.as_fake().read_file_sync(path).ok()?;
    Some(String::from_utf8(bytes).unwrap())
}

fn active_console(fixture: &Fixture, cx: &mut VisualTestContext) -> Entity<QueryConsole> {
    fixture
        .workspace
        .read_with(cx, |workspace, cx| {
            workspace.active_item_as::<QueryConsole>(cx)
        })
        .expect("a console should be the active item")
}

#[gpui::test]
async fn the_sql_is_kept_in_a_file_and_comes_back_after_the_tab_is_closed(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    let fs = fake_fs(&console.fixture, cx);
    let file = console
        .console
        .read_with(cx, |console, _| console.file().to_path_buf());
    assert_eq!(file, crate::console_store::console_file("c1", 1));
    assert_eq!(
        file_text(&fs, &file),
        None,
        "nothing written before an edit"
    );

    console.type_sql("select * from owners;\n", cx);
    cx.executor()
        .advance_clock(crate::console_store::SAVE_DELAY + Duration::from_millis(1));
    cx.run_until_parked();
    assert_eq!(
        file_text(&fs, &file).as_deref(),
        Some("select * from owners;\n")
    );

    // Typed just before closing: still written.
    console.type_sql("select 1;\n", cx);
    let Console { fixture, console } = console;
    close_console(&fixture, console, cx);
    assert_eq!(file_text(&fs, &file).as_deref(), Some("select 1;\n"));

    fixture.panel.update_in(cx, |panel, window, cx| {
        panel.select(Some(RowKey::data_source("c1")), cx);
        panel.new_query_console(window, cx)
    });
    cx.run_until_parked();
    let reopened = active_console(&fixture, cx);
    reopened.read_with(cx, |console, cx| {
        assert_eq!(console.file(), file);
        assert_eq!(console.editor().read(cx).text(cx), "select 1;\n");
    });
}

#[gpui::test]
async fn a_second_console_of_a_data_source_gets_its_own_file(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    let fixture = &console.fixture;
    fixture
        .panel
        .update_in(cx, |panel, window, cx| panel.new_query_console(window, cx));
    cx.run_until_parked();
    let second = active_console(fixture, cx);
    assert_ne!(second.entity_id(), console.console.entity_id());
    second.read_with(cx, |second, _| {
        assert_eq!(second.file(), crate::console_store::console_file("c1", 2));
    });
}

#[gpui::test]
async fn a_table_opened_in_a_console_with_saved_sql_goes_below_it(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    let fs = fake_fs(&fixture, cx);
    fs.atomic_write(
        crate::console_store::console_file("c1", 1),
        "select 42;".into(),
    )
    .await
    .unwrap();
    let connection = fixture
        .panel
        .read_with(cx, |panel, _| panel.tree().sources()[0].clone());
    fixture.workspace.update_in(cx, |workspace, window, cx| {
        open_console(
            workspace,
            connection,
            None,
            Some("SELECT * FROM public.owners".into()),
            window,
            cx,
        )
    });
    cx.run_until_parked();
    let console = active_console(&fixture, cx);
    console.read_with(cx, |console, cx| {
        assert_eq!(
            console.editor().read(cx).text(cx),
            "select 42;\n\nSELECT * FROM public.owners"
        );
    });
    assert_eq!(
        fixture.calls_of("db/execute"),
        [json!({ "connectionId": "c1", "sql": "SELECT * FROM public.owners" })]
    );
}

#[gpui::test]
async fn a_restored_console_reopens_its_data_source_and_file(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    let fs = fake_fs(&fixture, cx);
    let file = crate::console_store::console_file("c2", 3);
    fs.atomic_write(file.clone(), "select * from reports;".into())
        .await
        .unwrap();
    let connection = fixture
        .panel
        .read_with(cx, |panel, _| panel.tree().sources()[1].clone());
    let saved = crate::console_store::SavedConsole {
        connection,
        file: file.clone(),
    };
    let project = fixture
        .workspace
        .read_with(cx, |workspace, _| workspace.project().clone());
    let console = cx.update(|window, cx| restore_console(saved, project, window, cx));
    cx.run_until_parked();
    console.read_with(cx, |console, cx| {
        assert_eq!(console.connection().id, "c2");
        assert_eq!(console.tab_content_text(0, cx), "Reports console");
        assert_eq!(console.editor().read(cx).text(cx), "select * from reports;");
    });
    // A new console of that data source does not take the restored console's file.
    let next = cx.update(|_, cx| crate::console_store::claim_free_file("c2", cx));
    assert_eq!(next, crate::console_store::console_file("c2", 1));
    let next = cx.update(|_, cx| crate::console_store::claim_free_file("c2", cx));
    assert_eq!(next, crate::console_store::console_file("c2", 2));
    let next = cx.update(|_, cx| crate::console_store::claim_free_file("c2", cx));
    assert_eq!(next, crate::console_store::console_file("c2", 4));
}

#[test]
fn console_files_are_named_per_data_source() {
    let first = crate::console_store::console_file("4f1c-9a", 1);
    assert!(first.ends_with("db_consoles/4f1c-9a/console.sql"));
    let third = crate::console_store::console_file("4f1c-9a", 3);
    assert!(third.ends_with("db_consoles/4f1c-9a/console_3.sql"));
    let odd = crate::console_store::consoles_dir("../x:y");
    assert!(odd.ends_with("db_consoles/___x_y"));
}

fn executed_sql(console: &Console) -> Vec<String> {
    console
        .executed()
        .iter()
        .map(|params| params["sql"].as_str().unwrap().to_string())
        .collect()
}

#[gpui::test]
async fn a_selection_of_several_statements_runs_them_all_with_a_tab_each(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    let text = "select * from owners;\nupdate owners set name = 'x';\nselect * from pets;";
    console.type_sql(text, cx);
    console.editor(cx).update_in(cx, |editor, window, cx| {
        editor.change_selections(SelectionEffects::no_scroll(), window, cx, |selections| {
            selections.select_ranges([MultiBufferOffset(0)..MultiBufferOffset(text.len())])
        });
    });
    console.run(cx);
    assert_eq!(
        executed_sql(&console),
        [
            "select * from owners",
            "update owners set name = 'x'",
            "select * from pets"
        ]
    );
    console.console.read_with(cx, |console, _| {
        assert_eq!(console.result_count(), 3);
        assert_eq!(console.active_result(), 2, "the last result is shown");
        assert!(!console.is_running());
    });
    assert_eq!(console.status(cx), "500 rows fetched+ in 12 ms");
    let cursor = console
        .grid(cx)
        .read_with(cx, |grid, _| grid.cursor_id().map(str::to_string));
    assert_eq!(cursor.as_deref(), Some("k2"));

    console
        .console
        .update(cx, |console, cx| console.show_result(1, cx));
    assert_eq!(console.status(cx), "3 rows affected in 4 ms");
    console
        .console
        .read_with(cx, |console, _| assert!(console.grid().is_none()));
    console
        .console
        .update(cx, |console, cx| console.show_result(0, cx));
    let cursor = console
        .grid(cx)
        .read_with(cx, |grid, _| grid.cursor_id().map(str::to_string));
    assert_eq!(cursor.as_deref(), Some("k1"));

    // The next run closes every cursor of the last one.
    console.type_sql("select 1", cx);
    console.run(cx);
    assert_eq!(
        console.fixture.calls_of("db/close"),
        [json!({ "cursorId": "k1" }), json!({ "cursorId": "k2" })]
    );
    console
        .console
        .read_with(cx, |console, _| assert_eq!(console.result_count(), 1));
}

#[gpui::test]
async fn run_all_waits_for_the_first_page_before_the_next_statement(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    console.type_sql("select * from owners;| select * from pets", cx);
    console.fixture.state.hold("db/fetch");
    console.console.update_in(cx, |console, window, cx| {
        console.run_all(&RunAllStatements, window, cx)
    });
    cx.run_until_parked();
    // Running the second statement now could end the first result before it showed anything.
    assert_eq!(executed_sql(&console), ["select * from owners"]);
    console
        .console
        .read_with(cx, |console, _| assert!(console.is_running()));
    assert_eq!(console.status(cx), "0 rows fetched+ in 12 ms");

    console.fixture.state.release("db/fetch");
    cx.run_until_parked();
    assert_eq!(
        executed_sql(&console),
        ["select * from owners", "select * from pets"]
    );
    console.fixture.state.release("db/fetch");
    cx.run_until_parked();
    console.console.read_with(cx, |console, _| {
        assert!(!console.is_running());
        assert_eq!(console.result_count(), 2);
    });
}

#[gpui::test]
async fn a_failing_statement_ends_the_run(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    console.type_sql("update owners set name = 'x'; fail; select 1", cx);
    console.console.update_in(cx, |console, window, cx| {
        console.run_all(&RunAllStatements, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        executed_sql(&console),
        ["update owners set name = 'x'", "fail"]
    );
    console.console.read_with(cx, |console, cx| {
        assert_eq!(console.result_count(), 2);
        assert_eq!(console.status_line(cx).1, Color::Error);
        assert!(!console.is_running());
    });
}

#[gpui::test]
async fn cancel_drops_the_statements_not_started_yet(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    console.fixture.state.hold("db/execute");
    console.type_sql("select pg_sleep(60); select 1; select 2", cx);
    console.console.update_in(cx, |console, window, cx| {
        console.run_all(&RunAllStatements, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(console.status(cx), "Running 1 of 3…");
    console.console.update_in(cx, |console, window, cx| {
        console.cancel(&CancelStatement, window, cx)
    });
    console.fixture.state.release("db/execute");
    cx.run_until_parked();
    assert_eq!(executed_sql(&console), ["select pg_sleep(60)"]);
    console.console.read_with(cx, |console, _| {
        assert!(!console.is_running());
        assert_eq!(console.result_count(), 1);
    });
}

#[gpui::test]
async fn results_the_daemon_dropped_are_mentioned(cx: &mut TestAppContext) {
    let (console, cx) = console(cx).await;
    console.type_sql("update owners /*more*/ set name = 'x'", cx);
    console.run(cx);
    assert_eq!(
        console.status(cx),
        format!("3 rows affected in 4 ms · {MORE_RESULTS_DROPPED}")
    );
    console.console.read_with(cx, |console, cx| {
        assert_eq!(console.status_line(cx).1, Color::Warning)
    });
}
