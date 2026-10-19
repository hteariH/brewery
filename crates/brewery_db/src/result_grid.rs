//! The result grid of a query console: the rows of one daemon cursor, pulled in pages of 500 as
//! the user scrolls, in a virtualised list with a header row, scrollbars, keyboard and mouse
//! selection of a cell or a range, and copying of the selection.

use crate::{
    CopyResultCell, ExtendCellsDown, ExtendCellsLeft, ExtendCellsRight, ExtendCellsUp,
    MoveCellDown, MoveCellLeft, MoveCellRight, MoveCellToFirst, MoveCellToLast, MoveCellToRowEnd,
    MoveCellToRowStart, MoveCellUp, SelectAllCells,
};
use brewery_daemon_client::{
    ColumnInfo, DB_NOT_FOUND, FetchParams, ProjectDaemon, daemon_error_code, daemon_error_message,
};
use gpui::{
    ClipboardItem, Context, Entity, FocusHandle, Focusable, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Render, ScrollHandle, ScrollStrategy, Stateful, Task,
    UniformListScrollHandle, Window, point, px, uniform_list,
};
use std::borrow::Cow;
use std::ops::Range;
use ui::{ScrollAxes, Scrollbars, WithScrollbar, prelude::*};

/// Rows per `db/fetch`; the first page is fetched as soon as the statement has run.
pub const PAGE_SIZE: u32 = 500;
/// The next page is fetched once the visible rows come this close to the end of what is loaded.
pub const PREFETCH_ROWS: usize = 100;
pub const MIN_COLUMN_WIDTH: f32 = 60.;
pub const MAX_COLUMN_WIDTH: f32 = 400.;
pub const CELL_PADDING: f32 = 16.;
pub const ROW_HEIGHT: f32 = 22.;
pub const NULL_TEXT: &str = "<null>";
/// Shown instead of the daemon error when a cursor timed out or was replaced by another
/// statement on the same connection (`DB_NOT_FOUND`).
pub const RESULT_EXPIRED: &str = "Result expired — run again";
/// Shown when the daemon cut a result off: it caps results it cannot stream (inside a
/// PostgreSQL transaction, `... RETURNING`, `FOR UPDATE`) at 100 000 rows.
pub const RESULT_TRUNCATED: &str = "first rows only, the rest was cut off";
/// Shown when the daemon dropped further results of the same SQL text (several statements the
/// splitter took for one).
pub const MORE_RESULTS_DROPPED: &str = "further results of this SQL were not shown";

#[derive(Debug, Clone, PartialEq)]
pub enum FetchFailure {
    Expired,
    Error(String),
}

#[derive(Debug, Clone, PartialEq)]
enum PageState {
    Idle,
    Loading,
    Done,
    Stopped(FetchFailure),
}

/// The rows of a cursor fetched so far and whether more may come. Pure, so paging rules are
/// tested without a window.
pub struct GridPages {
    columns: Vec<ColumnInfo>,
    rows: Vec<Vec<Option<String>>>,
    state: PageState,
    /// Characters per column: the header and the longest value fetched so far.
    column_chars: Vec<usize>,
    /// The daemon cut the result off after these rows.
    truncated: bool,
    /// The daemon dropped results that followed this one.
    more_results: bool,
}

impl GridPages {
    pub fn new(columns: Vec<ColumnInfo>) -> Self {
        let column_chars = columns.iter().map(header_chars).collect();
        Self {
            columns,
            rows: Vec::new(),
            state: PageState::Idle,
            column_chars,
            truncated: false,
            more_results: false,
        }
    }

    pub fn columns(&self) -> &[ColumnInfo] {
        &self.columns
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn is_done(&self) -> bool {
        self.state == PageState::Done
    }

    pub fn is_loading(&self) -> bool {
        self.state == PageState::Loading
    }

    /// Whether the rows up to `visible_end` (exclusive) come close enough to the end of what was
    /// fetched to ask for the next page.
    pub fn needs_more(&self, visible_end: usize) -> bool {
        self.state == PageState::Idle && visible_end + PREFETCH_ROWS >= self.rows.len()
    }

    /// The offset of the next page, or `None` when a fetch is running or nothing more will come.
    pub fn start_fetch(&mut self) -> Option<u32> {
        if self.state != PageState::Idle {
            return None;
        }
        self.state = PageState::Loading;
        Some(u32::try_from(self.rows.len()).unwrap_or(u32::MAX))
    }

    pub fn page_loaded(&mut self, rows: Vec<Vec<Option<String>>>, done: bool) {
        // Every page is measured, so a long value further down widens its column when it comes.
        for row in &rows {
            for (chars, value) in self.column_chars.iter_mut().zip(row) {
                let len = value.as_deref().unwrap_or(NULL_TEXT).chars().count();
                *chars = (*chars).max(len);
            }
        }
        // An empty page also ends the result: asking again would only get another empty page.
        let empty = rows.is_empty();
        self.rows.extend(rows);
        self.state = if done || empty {
            PageState::Done
        } else {
            PageState::Idle
        };
    }

    pub fn page_failed(&mut self, failure: FetchFailure) {
        self.state = PageState::Stopped(failure);
    }

    /// The last page said the daemon cut the result off (`truncated`).
    pub fn mark_truncated(&mut self) {
        self.truncated = true;
    }

    /// The last page said further results of the SQL were dropped (`moreResults`).
    pub fn mark_more_results(&mut self) {
        self.more_results = true;
    }

    /// Why paging stopped early, if it did.
    pub fn problem(&self) -> Option<&str> {
        match &self.state {
            PageState::Stopped(FetchFailure::Expired) => Some(RESULT_EXPIRED),
            PageState::Stopped(FetchFailure::Error(message)) => Some(message),
            PageState::Done if self.truncated => Some(RESULT_TRUNCATED),
            PageState::Done if self.more_results => Some(MORE_RESULTS_DROPPED),
            _ => None,
        }
    }

    /// The value at `row`, `column`; `None` for SQL NULL and outside the fetched rows.
    pub fn cell(&self, row: usize, column: usize) -> Option<&str> {
        self.rows.get(row)?.get(column)?.as_deref()
    }

    fn has_cell(&self, row: usize, column: usize) -> bool {
        row < self.rows.len() && column < self.columns.len()
    }

    /// What copying `selection` puts on the clipboard: one cell as its plain value, a range as
    /// tab-separated rows. SQL NULL copies as an empty value.
    pub fn copy_text(&self, selection: &GridSelection) -> String {
        let (rows, columns) = selection.ranges();
        if rows.len() == 1 && columns.len() == 1 {
            return self
                .cell(rows.start, columns.start)
                .unwrap_or_default()
                .to_string();
        }
        rows.map(|row| {
            columns
                .clone()
                .map(|column| tsv_field(self.cell(row, column).unwrap_or_default()))
                .collect::<Vec<_>>()
                .join("\t")
        })
        .collect::<Vec<_>>()
        .join("\n")
    }

    /// "N rows fetched[+] in X ms"; `+` while more rows may follow.
    pub fn status(&self, elapsed_ms: u64) -> String {
        let count = self.rows.len();
        let noun = if count == 1 { "row" } else { "rows" };
        let more = if self.is_done() { "" } else { "+" };
        format!("{count} {noun} fetched{more} in {elapsed_ms} ms")
    }

    pub fn column_chars(&self) -> &[usize] {
        &self.column_chars
    }
}

/// A value as one TSV field: quoted, with its quotes doubled, when it holds a tab, a line break
/// or a quote, so that a spreadsheet reads it back as one cell.
pub fn tsv_field(value: &str) -> Cow<'_, str> {
    if value.contains(['\t', '\n', '\r', '"']) {
        Cow::Owned(format!("\"{}\"", value.replace('"', "\"\"")))
    } else {
        Cow::Borrowed(value)
    }
}

/// The selected cells: the rectangle between the cell where the selection started (`anchor`)
/// and the cell it was extended to (`head`, which keyboard moves start from).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridSelection {
    pub anchor: (usize, usize),
    pub head: (usize, usize),
}

impl GridSelection {
    pub fn cell(row: usize, column: usize) -> Self {
        Self {
            anchor: (row, column),
            head: (row, column),
        }
    }

    /// The rows and columns covered, as half-open ranges.
    pub fn ranges(&self) -> (Range<usize>, Range<usize>) {
        let (anchor_row, anchor_column) = self.anchor;
        let (head_row, head_column) = self.head;
        (
            anchor_row.min(head_row)..anchor_row.max(head_row) + 1,
            anchor_column.min(head_column)..anchor_column.max(head_column) + 1,
        )
    }

    pub fn contains(&self, row: usize, column: usize) -> bool {
        let (rows, columns) = self.ranges();
        rows.contains(&row) && columns.contains(&column)
    }

    /// Moves the head to `head`; without `extend` the selection collapses onto it.
    pub fn moved_to(self, head: (usize, usize), extend: bool) -> Self {
        if extend {
            Self { head, ..self }
        } else {
            Self::cell(head.0, head.1)
        }
    }
}

/// Where a keyboard move takes the head of the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellMove {
    Up,
    Down,
    Left,
    Right,
    RowStart,
    RowEnd,
    First,
    Last,
}

/// The cell `movement` reaches from `head` in a grid of `rows` by `columns`, clamped to it.
pub fn moved_cell(
    head: (usize, usize),
    movement: CellMove,
    rows: usize,
    columns: usize,
) -> (usize, usize) {
    let (row, column) = head;
    let last_row = rows.saturating_sub(1);
    let last_column = columns.saturating_sub(1);
    match movement {
        CellMove::Up => (row.saturating_sub(1), column),
        CellMove::Down => ((row + 1).min(last_row), column),
        CellMove::Left => (row, column.saturating_sub(1)),
        CellMove::Right => (row, (column + 1).min(last_column)),
        CellMove::RowStart => (row, 0),
        CellMove::RowEnd => (row, last_column),
        CellMove::First => (0, 0),
        CellMove::Last => (last_row, last_column),
    }
}

/// The horizontal scroll offset (negative, as gpui keeps it) that brings the column spanning
/// `start..start + width` into a view `view_width` wide, moving as little as possible.
pub fn offset_to_reveal(offset_x: f32, view_width: f32, start: f32, width: f32) -> f32 {
    let left = -offset_x;
    if start < left || width > view_width {
        -start
    } else if start + width > left + view_width {
        -(start + width - view_width)
    } else {
        offset_x
    }
}

/// The header shows the name, a space and the type.
fn header_chars(column: &ColumnInfo) -> usize {
    column.name.chars().count() + 1 + column.type_name.chars().count()
}

/// Pixel width of a column of `chars` monospace characters of `char_width`, clamped.
pub fn column_width(chars: usize, char_width: f32) -> f32 {
    (chars as f32 * char_width + CELL_PADDING).clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH)
}

/// Line breaks and tabs as spaces, so every value fits its one-line cell.
pub fn single_line(text: &str) -> Cow<'_, str> {
    if text.contains(['\n', '\r', '\t']) {
        Cow::Owned(text.replace(['\n', '\r', '\t'], " "))
    } else {
        Cow::Borrowed(text)
    }
}

/// The grid view of one cursor. It fetches its own pages; the console closes the cursor.
pub struct ResultGrid {
    daemon: Entity<ProjectDaemon>,
    cursor_id: Option<String>,
    pages: GridPages,
    focus_handle: FocusHandle,
    scroll_handle: UniformListScrollHandle,
    horizontal_scroll: ScrollHandle,
    selection: Option<GridSelection>,
    /// The left button went down on a cell and has not come up yet: moving extends the range.
    dragging: bool,
    fetch_task: Option<Task<()>>,
    /// The rows the list rendered last.
    last_visible: Option<Range<usize>>,
    /// The column widths of the last render, to scroll a selected column into view.
    last_widths: Vec<f32>,
}

impl ResultGrid {
    /// Starts fetching the first page right away.
    pub fn new(
        daemon: Entity<ProjectDaemon>,
        cursor_id: String,
        columns: Vec<ColumnInfo>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut grid = Self {
            daemon,
            cursor_id: Some(cursor_id),
            pages: GridPages::new(columns),
            focus_handle: cx.focus_handle(),
            scroll_handle: UniformListScrollHandle::new(),
            horizontal_scroll: ScrollHandle::new(),
            selection: None,
            dragging: false,
            fetch_task: None,
            last_visible: None,
            last_widths: Vec::new(),
        };
        grid.fetch_more(cx);
        grid
    }

    pub fn pages(&self) -> &GridPages {
        &self.pages
    }

    pub fn cursor_id(&self) -> Option<&str> {
        self.cursor_id.as_deref()
    }

    pub fn horizontal_offset(&self) -> gpui::Point<gpui::Pixels> {
        self.horizontal_scroll.offset()
    }

    pub fn vertical_offset(&self) -> gpui::Point<gpui::Pixels> {
        self.scroll_handle.0.borrow().base_handle.offset()
    }

    /// Where the grid was painted last.
    pub fn bounds(&self) -> gpui::Bounds<gpui::Pixels> {
        self.horizontal_scroll.bounds()
    }

    /// The width of all columns, as of the last paint.
    pub fn content_width(&self) -> gpui::Pixels {
        self.horizontal_scroll.bounds().size.width + self.horizontal_scroll.max_offset().x
    }

    pub fn scroll_handle(&self) -> &UniformListScrollHandle {
        &self.scroll_handle
    }

    /// Hands the cursor over for closing; no more pages are fetched afterwards.
    pub fn take_cursor(&mut self) -> Option<String> {
        // Dropping the task drops the page in flight; the state is left as it was.
        self.fetch_task = None;
        self.cursor_id.take()
    }

    /// Called with the rows the list is about to show; fetches the next page when they come
    /// close to the end.
    pub fn rows_visible(&mut self, visible: Range<usize>, cx: &mut Context<Self>) {
        let end = visible.end;
        // `uniform_list` measures its item size by rendering row 0 alone first; that is not
        // what is on screen.
        if visible == (0..1) && self.pages.row_count() > 1 {
            return;
        }
        self.last_visible = Some(visible);
        if self.pages.needs_more(end) {
            self.fetch_more(cx);
        }
    }

    fn fetch_more(&mut self, cx: &mut Context<Self>) {
        let Some(cursor_id) = self.cursor_id.clone() else {
            return;
        };
        let Some(offset) = self.pages.start_fetch() else {
            return;
        };
        let fetch = self.daemon.read(cx).db_fetch(
            FetchParams {
                cursor_id,
                offset,
                limit: PAGE_SIZE,
            },
            cx,
        );
        self.fetch_task = Some(cx.spawn(async move |this, cx| {
            let page = fetch.await;
            this.update(cx, |this, cx| {
                match page {
                    Ok(page) => {
                        this.pages.page_loaded(page.rows, page.done);
                        if page.truncated {
                            this.pages.mark_truncated();
                        }
                        if page.more_results {
                            this.pages.mark_more_results();
                        }
                    }
                    Err(error) => {
                        let failure = if daemon_error_code(&error) == Some(DB_NOT_FOUND.into()) {
                            FetchFailure::Expired
                        } else {
                            FetchFailure::Error(daemon_error_message(&error))
                        };
                        this.pages.page_failed(failure);
                    }
                }
                this.fetch_task = None;
                cx.notify();
            })
            .ok();
        }));
    }

    pub fn last_visible_rows(&self) -> Option<Range<usize>> {
        self.last_visible.clone()
    }

    /// The cell keyboard moves start from.
    pub fn selected_cell(&self) -> Option<(usize, usize)> {
        self.selection.map(|selection| selection.head)
    }

    pub fn selection(&self) -> Option<GridSelection> {
        self.selection
    }

    pub fn select_cell(&mut self, row: usize, column: usize, cx: &mut Context<Self>) {
        if self.pages.has_cell(row, column) {
            self.selection = Some(GridSelection::cell(row, column));
            cx.notify();
        }
    }

    /// Extends the selection from where it started to `row`, `column` (shift-click, drag).
    pub fn extend_selection_to(&mut self, row: usize, column: usize, cx: &mut Context<Self>) {
        if !self.pages.has_cell(row, column) {
            return;
        }
        let selection = match self.selection {
            Some(selection) => selection.moved_to((row, column), true),
            None => GridSelection::cell(row, column),
        };
        if self.selection != Some(selection) {
            self.selection = Some(selection);
            cx.notify();
        }
    }

    /// Moves the selected cell (or, with `extend`, the far corner of the range) and scrolls it
    /// into view. With nothing selected, the first cell is selected.
    pub fn move_selection(&mut self, movement: CellMove, extend: bool, cx: &mut Context<Self>) {
        let rows = self.pages.row_count();
        let columns = self.pages.columns().len();
        if rows == 0 || columns == 0 {
            return;
        }
        let selection = match self.selection {
            Some(selection) => {
                let head = moved_cell(selection.head, movement, rows, columns);
                selection.moved_to(head, extend)
            }
            None => GridSelection::cell(0, 0),
        };
        self.selection = Some(selection);
        self.reveal(selection.head);
        cx.notify();
    }

    fn reveal(&mut self, (row, column): (usize, usize)) {
        self.scroll_handle
            .scroll_to_item(row, ScrollStrategy::Nearest);
        let Some(width) = self.last_widths.get(column).copied() else {
            return;
        };
        let start: f32 = self.last_widths[..column].iter().sum();
        let offset = self.horizontal_scroll.offset();
        let view_width = f32::from(self.horizontal_scroll.bounds().size.width);
        let x = offset_to_reveal(f32::from(offset.x), view_width, start, width);
        self.horizontal_scroll.set_offset(point(px(x), offset.y));
    }

    pub fn select_all(&mut self, _: &SelectAllCells, _: &mut Window, cx: &mut Context<Self>) {
        let rows = self.pages.row_count();
        let columns = self.pages.columns().len();
        if rows == 0 || columns == 0 {
            return;
        }
        self.selection = Some(GridSelection {
            anchor: (0, 0),
            head: (rows - 1, columns - 1),
        });
        cx.notify();
    }

    /// Copies the selection: one cell as its value, a range as TSV; NULL copies as empty.
    pub fn copy(&mut self, _: &CopyResultCell, _: &mut Window, cx: &mut Context<Self>) {
        let Some(selection) = self.selection else {
            return;
        };
        let text = self.pages.copy_text(&selection);
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn on_cell_down(
        &mut self,
        row: usize,
        column: usize,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        if event.modifiers.shift {
            self.extend_selection_to(row, column, cx);
        } else {
            self.select_cell(row, column, cx);
        }
        self.dragging = true;
    }

    fn on_cell_move(
        &mut self,
        row: usize,
        column: usize,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        if !self.dragging {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.dragging = false;
            return;
        }
        self.extend_selection_to(row, column, cx);
    }

    fn register_actions(&self, grid: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        let moves = |movement: CellMove, extend: bool| {
            move |this: &mut Self, cx: &mut Context<Self>| this.move_selection(movement, extend, cx)
        };
        let up = moves(CellMove::Up, false);
        let down = moves(CellMove::Down, false);
        let left = moves(CellMove::Left, false);
        let right = moves(CellMove::Right, false);
        let extend_up = moves(CellMove::Up, true);
        let extend_down = moves(CellMove::Down, true);
        let extend_left = moves(CellMove::Left, true);
        let extend_right = moves(CellMove::Right, true);
        let row_start = moves(CellMove::RowStart, false);
        let row_end = moves(CellMove::RowEnd, false);
        let first = moves(CellMove::First, false);
        let last = moves(CellMove::Last, false);
        grid.on_action(cx.listener(move |this, _: &MoveCellUp, _, cx| up(this, cx)))
            .on_action(cx.listener(move |this, _: &MoveCellDown, _, cx| down(this, cx)))
            .on_action(cx.listener(move |this, _: &MoveCellLeft, _, cx| left(this, cx)))
            .on_action(cx.listener(move |this, _: &MoveCellRight, _, cx| right(this, cx)))
            .on_action(cx.listener(move |this, _: &ExtendCellsUp, _, cx| extend_up(this, cx)))
            .on_action(cx.listener(move |this, _: &ExtendCellsDown, _, cx| extend_down(this, cx)))
            .on_action(cx.listener(move |this, _: &ExtendCellsLeft, _, cx| extend_left(this, cx)))
            .on_action(cx.listener(move |this, _: &ExtendCellsRight, _, cx| extend_right(this, cx)))
            .on_action(cx.listener(move |this, _: &MoveCellToRowStart, _, cx| row_start(this, cx)))
            .on_action(cx.listener(move |this, _: &MoveCellToRowEnd, _, cx| row_end(this, cx)))
            .on_action(cx.listener(move |this, _: &MoveCellToFirst, _, cx| first(this, cx)))
            .on_action(cx.listener(move |this, _: &MoveCellToLast, _, cx| last(this, cx)))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::copy))
    }

    fn widths(&self, window: &Window, cx: &App) -> Vec<f32> {
        // The cells use the buffer font at the small text size; monospace, so a char count does.
        let font_size = rems(0.875).to_pixels(window.rem_size());
        let font = theme::theme_settings(cx).buffer_font(cx).clone();
        let text_system = window.text_system();
        let char_width = text_system
            .em_advance(text_system.resolve_font(&font), font_size)
            .map(f32::from)
            .unwrap_or(f32::from(font_size) * 0.6);
        self.pages
            .column_chars()
            .iter()
            .map(|chars| column_width(*chars, char_width))
            .collect()
    }

    fn render_header(&self, widths: &[f32], cx: &Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        h_flex()
            .h(px(ROW_HEIGHT + 2.))
            .flex_none()
            .bg(colors.editor_subheader_background)
            .border_b_1()
            .border_color(colors.border)
            .children(
                self.pages
                    .columns()
                    .iter()
                    .zip(widths)
                    .map(|(column, width)| {
                        h_flex()
                            .w(px(*width))
                            .flex_none()
                            .h_full()
                            .px_2()
                            .gap_1()
                            .overflow_hidden()
                            .border_r_1()
                            .border_color(colors.border_variant)
                            .child(
                                Label::new(column.name.clone())
                                    .size(LabelSize::Small)
                                    .single_line(),
                            )
                            .child(
                                Label::new(column.type_name.clone())
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted)
                                    .single_line(),
                            )
                    }),
            )
    }

    fn render_row(&self, row: usize, widths: &[f32], cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors();
        let column_count = self.pages.columns().len();
        h_flex()
            .id(("result-row", row))
            .h(px(ROW_HEIGHT))
            .border_b_1()
            .border_color(colors.border_variant)
            .children(widths.iter().enumerate().map(|(column, width)| {
                let selected = self
                    .selection
                    .is_some_and(|selection| selection.contains(row, column));
                let is_head = self.selected_cell() == Some((row, column));
                let value = self.pages.cell(row, column);
                let (text, color) = match value {
                    Some(value) => (single_line(value).into_owned(), Color::Default),
                    None => (NULL_TEXT.to_string(), Color::Muted),
                };
                div()
                    .id(("result-cell", row * column_count + column))
                    .w(px(*width))
                    .flex_none()
                    .h_full()
                    .px_2()
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .border_r_1()
                    .border_color(colors.border_variant)
                    .font_buffer(cx)
                    .text_sm()
                    .text_color(color.color(cx))
                    .when(selected, |cell| cell.bg(colors.element_selected))
                    .when(is_head, |cell| {
                        cell.border_1().border_color(colors.border_focused)
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.on_cell_down(row, column, event, window, cx)
                        }),
                    )
                    .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                        this.on_cell_move(row, column, event, cx)
                    }))
                    .child(text)
            }))
            .into_any_element()
    }
}

impl Render for ResultGrid {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let widths = self.widths(window, cx);
        self.last_widths = widths.clone();
        let total_width: f32 = widths.iter().sum();
        let row_count = self.pages.row_count();
        let widths_for_rows = widths.clone();
        let mut rows = uniform_list(
            "result-rows",
            row_count,
            cx.processor(move |this, range: Range<usize>, _window, cx| {
                this.rows_visible(range.clone(), cx);
                range
                    .map(|row| this.render_row(row, &widths_for_rows, cx))
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .track_scroll(&self.scroll_handle);
        // Each scroller takes only its own axis: otherwise a vertical wheel also slides the
        // columns (the outer div takes `delta.y` as x) and a sideways swipe scrolls the rows.
        rows.interactivity().base_style.restrict_scroll_to_axis = Some(true);
        // The columns scroll sideways inside a frame that holds the vertical scrollbar, so that
        // it stays at the right edge however far the columns are scrolled.
        let columns = div()
            .id("result-grid-columns")
            .restrict_scroll_to_axis()
            .size_full()
            .overflow_x_scroll()
            .track_scroll(&self.horizontal_scroll)
            .child(
                v_flex()
                    .w(px(total_width))
                    .h_full()
                    .child(self.render_header(&widths, cx))
                    .child(rows),
            )
            .custom_scrollbars(
                Scrollbars::new(ScrollAxes::Horizontal)
                    .id("result-grid-horizontal")
                    .tracked_scroll_handle(&self.horizontal_scroll),
                window,
                cx,
            );
        let grid = div()
            .id("result-grid")
            .key_context("QueryResultGrid")
            .track_focus(&self.focus_handle)
            .size_full()
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| this.dragging = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| this.dragging = false),
            )
            .child(columns)
            .custom_scrollbars(
                Scrollbars::new(ScrollAxes::Vertical)
                    .id("result-grid-vertical")
                    .tracked_scroll_handle(&self.scroll_handle),
                window,
                cx,
            );
        self.register_actions(grid, cx)
    }
}

impl Focusable for ResultGrid {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
#[path = "result_grid_tests.rs"]
mod tests;
