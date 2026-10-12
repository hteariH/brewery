use super::*;

fn texts<'a>(text: &'a str, ranges: &[Range<usize>]) -> Vec<&'a str> {
    ranges.iter().map(|range| &text[range.clone()]).collect()
}

fn at(text: &str, offset: usize) -> Option<&str> {
    statement_at(text, offset).map(|range| &text[range])
}

/// The text with `|` marking the cursor, and the statement found there.
fn at_cursor(marked: &str) -> Option<String> {
    let offset = marked.find('|').expect("a cursor marker");
    let text = marked.replacen('|', "", 1);
    statement_at(&text, offset).map(|range| text[range].to_string())
}

#[test]
fn splits_on_semicolons_and_trims() {
    let text = "select 1;\n  select 2 ;\nselect 3";
    assert_eq!(
        texts(text, &split_statements(text)),
        ["select 1", "select 2", "select 3"]
    );
}

#[test]
fn empty_statements_are_skipped() {
    let text = ";;  select 1;\n;\n -- only a comment\n; /* and another */ ;select 2;;";
    assert_eq!(
        texts(text, &split_statements(text)),
        ["select 1", "select 2"]
    );
    assert!(split_statements("").is_empty());
    assert!(split_statements("  ;\n -- nothing\n").is_empty());
}

#[test]
fn semicolons_in_strings_and_identifiers_do_not_split() {
    let text = "insert into t values ('a;b', 'it''s; fine');\nselect \"odd;name\", `x;y` from t";
    assert_eq!(
        texts(text, &split_statements(text)),
        [
            "insert into t values ('a;b', 'it''s; fine')",
            "select \"odd;name\", `x;y` from t"
        ]
    );
}

#[test]
fn semicolons_in_comments_do_not_split() {
    let text = "select 1 -- not here; really\n, 2;\nselect /* nor; here /* nested; */ still; */ 3";
    assert_eq!(
        texts(text, &split_statements(text)),
        [
            "select 1 -- not here; really\n, 2",
            "select /* nor; here /* nested; */ still; */ 3"
        ]
    );
}

#[test]
fn leading_and_trailing_comments_are_not_sent() {
    let text = "-- the owners\nselect * from owners -- all of them\n;";
    assert_eq!(
        texts(text, &split_statements(text)),
        ["select * from owners"]
    );
}

#[test]
fn dollar_quoted_bodies_do_not_split() {
    let text = "create function f() returns int as $$ begin; return 1; end; $$ language plpgsql;\n\
                do $body$ begin perform 1; end $body$;\n\
                select $1, a$b from t";
    assert_eq!(
        texts(text, &split_statements(text)),
        [
            "create function f() returns int as $$ begin; return 1; end; $$ language plpgsql",
            "do $body$ begin perform 1; end $body$",
            "select $1, a$b from t"
        ]
    );
}

#[test]
fn an_unterminated_string_runs_to_the_end() {
    let text = "select 'open; still open";
    assert_eq!(texts(text, &split_statements(text)), [text]);
}

#[test]
fn statement_at_the_cursor() {
    assert_eq!(
        at_cursor("select 1;\nsel|ect 2;\nselect 3").as_deref(),
        Some("select 2")
    );
    assert_eq!(
        at_cursor("|select 1; select 2").as_deref(),
        Some("select 1")
    );
    // Right before or right after the `;` of a statement, still that statement.
    assert_eq!(
        at_cursor("select 1|; select 2").as_deref(),
        Some("select 1")
    );
    assert_eq!(
        at_cursor("select 1;|\nselect 2").as_deref(),
        Some("select 1")
    );
    // Touching the next statement picks the next one.
    assert_eq!(at_cursor("select 1;|select 2").as_deref(), Some("select 2"));
    assert_eq!(
        at_cursor("select 1;\n|select 2").as_deref(),
        Some("select 2")
    );
}

#[test]
fn cursor_after_the_last_semicolon_picks_the_last_statement() {
    assert_eq!(
        at_cursor("select 1;\nselect 2;\n\n  |").as_deref(),
        Some("select 2")
    );
    assert_eq!(
        at_cursor("select 1;\nselect 2; -- done|").as_deref(),
        Some("select 2")
    );
}

#[test]
fn cursor_in_a_blank_gap_picks_the_statement_above() {
    assert_eq!(
        at_cursor("select 1;\n\n|\n\nselect 2;").as_deref(),
        Some("select 1")
    );
    // Nothing above: the one below.
    assert_eq!(at_cursor("\n|\n;\nselect 2;").as_deref(), Some("select 2"));
}

#[test]
fn nothing_to_run_in_an_empty_console() {
    assert_eq!(at("", 0), None);
    assert_eq!(at("  -- just a note\n", 3), None);
    // An offset past the end is clamped.
    assert_eq!(at("select 1", 100), Some("select 1"));
}

#[test]
fn selection_runs_the_selected_statements() {
    let text = "select 1;\nselect 2;\nselect 3;";
    // Empty selection: the statement at the cursor.
    let cursor = text.find("2").unwrap();
    assert_eq!(
        texts(text, &statements_to_run(text, cursor..cursor)),
        ["select 2"]
    );
    // A selection inside one statement runs just the selected text.
    let start = text.find("select 3").unwrap();
    assert_eq!(
        texts(text, &statements_to_run(text, start..start + 8)),
        ["select 3"]
    );
    // A selection across statements yields each of them, trimmed, in selection offsets.
    let start = text.find("2").unwrap() - 7;
    let end = text.len();
    assert_eq!(
        texts(text, &statements_to_run(text, start..end)),
        ["select 2", "select 3"]
    );
    // A selection of only whitespace runs nothing.
    assert!(statements_to_run(text, 9..10).is_empty());
}

#[test]
fn identifiers_are_quoted_only_when_needed() {
    assert_eq!(quote_identifier("postgresql", "owners"), "owners");
    assert_eq!(quote_identifier("postgresql", "pet_types2"), "pet_types2");
    // PostgreSQL folds unquoted names to lower case.
    assert_eq!(quote_identifier("postgresql", "Owners"), "\"Owners\"");
    assert_eq!(quote_identifier("postgresql", "my table"), "\"my table\"");
    assert_eq!(quote_identifier("postgresql", "2fa"), "\"2fa\"");
    assert_eq!(quote_identifier("postgresql", "user"), "\"user\"");
    assert_eq!(quote_identifier("postgresql", "a\"b"), "\"a\"\"b\"");
    assert_eq!(quote_identifier("postgresql", ""), "\"\"");
    // SQLite and MySQL do not fold case.
    assert_eq!(quote_identifier("sqlite", "Owners"), "Owners");
    assert_eq!(quote_identifier("sqlite", "order"), "\"order\"");
    assert_eq!(quote_identifier("mysql", "Owners"), "Owners");
    assert_eq!(quote_identifier("mysql", "my-table"), "`my-table`");
    assert_eq!(quote_identifier("mariadb", "a`b"), "`a``b`");
    assert_eq!(quote_identifier("mariadb", "select"), "`select`");
}

#[test]
fn select_all_from_a_table_path() {
    assert_eq!(
        select_all_from("postgresql", &["public".into(), "owners".into()]),
        "SELECT * FROM public.owners"
    );
    assert_eq!(
        select_all_from("mysql", &["shop".into(), "Order Items".into()]),
        "SELECT * FROM shop.`Order Items`"
    );
    assert_eq!(
        select_all_from("sqlite", &["pets".into()]),
        "SELECT * FROM pets"
    );
}
