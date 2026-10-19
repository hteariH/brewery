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
fn a_truncated_result_says_so() {
    let mut pages = GridPages::new(columns());
    pages.start_fetch();
    pages.page_loaded(page(0, 500), false);
    assert_eq!(pages.problem(), None);
    pages.start_fetch();
    pages.page_loaded(page(500, 2), true);
    pages.mark_truncated();
    assert!(pages.is_done());
    assert_eq!(pages.status(12), "502 rows fetched in 12 ms");
    assert_eq!(pages.problem(), Some(RESULT_TRUNCATED));
}

#[test]
fn dropped_results_are_mentioned_once_the_rows_are_in() {
    let mut pages = GridPages::new(columns());
    pages.start_fetch();
    pages.page_loaded(page(0, 2), true);
    assert_eq!(pages.problem(), None);
    pages.mark_more_results();
    assert_eq!(pages.problem(), Some(MORE_RESULTS_DROPPED));
}

#[test]
fn column_widths_grow_with_later_pages() {
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
    assert_eq!(
        pages.column_chars(),
        &[7, 300],
        "a later page widens its column"
    );
}

fn grid_with(rows: Vec<Vec<Option<String>>>) -> GridPages {
    let mut pages = GridPages::new(columns());
    pages.start_fetch();
    pages.page_loaded(rows, true);
    pages
}

#[test]
fn one_cell_copies_as_its_value_and_a_range_as_tsv() {
    let pages = grid_with(page(0, 4));
    assert_eq!(pages.copy_text(&GridSelection::cell(2, 1)), "row 2");
    // NULL copies as an empty value.
    assert_eq!(pages.copy_text(&GridSelection::cell(1, 1)), "");
    let range = GridSelection {
        anchor: (2, 1),
        head: (0, 0),
    };
    assert_eq!(pages.copy_text(&range), "0\trow 0\n1\t\n2\trow 2");
}

#[test]
fn tsv_quotes_values_with_tabs_breaks_or_quotes() {
    let pages = grid_with(vec![
        vec![Some("a\tb".into()), Some("say \"hi\"".into())],
        vec![Some("line\nbreak".into()), Some("plain".into())],
    ]);
    let all = GridSelection {
        anchor: (0, 0),
        head: (1, 1),
    };
    assert_eq!(
        pages.copy_text(&all),
        "\"a\tb\"\t\"say \"\"hi\"\"\"\n\"line\nbreak\"\tplain"
    );
    // A single cell copies raw.
    assert_eq!(pages.copy_text(&GridSelection::cell(0, 1)), "say \"hi\"");
}

#[test]
fn a_selection_covers_the_rectangle_between_anchor_and_head() {
    let selection = GridSelection {
        anchor: (5, 2),
        head: (3, 0),
    };
    assert_eq!(selection.ranges(), (3..6, 0..3));
    assert!(selection.contains(4, 1));
    assert!(!selection.contains(6, 1));
    assert_eq!(
        selection.moved_to((7, 1), true),
        GridSelection {
            anchor: (5, 2),
            head: (7, 1)
        }
    );
    assert_eq!(selection.moved_to((7, 1), false), GridSelection::cell(7, 1));
}

#[test]
fn moves_stay_inside_the_grid() {
    let (rows, columns) = (10, 3);
    assert_eq!(moved_cell((0, 0), CellMove::Up, rows, columns), (0, 0));
    assert_eq!(moved_cell((0, 0), CellMove::Left, rows, columns), (0, 0));
    assert_eq!(moved_cell((0, 0), CellMove::Down, rows, columns), (1, 0));
    assert_eq!(moved_cell((9, 2), CellMove::Down, rows, columns), (9, 2));
    assert_eq!(moved_cell((9, 2), CellMove::Right, rows, columns), (9, 2));
    assert_eq!(
        moved_cell((4, 1), CellMove::RowStart, rows, columns),
        (4, 0)
    );
    assert_eq!(moved_cell((4, 1), CellMove::RowEnd, rows, columns), (4, 2));
    assert_eq!(moved_cell((4, 1), CellMove::First, rows, columns), (0, 0));
    assert_eq!(moved_cell((4, 1), CellMove::Last, rows, columns), (9, 2));
}

#[test]
fn revealing_a_column_scrolls_as_little_as_possible() {
    // A 300 px view scrolled 100 px to the right shows 100..400.
    assert_eq!(
        offset_to_reveal(-100., 300., 150., 100.),
        -100.,
        "already visible"
    );
    assert_eq!(
        offset_to_reveal(-100., 300., 50., 100.),
        -50.,
        "left of the view"
    );
    assert_eq!(
        offset_to_reveal(-100., 300., 380., 100.),
        -180.,
        "right of the view"
    );
    assert_eq!(
        offset_to_reveal(0., 300., 500., 400.),
        -500.,
        "wider than the view"
    );
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

#[test]
fn an_empty_page_ends_the_result_even_when_not_done() {
    // A daemon that answers "no rows, more to come" would otherwise be asked again and again.
    let mut pages = GridPages::new(columns());
    pages.start_fetch();
    pages.page_loaded(page(0, 500), false);
    pages.start_fetch();
    pages.page_loaded(Vec::new(), false);
    assert!(pages.is_done());
    assert!(!pages.needs_more(500));
    assert_eq!(pages.status(1), "500 rows fetched in 1 ms");
}
