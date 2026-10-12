use super::*;

fn columns() -> Vec<ColumnInfo> {
    vec![
        ColumnInfo {
            name: "id".into(),
            type_name: "int4".into(),
        },
        ColumnInfo {
            name: "description".into(),
            type_name: "text".into(),
        },
    ]
}

fn page(start: usize, count: usize) -> Vec<Vec<Option<String>>> {
    (start..start + count)
        .map(|ix| {
            vec![
                Some(ix.to_string()),
                (ix % 2 == 0).then(|| format!("row {ix}")),
            ]
        })
        .collect()
}

#[test]
fn the_first_page_is_wanted_right_away() {
    let mut pages = GridPages::new(columns());
    assert!(pages.needs_more(0));
    assert_eq!(pages.start_fetch(), Some(0));
    // One fetch at a time.
    assert!(!pages.needs_more(0));
    assert_eq!(pages.start_fetch(), None);
}

#[test]
fn more_is_wanted_within_100_rows_of_the_end() {
    let mut pages = GridPages::new(columns());
    pages.start_fetch();
    pages.page_loaded(page(0, 500), false);
    assert_eq!(pages.row_count(), 500);
    assert!(!pages.needs_more(0));
    assert!(!pages.needs_more(399));
    assert!(pages.needs_more(400));
    assert_eq!(pages.start_fetch(), Some(500));
    pages.page_loaded(page(500, 500), false);
    assert_eq!(pages.row_count(), 1000);
    assert_eq!(pages.cell(999, 0), Some("999"));
    assert!(pages.needs_more(950));
}

#[test]
fn nothing_more_once_done() {
    let mut pages = GridPages::new(columns());
    pages.start_fetch();
    pages.page_loaded(page(0, 20), true);
    assert!(pages.is_done());
    assert!(!pages.needs_more(20));
    assert_eq!(pages.start_fetch(), None);
}

#[test]
fn an_expired_cursor_stops_paging() {
    let mut pages = GridPages::new(columns());
    pages.start_fetch();
    pages.page_loaded(page(0, 500), false);
    pages.start_fetch();
    pages.page_failed(FetchFailure::Expired);
    assert!(!pages.needs_more(499));
    assert_eq!(pages.start_fetch(), None);
    // What was fetched stays visible.
    assert_eq!(pages.row_count(), 500);
    assert_eq!(pages.problem(), Some(RESULT_EXPIRED));
}

#[test]
fn a_failed_page_stops_paging_with_the_message() {
    let mut pages = GridPages::new(columns());
    pages.start_fetch();
    pages.page_failed(FetchFailure::Error("connection reset".into()));
    assert!(!pages.needs_more(0));
    assert_eq!(pages.problem(), Some("connection reset"));
}

#[test]
fn null_cells_are_none() {
    let mut pages = GridPages::new(columns());
    pages.start_fetch();
    pages.page_loaded(page(0, 2), true);
    assert_eq!(pages.cell(0, 1), Some("row 0"));
    assert_eq!(pages.cell(1, 1), None);
    assert_eq!(pages.cell(5, 0), None);
}

#[test]
fn status_line() {
    let mut pages = GridPages::new(columns());
    pages.start_fetch();
    pages.page_loaded(page(0, 500), false);
    assert_eq!(pages.status(12), "500 rows fetched+ in 12 ms");
    pages.start_fetch();
    pages.page_loaded(page(500, 1), true);
    assert_eq!(pages.status(12), "501 rows fetched in 12 ms");

    let mut one = GridPages::new(columns());
    one.start_fetch();
    one.page_loaded(page(0, 1), true);
    assert_eq!(one.status(3), "1 row fetched in 3 ms");
}

#[test]
fn column_widths_come_from_the_first_page() {
    let mut pages = GridPages::new(columns());
    pages.start_fetch();
    let mut first = page(0, 3);
    first[1][1] = Some("x".repeat(30));
    pages.page_loaded(first, false);
    // Header: name, a space and the type; cells: the longest value; null as "<null>".
    assert_eq!(pages.column_chars(), &[7, 30]);
    pages.start_fetch();
    let mut second = page(3, 1);
    second[0][1] = Some("y".repeat(300));
    pages.page_loaded(second, true);
    assert_eq!(pages.column_chars(), &[7, 30], "later pages do not resize");
}

#[test]
fn an_empty_first_page_sizes_by_the_header() {
    let mut pages = GridPages::new(columns());
    pages.start_fetch();
    pages.page_loaded(Vec::new(), true);
    assert_eq!(
        pages.column_chars(),
        &["id int4".len(), "description text".len()]
    );
}

#[test]
fn widths_are_clamped() {
    assert_eq!(column_width(1, 8.), MIN_COLUMN_WIDTH);
    assert_eq!(column_width(10, 8.), 10. * 8. + CELL_PADDING);
    assert_eq!(column_width(1000, 8.), MAX_COLUMN_WIDTH);
}

#[test]
fn cell_text_stays_on_one_line() {
    assert_eq!(single_line("a\nb\r\nc\td"), "a b  c d");
    assert_eq!(single_line("plain"), "plain");
}
