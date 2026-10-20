use super::*;
use crate::sql_context::context_at;
use crate::test_support;

fn schema() -> DbSchema {
    serde_json::from_value(test_support::schema()).unwrap()
}

/// The completions at the `|` in `marked`, as `label detail` strings.
fn offered_with(marked: &str, schema: Option<&DbSchema>, driver_id: &str) -> Vec<SqlCompletion> {
    let default_schema = match driver_id {
        "postgresql" => Some("public".to_string()),
        _ => None,
    };
    offered_in(
        marked,
        schema,
        &Dialect {
            driver_id: driver_id.to_string(),
            default_schema,
        },
    )
}

fn offered_in(marked: &str, schema: Option<&DbSchema>, dialect: &Dialect) -> Vec<SqlCompletion> {
    let offset = marked.find('|').expect("a cursor marker");
    let text = marked.replacen('|', "", 1);
    let flavor = crate::statements::SqlFlavor::for_driver(&dialect.driver_id);
    completions_for(&context_at(&text, offset, flavor), schema, dialect)
}

fn offered(marked: &str) -> Vec<String> {
    offered_with(marked, Some(&schema()), "postgresql")
        .into_iter()
        .map(|completion| format!("{} {}", completion.label, completion.detail))
        .collect()
}

fn labels(marked: &str) -> Vec<String> {
    offered_with(marked, Some(&schema()), "postgresql")
        .into_iter()
        .map(|completion| completion.label)
        .collect()
}

#[test]
fn tables_and_views_match_the_prefix_case_insensitively() {
    assert_eq!(
        offered("SELECT * FROM ow|"),
        vec!["owners table · public", "owner_names view · public"]
    );
    assert_eq!(labels("SELECT * FROM OW|"), vec!["owners", "owner_names"]);
    assert_eq!(labels("SELECT * FROM inv|"), vec!["invoices"]);
}

#[test]
fn without_a_prefix_every_table_then_the_schemas() {
    assert_eq!(
        labels("SELECT * FROM |"),
        vec![
            "owners",
            "owner_names",
            "pets",
            "invoices",
            "public",
            "billing"
        ]
    );
    let schemas: Vec<String> = offered("SELECT * FROM pub|");
    assert_eq!(schemas, vec!["public schema"]);
}

#[test]
fn tables_of_the_statement_come_first() {
    assert_eq!(
        labels("SELECT * FROM pets p JOIN |"),
        vec![
            "pets",
            "owners",
            "owner_names",
            "invoices",
            "public",
            "billing"
        ]
    );
}

#[test]
fn after_a_schema_only_its_tables() {
    assert_eq!(
        offered("SELECT * FROM billing.|"),
        vec!["invoices table · billing"]
    );
    assert_eq!(labels("SELECT * FROM PUBLIC.p|"), vec!["pets"]);
    assert!(labels("SELECT * FROM nowhere.|").is_empty());
}

#[test]
fn columns_of_the_statement_then_keywords() {
    let items = offered("SELECT | FROM owners");
    assert_eq!(
        items[..3],
        [
            "id serial".to_string(),
            "email varchar(50)".to_string(),
            "First Name text".to_string()
        ]
    );
    assert_eq!(items[3], "SELECT keyword");
    assert!(items[3..].iter().all(|item| item.ends_with(" keyword")));

    // With two tables, the detail says which one a column belongs to.
    assert_eq!(
        offered("SELECT * FROM owners o JOIN pets p ON p.owner_id = o.id WHERE na|"),
        vec!["name text · pets"]
    );
    assert_eq!(
        labels("SELECT e| FROM owners"),
        vec!["email", "exists", "else", "end", "explain"]
    );
}

#[test]
fn an_alias_offers_the_columns_of_its_table_only() {
    assert_eq!(
        offered("SELECT o.| FROM owners o"),
        vec!["id serial", "email varchar(50)", "First Name text"]
    );
    assert_eq!(labels("SELECT o.e| FROM owners o"), vec!["email"]);
    // A table not in the statement, by name or with its schema.
    assert_eq!(
        labels("SELECT pets.| FROM owners"),
        vec!["id", "owner_id", "name"]
    );
    assert_eq!(
        labels("SELECT billing.invoices.| FROM billing.invoices"),
        vec!["id", "total"]
    );
    assert!(labels("SELECT x.| FROM owners o").is_empty());
}

#[test]
fn keywords_follow_the_case_of_the_prefix() {
    assert_eq!(labels("SEL|"), vec!["SELECT"]);
    assert_eq!(labels("sel|"), vec!["select"]);
    assert_eq!(labels("Sel|"), vec!["select"]);
    let all = labels("|");
    assert_eq!(all.first().map(String::as_str), Some("SELECT"));
    assert!(
        all.iter()
            .all(|keyword| keyword.chars().all(|c| !c.is_lowercase()))
    );
}

#[test]
fn identifiers_are_quoted_only_when_needed() {
    let inserted = |marked: &str, driver: &str| -> Vec<String> {
        offered_with(marked, Some(&schema()), driver)
            .into_iter()
            .map(|completion| completion.insert)
            .collect()
    };
    assert_eq!(
        inserted("SELECT o.| FROM owners o", "postgresql"),
        vec!["id", "email", "\"First Name\""]
    );
    assert_eq!(
        inserted("SELECT o.| FROM owners o", "mysql"),
        vec!["id", "email", "`First Name`"]
    );
    assert_eq!(
        inserted("SELECT * FROM ow|", "postgresql"),
        vec!["owners", "owner_names"]
    );
    assert_eq!(inserted("sel|", "postgresql"), vec!["select"]);
}

#[test]
fn without_a_schema_only_keywords() {
    let without = |marked: &str| -> Vec<String> {
        offered_with(marked, None, "postgresql")
            .into_iter()
            .map(|completion| completion.label)
            .collect()
    };
    assert!(without("SELECT * FROM ow|").is_empty());
    assert!(without("SELECT o.| FROM owners o").is_empty());
    assert_eq!(
        without("SELECT e| FROM owners"),
        vec!["exists", "else", "end", "explain"]
    );
}

#[test]
fn nothing_in_strings_and_comments() {
    assert!(labels("SELECT * FROM owners WHERE email = 'ow|'").is_empty());
    assert!(labels("-- SEL|").is_empty());
}

#[test]
fn tables_outside_the_default_schema_are_inserted_qualified() {
    let inserted = |marked: &str, driver_id: &str, default_schema: Option<&str>| -> Vec<String> {
        let dialect = Dialect {
            driver_id: driver_id.to_string(),
            default_schema: default_schema.map(str::to_string),
        };
        offered_in(marked, Some(&schema()), &dialect)
            .into_iter()
            .map(|completion| completion.insert)
            .collect()
    };
    assert_eq!(
        inserted("SELECT * FROM |", "postgresql", Some("public")),
        [
            "owners",
            "owner_names",
            "pets",
            "billing.invoices",
            "public",
            "billing"
        ]
    );
    assert_eq!(
        inserted("SELECT * FROM |", "postgresql", Some("billing")),
        [
            "public.owners",
            "public.owner_names",
            "public.pets",
            "invoices",
            "public",
            "billing"
        ]
    );
    assert_eq!(
        inserted("SELECT o.| FROM owners o", "postgresql", Some("billing")),
        ["id", "email", "\"First Name\""],
        "columns are never qualified"
    );
    // After `schema.` the schema is already there; without a known default nothing is qualified.
    assert_eq!(
        inserted("SELECT * FROM billing.|", "postgresql", Some("public")),
        ["invoices"]
    );
    assert_eq!(inserted("SELECT * FROM inv|", "sqlite", None), ["invoices"]);
    // MySQL and MariaDB without a database in the URL have no default one: every table is
    // inserted with its database, else the query fails with "No database selected".
    for driver_id in ["mysql", "mariadb"] {
        assert_eq!(
            inserted("SELECT * FROM |", driver_id, None),
            [
                "public.owners",
                "public.owner_names",
                "public.pets",
                "billing.invoices",
                "public",
                "billing"
            ],
            "{driver_id}"
        );
        assert_eq!(
            inserted("SELECT * FROM billing.|", driver_id, None),
            ["invoices"]
        );
    }
}

#[test]
fn the_default_schema_comes_from_the_connection() {
    let dialect = |driver: &str, url: &str| {
        let connection: ConnectionInfo = serde_json::from_value(serde_json::json!({
            "id": "c1", "name": "db", "driverId": driver, "url": url,
            "user": null, "hasPassword": false, "options": {}
        }))
        .unwrap();
        Dialect::for_connection(&connection).default_schema
    };
    assert_eq!(
        dialect("postgresql", "jdbc:postgresql://localhost:5432/shop"),
        Some("public".to_string())
    );
    assert_eq!(
        dialect(
            "postgresql",
            "jdbc:postgresql://localhost/shop?ssl=true&currentSchema=billing,public"
        ),
        Some("billing".to_string())
    );
    assert_eq!(
        dialect(
            "mariadb",
            "jdbc:mariadb://localhost:3306/petclinic?useSSL=false"
        ),
        Some("petclinic".to_string())
    );
    assert_eq!(dialect("mysql", "jdbc:mysql://localhost:3306/"), None);
    assert_eq!(dialect("sqlite", "jdbc:sqlite:/tmp/app.db"), None);
}

#[test]
fn subqueries_and_derived_tables() {
    let items = labels("SELECT * FROM pets WHERE owner_id IN (SELECT | FROM owners)");
    assert_eq!(items[..4], ["id", "email", "First Name", "id"]);
    assert!(labels("SELECT d.| FROM (SELECT id FROM owners) d").is_empty());
    assert_eq!(
        labels("SELECT * FROM owners o WHERE id IN (SELECT owner_id FROM pets o WHERE o.n|)"),
        ["name"]
    );
}
