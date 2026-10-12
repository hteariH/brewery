use super::*;
use brewery_daemon_client::DbNodeKind;
use std::collections::BTreeMap;

fn source(id: &str, name: &str) -> ConnectionInfo {
    ConnectionInfo {
        id: id.to_string(),
        name: name.to_string(),
        driver_id: "postgresql".to_string(),
        url: "jdbc:postgresql://localhost:5432/app".to_string(),
        user: None,
        has_password: false,
        options: BTreeMap::new(),
        warning: None,
    }
}

fn node(kind: DbNodeKind, path: &[&str], has_children: bool) -> DbNode {
    DbNode {
        name: path.last().copied().unwrap_or_default().to_string(),
        kind,
        path: path.iter().map(|segment| segment.to_string()).collect(),
        has_children,
        detail: None,
    }
}

fn key(id: &str, path: &[&str]) -> RowKey {
    RowKey::new(id, path.iter().map(|segment| segment.to_string()).collect())
}

/// One short line per row so that a whole tree reads as one assertion.
fn outline(tree: &DbTree) -> Vec<String> {
    tree.rows()
        .iter()
        .map(|row| match row {
            Row::DataSource {
                connection,
                expanded,
                error,
            } => format!(
                "{}{}{}",
                if *expanded { "v " } else { "> " },
                connection.name,
                if error.is_some() { " !" } else { "" }
            ),
            Row::Node {
                node,
                depth,
                expanded,
                ..
            } => format!(
                "{}{}{}",
                "  ".repeat(*depth),
                if !node.has_children {
                    "- "
                } else if *expanded {
                    "v "
                } else {
                    "> "
                },
                node_label(node)
            ),
            Row::Loading { depth, .. } => format!("{}loading", "  ".repeat(*depth)),
            Row::Empty { depth, .. } => format!("{}empty", "  ".repeat(*depth)),
            Row::Error { depth, message, .. } => {
                format!("{}error: {message}", "  ".repeat(*depth))
            }
        })
        .collect()
}

#[test]
fn data_sources_start_collapsed() {
    let mut tree = DbTree::default();
    tree.set_sources(vec![source("a", "Alpha"), source("b", "Beta")]);
    assert_eq!(outline(&tree), ["> Alpha", "> Beta"]);
    assert_eq!(
        tree.rows()[1].key(),
        Some(key("b", &[])),
        "a data source row is keyed by its id and the empty path"
    );
}

#[test]
fn expanding_loads_lazily_and_caches() {
    let mut tree = DbTree::default();
    tree.set_sources(vec![source("a", "Alpha")]);

    assert!(tree.expand(key("a", &[])), "first expansion must fetch");
    assert_eq!(outline(&tree), ["v Alpha", "  loading"]);

    tree.set_children(
        &key("a", &[]),
        Ok(vec![node(DbNodeKind::Schema, &["public"], true)]),
    );
    assert_eq!(outline(&tree), ["v Alpha", "  > public"]);

    assert!(tree.expand(key("a", &["public"])));
    tree.set_children(
        &key("a", &["public"]),
        Ok(vec![
            node(DbNodeKind::Table, &["public", "owners"], true),
            node(DbNodeKind::Sequence, &["public", "owners_id_seq"], false),
        ]),
    );
    assert_eq!(
        outline(&tree),
        [
            "v Alpha",
            "  v public",
            "    > owners",
            "    - owners_id_seq"
        ]
    );

    tree.collapse(&key("a", &[]));
    assert_eq!(outline(&tree), ["> Alpha"]);
    assert!(
        !tree.expand(key("a", &[])),
        "expanding again uses the cache"
    );
    assert_eq!(
        outline(&tree),
        [
            "v Alpha",
            "  v public",
            "    > owners",
            "    - owners_id_seq"
        ],
        "collapsing a parent keeps the children's expansion"
    );
}

#[test]
fn toggle_flips_expansion() {
    let mut tree = DbTree::default();
    tree.set_sources(vec![source("a", "Alpha")]);
    assert!(tree.toggle(key("a", &[])));
    assert!(tree.is_expanded(&key("a", &[])));
    assert!(!tree.toggle(key("a", &[])));
    assert!(!tree.is_expanded(&key("a", &[])));
}

#[test]
fn folders_have_short_labels_and_columns_their_detail() {
    let folder = |name: &str| node(DbNodeKind::Folder, &["public", "owners", name], true);
    assert_eq!(node_label(&folder("columns")), "columns");
    assert_eq!(node_label(&folder("indexes")), "indexes");
    assert_eq!(node_label(&folder("foreignKeys")), "keys");
    assert_eq!(
        node_label(&node(DbNodeKind::Table, &["public", "foreignKeys"], true)),
        "foreignKeys",
        "only folders are renamed"
    );
}

#[test]
fn failures_show_an_error_row_and_a_badge_and_retry_on_expand() {
    let mut tree = DbTree::default();
    tree.set_sources(vec![source("a", "Alpha")]);
    tree.expand(key("a", &[]));
    tree.set_children(&key("a", &[]), Err("connection refused".to_string()));
    assert_eq!(outline(&tree), ["v Alpha !", "  error: connection refused"]);
    assert_eq!(tree.root_error("a").as_deref(), Some("connection refused"));

    tree.collapse(&key("a", &[]));
    assert!(
        tree.expand(key("a", &[])),
        "a failed listing is fetched again"
    );
    assert_eq!(outline(&tree), ["v Alpha", "  loading"]);
}

#[test]
fn nested_failures_and_empty_listings() {
    let mut tree = DbTree::default();
    tree.set_sources(vec![source("a", "Alpha")]);
    tree.expand(key("a", &[]));
    tree.set_children(
        &key("a", &[]),
        Ok(vec![
            node(DbNodeKind::Schema, &["broken"], true),
            node(DbNodeKind::Schema, &["empty"], true),
        ]),
    );
    tree.expand(key("a", &["broken"]));
    tree.set_children(&key("a", &["broken"]), Err("denied".to_string()));
    tree.expand(key("a", &["empty"]));
    tree.set_children(&key("a", &["empty"]), Ok(Vec::new()));
    assert_eq!(
        outline(&tree),
        [
            "v Alpha",
            "  v broken",
            "    error: denied",
            "  v empty",
            "    empty"
        ]
    );
    assert_eq!(
        tree.root_error("a"),
        None,
        "only the root failure is a badge"
    );
}

#[test]
fn late_replies_after_a_collapse_still_fill_the_cache_but_unasked_ones_do_not() {
    let mut tree = DbTree::default();
    tree.set_sources(vec![source("a", "Alpha")]);
    tree.set_children(
        &key("a", &[]),
        Ok(vec![node(DbNodeKind::Schema, &["public"], true)]),
    );
    assert!(
        tree.expand(key("a", &[])),
        "a reply nobody asked for is ignored"
    );
    tree.collapse(&key("a", &[]));
    tree.set_children(
        &key("a", &[]),
        Ok(vec![node(DbNodeKind::Schema, &["public"], true)]),
    );
    assert!(!tree.expand(key("a", &[])));
    assert_eq!(outline(&tree), ["v Alpha", "  > public"]);
}

#[test]
fn refresh_clears_one_data_source_and_refetches_what_was_open() {
    let mut tree = DbTree::default();
    tree.set_sources(vec![source("a", "Alpha"), source("b", "Beta")]);
    for id in ["a", "b"] {
        tree.expand(key(id, &[]));
        tree.set_children(
            &key(id, &[]),
            Ok(vec![node(DbNodeKind::Schema, &["public"], true)]),
        );
        tree.expand(key(id, &["public"]));
        tree.set_children(&key(id, &["public"]), Ok(Vec::new()));
    }

    let refetch = tree.refresh("a");
    assert_eq!(
        refetch,
        vec![Vec::<String>::new(), vec!["public".to_string()]],
        "shortest paths first"
    );
    assert_eq!(
        outline(&tree),
        ["v Alpha", "  loading", "v Beta", "  v public", "    empty"]
    );

    tree.collapse(&key("b", &[]));
    tree.collapse(&key("b", &["public"]));
    assert!(
        tree.refresh("b").is_empty(),
        "nothing open, nothing to fetch"
    );
    assert!(tree.expand(key("b", &[])), "the cache is gone");
}

#[test]
fn replacing_the_sources_forgets_removed_ones() {
    let mut tree = DbTree::default();
    tree.set_sources(vec![source("a", "Alpha"), source("b", "Beta")]);
    tree.expand(key("a", &[]));
    tree.set_children(&key("a", &[]), Ok(Vec::new()));
    tree.expand(key("b", &[]));
    tree.set_children(&key("b", &[]), Ok(Vec::new()));

    tree.set_sources(vec![source("b", "Beta (renamed)")]);
    assert_eq!(outline(&tree), ["v Beta (renamed)", "  empty"]);
    tree.set_sources(vec![source("a", "Alpha"), source("b", "Beta")]);
    assert_eq!(
        outline(&tree),
        ["> Alpha", "v Beta", "  empty"],
        "a removed data source comes back collapsed"
    );
    assert_eq!(tree.source("b").map(|s| s.name.as_str()), Some("Beta"));
}

#[test]
fn clear_cache_keeps_expansion_and_refetches_everything_open() {
    let mut tree = DbTree::default();
    tree.set_sources(vec![source("a", "Alpha"), source("b", "Beta")]);
    tree.expand(key("a", &[]));
    tree.set_children(&key("a", &[]), Ok(Vec::new()));
    tree.expand(key("b", &[]));
    tree.set_children(&key("b", &[]), Ok(Vec::new()));

    let mut refetch = tree.refresh_all();
    refetch.sort();
    assert_eq!(refetch, vec![key("a", &[]), key("b", &[])]);
    assert_eq!(
        outline(&tree),
        ["v Alpha", "  loading", "v Beta", "  loading"]
    );
}
