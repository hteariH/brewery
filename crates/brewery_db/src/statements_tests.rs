use super::*;

fn texts<'a>(text: &'a str, ranges: &[Range<usize>]) -> Vec<&'a str> {
    ranges.iter().map(|range| &text[range.clone()]).collect()
}

fn at(text: &str, offset: usize) -> Option<&str> {
    statement_at(text, offset, SqlFlavor::Standard).map(|range| &text[range])
}

/// The text with `|` marking the cursor, and the statement found there.
fn at_cursor(marked: &str) -> Option<String> {
    let offset = marked.find('|').expect("a cursor marker");
    let text = marked.replacen('|', "", 1);
    statement_at(&text, offset, SqlFlavor::Standard).map(|range| text[range].to_string())
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
        texts(
            text,
            &statements_to_run(text, cursor..cursor, SqlFlavor::Standard)
        ),
        ["select 2"]
    );
    // A selection inside one statement runs just the selected text.
    let start = text.find("select 3").unwrap();
    assert_eq!(
        texts(
            text,
            &statements_to_run(text, start..start + 8, SqlFlavor::Standard)
        ),
        ["select 3"]
    );
    // A selection across statements yields each of them, trimmed, in selection offsets.
    let start = text.find("2").unwrap() - 7;
    let end = text.len();
    assert_eq!(
        texts(
            text,
            &statements_to_run(text, start..end, SqlFlavor::Standard)
        ),
        ["select 2", "select 3"]
    );
    // A selection of only whitespace runs nothing.
    assert!(statements_to_run(text, 9..10, SqlFlavor::Standard).is_empty());
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

#[test]
fn select_top_and_the_qualified_name_of_a_table() {
    let owners: Vec<String> = vec!["public".into(), "Owners".into()];
    assert_eq!(qualified_name("postgresql", &owners), "public.\"Owners\"");
    assert_eq!(
        select_top_from("postgresql", &owners, 500),
        "SELECT * FROM public.\"Owners\" LIMIT 500"
    );
    assert_eq!(
        select_top_from("mariadb", &["shop".into(), "items".into()], 500),
        "SELECT * FROM shop.items LIMIT 500"
    );
    assert_eq!(
        select_top_from("h2", &["public".into(), "t".into()], 500),
        "SELECT * FROM public.t",
        "no LIMIT where the driver's dialect is unknown"
    );
}

#[test]
fn common_non_reserved_keywords_are_quoted_too() {
    for word in [
        "rank", "row", "interval", "window", "level", "comment", "range",
    ] {
        assert_eq!(
            quote_identifier("postgresql", word),
            format!("\"{word}\""),
            "{word}"
        );
    }
    assert_eq!(quote_identifier("mysql", "rank"), "`rank`");
}

fn split_as(driver_id: &str, text: &str) -> Vec<String> {
    split_statements_for(text, SqlFlavor::for_driver(driver_id))
        .into_iter()
        .map(|range| text[range].to_string())
        .collect()
}

#[test]
fn mysql_hash_comments_and_backslash_escapes() {
    let text = "select 'it\\'s; fine' # a note; not code\n;\nselect \"a\\\"; b\"; select 3";
    assert_eq!(
        split_as("mysql", text),
        ["select 'it\\'s; fine'", "select \"a\\\"; b\"", "select 3"]
    );
    // Elsewhere `#` is no comment and a backslash escapes nothing.
    assert_eq!(
        split_as("postgresql", "select 'a\\'; select 2"),
        ["select 'a\\'", "select 2"]
    );
}

#[test]
fn postgresql_e_strings_take_backslash_escapes() {
    let text = "select E'it\\'s; one'; select e'\\\\'; select 3";
    assert_eq!(
        split_as("postgresql", text),
        ["select E'it\\'s; one'", "select e'\\\\'", "select 3"]
    );
    // `E` at the end of a longer word starts no E-string.
    assert_eq!(
        split_as("postgresql", "select name'x'; select 2"),
        ["select name'x'", "select 2"]
    );
}

#[test]
fn trigger_and_routine_bodies_stay_whole() {
    let trigger = "CREATE TRIGGER t AFTER INSERT ON a BEGIN\n  UPDATE b SET n = n + 1;\n  DELETE FROM c;\nEND";
    let text = format!("{trigger};\nselect 1");
    assert_eq!(split_as("sqlite", &text), [trigger, "select 1"]);

    let routine = "CREATE PROCEDURE p() BEGIN\n  IF x THEN SELECT 1; END IF;\n  CASE y WHEN 1 THEN SELECT 2; END CASE;\n  SET z = CASE WHEN a THEN 1 ELSE 2 END;\n  BEGIN SELECT 3; END;\n  IF b THEN SELECT 5; END IF;\nEND";
    let text = format!("{routine}; select 4");
    assert_eq!(split_as("mysql", &text), [routine, "select 4"]);

    let atomic =
        "CREATE FUNCTION f() RETURNS int LANGUAGE sql BEGIN ATOMIC SELECT 1; SELECT 2; END";
    let text = format!("{atomic}; select 5");
    assert_eq!(split_as("postgresql", &text), [atomic, "select 5"]);

    let anonymous = "BEGIN NOT ATOMIC SELECT 1; SELECT 2; END";
    let text = format!("{anonymous}; select 6");
    assert_eq!(split_as("mariadb", &text), [anonymous, "select 6"]);
}

#[test]
fn begin_as_a_transaction_still_ends_at_its_semicolon() {
    assert_eq!(
        split_as("postgresql", "BEGIN; UPDATE a SET b = 1; COMMIT"),
        ["BEGIN", "UPDATE a SET b = 1", "COMMIT"]
    );
    assert_eq!(
        split_as(
            "postgresql",
            "CREATE TABLE begin_log (id int); select case when 1 = 1 then 1 end; select 2"
        ),
        [
            "CREATE TABLE begin_log (id int)",
            "select case when 1 = 1 then 1 end",
            "select 2"
        ]
    );
}

#[test]
fn the_statement_at_the_cursor_inside_a_trigger_is_the_whole_trigger() {
    let text =
        "select 0;\nCREATE TRIGGER t AFTER INSERT ON a BEGIN UPDATE b SET n = 1; END;\nselect 2";
    let offset = text.find("UPDATE").unwrap();
    let found = statement_at(text, offset, SqlFlavor::for_driver("sqlite")).map(|r| &text[r]);
    assert_eq!(
        found,
        Some("CREATE TRIGGER t AFTER INSERT ON a BEGIN UPDATE b SET n = 1; END")
    );
}
