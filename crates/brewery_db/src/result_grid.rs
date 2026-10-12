//! The result grid of a query console: the rows of one daemon cursor, pulled in pages of 500 as
//! the user scrolls, in a virtualised list with a header row and horizontal scrolling.

use crate::CopyResultCell;
use brewery_daemon_client::{
    ColumnInfo, DB_NOT_FOUND, FetchParams, ProjectDaemon, daemon_error_code, daemon_error_message,
};
use gpui::{
    ClickEvent, ClipboardItem, Context, Entity, FocusHandle, Focusable, Render, ScrollHandle, Task,
    UniformListScrollHandle, Window, px, uniform_list,
};
use std::borrow::Cow;
use std::ops::Range;
use ui::prelude::*;

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
    /// Characters per column, measured on the first page.
    column_chars: Vec<usize>,
}

impl GridPages {
    pub fn new(columns: Vec<ColumnInfo>) -> Self {
        let column_chars = columns.iter().map(header_chars).collect();
        Self {
            columns,
            rows: Vec::new(),
            state: PageState::Idle,
            column_chars,
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
        if self.rows.is_empty() {
            for row in &rows {
                for (chars, value) in self.column_chars.iter_mut().zip(row) {
                    let len = value.as_deref().unwrap_or(NULL_TEXT).chars().count();
                    *chars = (*chars).max(len);
                }
            }
        }
        self.rows.extend(rows);
        self.state = if done {
            PageState::Done
        } else {
            PageState::Idle
        };
    }

    pub fn page_failed(&mut self, failure: FetchFailure) {
        self.state = PageState::Stopped(failure);
    }

    /// Why paging stopped early, if it did.
    pub fn problem(&self) -> Option<&str> {
        match &self.state {
            PageState::Stopped(FetchFailure::Expired) => Some(RESULT_EXPIRED),
            PageState::Stopped(FetchFailure::Error(message)) => Some(message),
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
    selected: Option<(usize, usize)>,
    fetch_task: Option<Task<()>>,
    /// The rows the list rendered last.
    last_visible: Option<Range<usize>>,
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
            selected: None,
            fetch_task: None,
            last_visible: None,
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

    pub fn scroll_handle(&self) -> &UniformListScrollHandle {
        &self.scroll_handle
    }

    /// Hands the cursor over for closing; no more pages are fetched afterwards.
    pub fn take_cursor(&mut self) -> Option<String> {
        self.fetch_task = None;
        if self.pages.is_loading() {
            self.pages.page_loaded(Vec::new(), true);
        }
        self.cursor_id.take()
    }

    /// Called with the rows the list is about to show; fetches the next page when they come
    /// close to the end.
    pub fn rows_visible(&mut self, visible: Range<usize>, cx: &mut Context<Self>) {
        let end = visible.end;
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
                    Ok(page) => this.pages.page_loaded(page.rows, page.done),
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

    pub fn selected_cell(&self) -> Option<(usize, usize)> {
        self.selected
    }

    pub fn select_cell(&mut self, row: usize, column: usize, cx: &mut Context<Self>) {
        if self.pages.has_cell(row, column) {
            self.selected = Some((row, column));
            cx.notify();
        }
    }

    /// Copies the selected value; SQL NULL copies as an empty string.
    pub fn copy(&mut self, _: &CopyResultCell, _: &mut Window, cx: &mut Context<Self>) {
        let Some((row, column)) = self.selected else {
            return;
        };
        let value = self.pages.cell(row, column).unwrap_or_default().to_string();
        cx.write_to_clipboard(ClipboardItem::new_string(value));
    }

    fn widths(&self, window: &Window) -> Vec<f32> {
        // The cells use the buffer font at the small text size; monospace, so a char count does.
        let char_width = f32::from(window.rem_size()) * 0.875 * 0.6;
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
                let selected = self.selected == Some((row, column));
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
                    .when(selected, |cell| {
                        cell.bg(colors.element_selected)
                            .border_1()
                            .border_color(colors.border_focused)
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        window.focus(&this.focus_handle, cx);
                        this.select_cell(row, column, cx);
                    }))
                    .child(text)
            }))
            .into_any_element()
    }
}

impl Render for ResultGrid {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let widths = self.widths(window);
        let total_width: f32 = widths.iter().sum();
        let row_count = self.pages.row_count();
        div()
            .id("result-grid")
            .key_context("QueryResultGrid")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::copy))
            .size_full()
            .overflow_x_scroll()
            .track_scroll(&self.horizontal_scroll)
            .child(
                v_flex()
                    .w(px(total_width))
                    .h_full()
                    .child(self.render_header(&widths, cx))
                    .child(
                        uniform_list(
                            "result-rows",
                            row_count,
                            cx.processor(move |this, range: Range<usize>, _window, cx| {
                                this.rows_visible(range.clone(), cx);
                                range
                                    .map(|row| this.render_row(row, &widths, cx))
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .flex_1()
                        .track_scroll(&self.scroll_handle),
                    ),
            )
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
