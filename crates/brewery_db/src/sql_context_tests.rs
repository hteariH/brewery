use super::*;

/// The context at the `|` in `marked`.
fn at(marked: &str) -> SqlContext {
    let offset = marked.find('|').expect("a cursor marker");
    let text = marked.replacen('|', "", 1);
    context_at(&text, offset, SqlFlavor::Standard)
}

fn kind(marked: &str) -> ContextKind {
    at(marked).kind
}

fn tables() -> ContextKind {
    ContextKind::Tables { schema: None }
}

fn tables_of(schema: &str) -> ContextKind {
    ContextKind::Tables {
        schema: Some(schema.to_string()),
    }
}

fn columns_of(qualifier: &[&str]) -> ContextKind {
    ContextKind::ColumnsOf {
        qualifier: qualifier.iter().map(|part| part.to_string()).collect(),
    }
}

fn table(schema: Option<&str>, name: &str, alias: Option<&str>) -> TableRef {
    TableRef {
        schema: schema.map(str::to_string),
        name: name.to_string(),
        alias: alias.map(str::to_string),
    }
}

/// The table `qualifier` stands for in the statement at the cursor.
fn resolved(marked: &str) -> Option<TableRef> {
    let context = at(marked);
    let ContextKind::ColumnsOf { qualifier } = &context.kind else {
        panic!(
            "expected a qualified column context, got {:?}",
            context.kind
        );
    };
    context.resolve(qualifier).cloned()
}

#[test]
fn table_keywords_offer_tables() {
    for marked in [
        "SELECT * FROM |",
        "select * from ow|",
        "SELECT * FROM owners o JOIN |",
        "SELECT * FROM owners o LEFT OUTER JOIN p|",
        "INSERT INTO |",
        "UPDATE |",
        "DELETE FROM |",
        "TRUNCATE TABLE |",
        "DESCRIBE |",
    ] {
        assert_eq!(kind(marked), tables(), "{marked}");
    }
}

#[test]
fn the_prefix_is_what_was_typed_of_the_word() {
    let context = at("SELECT * FROM ow|");
    assert_eq!(context.prefix, "ow");
    assert_eq!(context.prefix_start, 14);

    let context = at("SELECT * FROM |");
    assert_eq!(context.prefix, "");
    assert_eq!(context.prefix_start, 14);

    // In the middle of a word, only the part before the cursor counts.
    let context = at("SELECT * FROM ow|ners");
    assert_eq!(context.prefix, "ow");
    assert_eq!(context.prefix_start, 14);

    // A quoted identifier being typed: the replacement starts at the quote.
    let context = at("SELECT * FROM \"Ow|");
    assert_eq!(context.kind, tables());
    assert_eq!(context.prefix, "Ow");
    assert_eq!(context.prefix_start, 14);
}

#[test]
fn after_a_schema_and_a_dot_only_its_tables() {
    assert_eq!(kind("SELECT * FROM public.|"), tables_of("public"));
    let context = at("SELECT * FROM public.ow|");
    assert_eq!(context.kind, tables_of("public"));
    assert_eq!(context.prefix, "ow");
    assert_eq!(context.prefix_start, 21);
    assert_eq!(
        kind("SELECT * FROM owners o JOIN \"Billing\".|"),
        tables_of("Billing")
    );
    assert_eq!(kind("SELECT * FROM a, public.|"), tables_of("public"));
}

#[test]
fn comma_separated_from_lists_offer_tables() {
    let context = at("SELECT * FROM owners o, pe|");
    assert_eq!(context.kind, tables());
    assert_eq!(context.tables, vec![table(None, "owners", Some("o"))]);
    assert_eq!(
        kind("SELECT * FROM owners o, pets p, |"),
        tables(),
        "after the second table too"
    );
}

#[test]
fn an_alias_and_a_dot_offer_its_columns() {
    let context = at("SELECT o.| FROM owners o");
    assert_eq!(context.kind, columns_of(&["o"]));
    assert_eq!(context.prefix, "");
    assert_eq!(
        context.resolve(&["o".to_string()]),
        Some(&table(None, "owners", Some("o")))
    );

    let context = at("SELECT o.em| FROM owners o");
    assert_eq!(context.prefix, "em");
    assert_eq!(context.prefix_start, 9);
}

#[test]
fn aliases_come_from_as_joins_and_comma_lists() {
    assert_eq!(
        resolved("SELECT p.| FROM owners AS o JOIN public.pets AS p ON p.owner_id = o.id"),
        Some(table(Some("public"), "pets", Some("p")))
    );
    assert_eq!(
        resolved("SELECT p.| FROM owners o JOIN public.pets p ON p.owner_id = o.id"),
        Some(table(Some("public"), "pets", Some("p")))
    );
    assert_eq!(
        resolved("SELECT * FROM owners o, pets p WHERE p.|"),
        Some(table(None, "pets", Some("p")))
    );
    assert_eq!(
        resolved("SELECT x.| FROM \"My Table\" x"),
        Some(table(None, "My Table", Some("x")))
    );
    // Aliases are matched case-insensitively, as unquoted names are.
    assert_eq!(
        resolved("SELECT O.| FROM owners o"),
        Some(table(None, "owners", Some("o")))
    );
}

#[test]
fn the_bare_table_name_works_as_a_qualifier() {
    assert_eq!(
        resolved("SELECT owners.| FROM owners"),
        Some(table(None, "owners", None))
    );
    assert_eq!(
        resolved("SELECT owners.| FROM public.owners o"),
        Some(table(Some("public"), "owners", Some("o")))
    );
    // Not in the statement: the completion looks the name up in the schema itself.
    assert_eq!(resolved("SELECT pets.| FROM owners"), None);
    assert_eq!(
        kind("SELECT public.owners.| FROM public.owners"),
        columns_of(&["public", "owners"])
    );
}

#[test]
fn column_positions_offer_the_columns_of_the_statement() {
    for marked in [
        "SELECT |",
        "SELECT id, | FROM owners",
        "SELECT * FROM owners WHERE |",
        "SELECT * FROM owners WHERE id = 1 AND |",
        "SELECT * FROM owners WHERE id = 1 OR em|",
        "SELECT * FROM owners o JOIN pets p ON |",
        "SELECT * FROM owners ORDER BY |",
        "SELECT * FROM owners GROUP BY id HAVING |",
        "UPDATE owners SET |",
        "UPDATE owners SET email = 'x', |",
        "SELECT count(|) FROM owners",
        "INSERT INTO owners (|",
        "INSERT INTO owners (id, |",
        "SELECT * FROM owners WHERE id = |",
        "SELECT DISTINCT |",
    ] {
        assert_eq!(kind(marked), ContextKind::Columns, "{marked}");
    }
}

#[test]
fn tables_after_the_cursor_count() {
    let context = at("SELECT | FROM owners");
    assert_eq!(context.kind, ContextKind::Columns);
    assert_eq!(context.tables, vec![table(None, "owners", None)]);

    let context =
        at("SELECT |, name FROM owners o JOIN pets p ON p.owner_id = o.id WHERE o.id = 1");
    assert_eq!(
        context.tables,
        vec![
            table(None, "owners", Some("o")),
            table(None, "pets", Some("p"))
        ]
    );
}

#[test]
fn keywords_are_not_aliases() {
    let context = at("SELECT * FROM owners WHERE |");
    assert_eq!(context.tables, vec![table(None, "owners", None)]);
    let context = at("SELECT * FROM owners LEFT JOIN pets ON |");
    assert_eq!(
        context.tables,
        vec![table(None, "owners", None), table(None, "pets", None)]
    );
    let context = at("INSERT INTO owners (id) VALUES (|");
    assert_eq!(context.tables, vec![table(None, "owners", None)]);
    let context = at("UPDATE owners SET |");
    assert_eq!(context.tables, vec![table(None, "owners", None)]);
}

#[test]
fn the_word_being_typed_is_not_a_table() {
    let context = at("SELECT * FROM ow|");
    assert_eq!(context.tables, Vec::new());
}

#[test]
fn nothing_inside_strings_and_comments() {
    for marked in [
        "SELECT * FROM owners WHERE email = 'ow|'",
        "SELECT * FROM owners WHERE email = 'ow|",
        "SELECT * FROM owners -- FROM ow|",
        "SELECT * FROM owners -- FROM ow|\nWHERE",
        "SELECT * FROM /* ow| */ owners",
        "SELECT * FROM /* ow|",
        "SELECT $$ FROM ow| $$",
    ] {
        assert_eq!(kind(marked), ContextKind::Nothing, "{marked}");
    }
    // Right after a comment that is closed, completion works again.
    assert_eq!(kind("SELECT * FROM /* x */ |"), tables());
    assert_eq!(kind("-- a note\nSELECT * FROM |"), tables());
    // A number is not an identifier to complete.
    assert_eq!(kind("SELECT 12|"), ContextKind::Nothing);
}

#[test]
fn only_the_statement_under_the_cursor_counts() {
    let text = "SELECT * FROM pets p;\nSELECT p.| FROM owners p;\nSELECT 1 FROM vets p";
    assert_eq!(resolved(text), Some(table(None, "owners", Some("p"))));

    let context = at("SELECT * FROM pets; SELECT | FROM owners");
    assert_eq!(context.tables, vec![table(None, "owners", None)]);

    let context = at("SELECT | FROM pets; SELECT * FROM owners");
    assert_eq!(context.tables, vec![table(None, "pets", None)]);

    // Right after a `;` a new statement starts.
    assert_eq!(kind("SELECT * FROM owners;|"), ContextKind::Keywords);
    assert_eq!(kind("SELECT * FROM owners; |"), ContextKind::Keywords);
}

#[test]
fn anywhere_else_keywords() {
    for marked in [
        "|",
        "SEL|",
        "SELECT * FROM owners |",
        "SELECT * FROM owners o |",
        "SELECT * FROM owners WHERE id = 1 |",
    ] {
        assert_eq!(kind(marked), ContextKind::Keywords, "{marked}");
    }
}

#[test]
fn schema_changes_are_recognised_by_their_first_keyword() {
    for sql in [
        "CREATE TABLE t (id int)",
        "alter table t add column x int",
        "DROP VIEW v",
        "-- tidy up\nTRUNCATE t",
        "/* x */ rename table a to b",
    ] {
        assert!(is_schema_change(sql), "{sql}");
    }
    for sql in [
        "SELECT 1",
        "INSERT INTO t VALUES (1)",
        "",
        "-- CREATE TABLE",
    ] {
        assert!(!is_schema_change(sql), "{sql}");
    }
}

#[test]
fn inside_a_closed_quoted_name_the_replacement_runs_through_the_closing_quote() {
    let context = at("SELECT o.\"Fi|\" FROM owners o");
    assert_eq!(context.prefix, "Fi");
    assert_eq!(context.prefix_start, 9);
    assert_eq!(context.replace_end, 13);

    let context = at("SELECT o.\"Fi|rst\" FROM owners o");
    assert_eq!(context.prefix, "Fi");
    assert_eq!((context.prefix_start, context.replace_end), (9, 16));

    // Not closed yet, or not quoted: up to the cursor.
    let context = at("SELECT o.\"Fi|");
    assert_eq!((context.prefix_start, context.replace_end), (9, 12));
    let context = at("SELECT * FROM ow|ners");
    assert_eq!((context.prefix_start, context.replace_end), (14, 16));
}

#[test]
fn inside_a_word_the_rest_of_it_is_replaced_when_the_name_ends_with_it() {
    // Zed's `replace_suffix`: through the end of the word when the name ends with what follows
    // the cursor, else up to the cursor.
    let context = at("SELECT * FROM ow|ners");
    assert_eq!(context.replace_end_for("owners"), 20);
    assert_eq!(context.replace_end_for("OWNERS"), 20);
    assert_eq!(context.replace_end_for("owner_names"), 16);
    let context = at("SEL|ECT 1");
    assert_eq!(context.replace_end_for("SELECT"), 6);
    // At the end of the word, or in a quoted name: as before.
    let context = at("SELECT * FROM owners|");
    assert_eq!(context.replace_end_for("owners"), 20);
    let context = at("SELECT o.\"Fi|rst\" FROM owners o");
    assert_eq!(context.replace_end_for("First Name"), 16);
}

#[test]
fn subquery_aliases_resolve_in_their_own_level_first() {
    assert_eq!(
        resolved("SELECT * FROM a x WHERE id IN (SELECT id FROM b x WHERE x.|)"),
        Some(table(None, "b", Some("x")))
    );
    assert_eq!(
        resolved("SELECT * FROM a x WHERE id IN (SELECT id FROM b x WHERE x.|"),
        Some(table(None, "b", Some("x"))),
        "while the subquery is not closed yet"
    );
    assert_eq!(
        resolved("SELECT x.| FROM a x WHERE id IN (SELECT id FROM b x)"),
        Some(table(None, "a", Some("x")))
    );
    // A correlated subquery sees the outer aliases.
    assert_eq!(
        resolved("SELECT * FROM a x WHERE id IN (SELECT id FROM b y WHERE y.id = x.|)"),
        Some(table(None, "a", Some("x")))
    );
}

#[test]
fn column_positions_see_the_tables_of_the_levels_around_the_cursor() {
    let names = |marked: &str| -> Vec<String> {
        at(marked)
            .visible_tables()
            .into_iter()
            .map(|table| table.name.clone())
            .collect()
    };
    assert_eq!(
        names("SELECT | FROM a WHERE id IN (SELECT id FROM b)"),
        ["a"]
    );
    assert_eq!(
        names("SELECT * FROM a WHERE id IN (SELECT | FROM b)"),
        ["b", "a"]
    );
}

#[test]
fn a_derived_table_alias_stands_for_no_known_table() {
    let derived = table(None, "", Some("d"));
    assert_eq!(
        resolved("SELECT d.| FROM (SELECT id FROM owners) d"),
        Some(derived.clone())
    );
    assert_eq!(
        resolved("SELECT d.| FROM (SELECT id FROM owners) AS d, pets p"),
        Some(derived)
    );
    assert_eq!(
        resolved("SELECT p.| FROM (SELECT id FROM owners) AS d, pets p"),
        Some(table(None, "pets", Some("p")))
    );
    // The table inside is seen from inside only.
    let context = at("SELECT | FROM (SELECT id FROM owners) d");
    assert!(
        context
            .visible_tables()
            .iter()
            .all(|table| table.name != "owners")
    );
}

#[test]
fn parenthesised_from_lists() {
    assert_eq!(
        resolved("SELECT o.| FROM (owners o JOIN pets p ON p.owner_id = o.id)"),
        Some(table(None, "owners", Some("o")))
    );
    assert_eq!(
        resolved("SELECT p.| FROM (owners o JOIN pets p ON p.owner_id = o.id)"),
        Some(table(None, "pets", Some("p")))
    );
}

#[test]
fn a_star_in_the_select_list_is_a_finished_item() {
    for marked in [
        "SELECT * |",
        "SELECT o.* |",
        "SELECT id, * |",
        "SELECT DISTINCT * |",
    ] {
        assert_eq!(kind(marked), ContextKind::Keywords, "{marked}");
    }
    assert_eq!(kind("SELECT 2 * |"), ContextKind::Columns);
}

fn at_in(driver_id: &str, marked: &str) -> SqlContext {
    let offset = marked.find('|').expect("a cursor marker");
    let text = marked.replacen('|', "", 1);
    context_at(&text, offset, SqlFlavor::for_driver(driver_id))
}

#[test]
fn mysql_hash_comments_and_escaped_quotes_are_read_as_such() {
    // Inside a `#` comment nothing is completed; after it, the statement goes on.
    assert_eq!(
        at_in("mysql", "SELECT * FROM t # see |").kind,
        ContextKind::Nothing
    );
    assert_eq!(at_in("mysql", "SELECT 1 # note\nFROM |").kind, tables());
    // Elsewhere `#` is not a comment.
    assert_ne!(
        at_in("postgresql", "SELECT * FROM t # see |").kind,
        ContextKind::Nothing
    );
    // The escaped quote does not end the string, so the cursor after it is still in code.
    let context = at_in(
        "mysql",
        "SELECT * FROM owners o WHERE o.name = 'it\\'s' AND o.|",
    );
    assert_eq!(context.kind, columns_of(&["o"]));
    assert_eq!(
        context.resolve(&["o".to_string()]).cloned(),
        Some(table(None, "owners", Some("o")))
    );
}

#[test]
fn postgresql_e_strings_take_backslash_escapes() {
    let context = at_in(
        "postgresql",
        "SELECT * FROM owners o WHERE o.name = E'it\\'s' AND o.|",
    );
    assert_eq!(context.kind, columns_of(&["o"]));
    // Inside the E-string nothing is completed.
    assert_eq!(
        at_in("postgresql", "SELECT E'it\\'s fr|' FROM t").kind,
        ContextKind::Nothing
    );
}
