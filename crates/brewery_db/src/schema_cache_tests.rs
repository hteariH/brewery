use super::*;
use crate::test_support::*;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::json;

async fn cache_for<'a>(
    connection_id: &str,
    cx: &'a mut TestAppContext,
) -> (Fixture, Entity<SchemaCache>, &'a mut VisualTestContext) {
    let (fixture, cx) = shown_panel(cx).await;
    let daemon = fixture
        .panel
        .read_with(cx, |panel, _| panel.daemon().cloned())
        .expect("the panel holds the daemon");
    let cache = cx.update(|_, cx| SchemaCache::for_connection(&daemon, connection_id, cx));
    (fixture, cache, cx)
}

fn table_names(schema: &DbSchema) -> Vec<String> {
    schema
        .schemas
        .iter()
        .flat_map(|schema| schema.tables.iter().map(|table| table.name.clone()))
        .collect()
}

#[gpui::test]
async fn the_schema_is_fetched_once_on_first_use(cx: &mut TestAppContext) {
    let (fixture, cache, cx) = cache_for("c1", cx).await;
    cx.run_until_parked();
    assert!(
        fixture.calls_of("db/schema").is_empty(),
        "nothing before use"
    );

    let first = cache.update(cx, |cache, cx| cache.schema(cx));
    assert!(first.is_none(), "the first request starts the fetch");
    assert!(cache.read_with(cx, |cache, _| cache.is_loading()));
    cx.run_until_parked();
    assert_eq!(
        fixture.calls_of("db/schema"),
        vec![json!({"connectionId": "c1"})]
    );

    let schema = cache
        .update(cx, |cache, cx| cache.schema(cx))
        .expect("loaded");
    assert_eq!(
        table_names(&schema),
        ["owners", "owner_names", "pets", "invoices"]
    );
    cache.update(cx, |cache, cx| cache.schema(cx));
    cx.run_until_parked();
    assert_eq!(
        fixture.calls_of("db/schema").len(),
        1,
        "served from the cache"
    );
}

#[gpui::test]
async fn concurrent_requests_share_one_fetch(cx: &mut TestAppContext) {
    let (fixture, cache, cx) = cache_for("c1", cx).await;
    fixture.state.hold("db/schema");
    cache.update(cx, |cache, cx| cache.schema(cx));
    cache.update(cx, |cache, cx| cache.schema(cx));
    // A second console of the same data source gets the same cache.
    let daemon = fixture
        .panel
        .read_with(cx, |panel, _| panel.daemon().cloned())
        .unwrap();
    let same = cx.update(|_, cx| SchemaCache::for_connection(&daemon, "c1", cx));
    assert_eq!(same.entity_id(), cache.entity_id());
    same.update(cx, |cache, cx| cache.schema(cx));
    cx.run_until_parked();
    assert_eq!(fixture.state.held_count("db/schema"), 1);

    fixture.state.release("db/schema");
    cx.run_until_parked();
    assert!(same.update(cx, |cache, cx| cache.schema(cx)).is_some());
    assert_eq!(fixture.calls_of("db/schema").len(), 1);
}

#[gpui::test]
async fn refresh_rebuilds_and_serves_the_old_schema_meanwhile(cx: &mut TestAppContext) {
    let (fixture, cache, cx) = cache_for("c1", cx).await;
    cache.update(cx, |cache, cx| cache.schema(cx));
    cx.run_until_parked();

    let mut changed = schema();
    changed["schemas"][1]["tables"][0]["name"] = json!("receipts");
    *fixture.state.schema_reply.borrow_mut() = Some(changed);
    fixture.state.hold("db/schema");
    cache.update(cx, |cache, cx| cache.refresh(cx));
    cx.run_until_parked();
    assert_eq!(
        fixture.calls_of("db/schema").last(),
        Some(&json!({"connectionId": "c1", "refresh": true}))
    );
    let meanwhile = cache.update(cx, |cache, cx| cache.schema(cx)).unwrap();
    assert!(table_names(&meanwhile).contains(&"invoices".to_string()));

    fixture.state.release("db/schema");
    cx.run_until_parked();
    let fresh = cache.update(cx, |cache, cx| cache.schema(cx)).unwrap();
    assert!(table_names(&fresh).contains(&"receipts".to_string()));
    assert_eq!(fixture.calls_of("db/schema").len(), 2);
}

#[gpui::test]
async fn a_refresh_before_any_fetch_waits_for_first_use(cx: &mut TestAppContext) {
    let (fixture, cache, cx) = cache_for("c1", cx).await;
    cache.update(cx, |cache, cx| cache.refresh(cx));
    cx.run_until_parked();
    assert!(fixture.calls_of("db/schema").is_empty());
    cache.update(cx, |cache, cx| cache.schema(cx));
    cx.run_until_parked();
    assert_eq!(
        fixture.calls_of("db/schema"),
        vec![json!({"connectionId": "c1", "refresh": true})]
    );

    // A data source refreshed while no console holds its cache rebuilds on first use as well.
    let daemon = fixture
        .panel
        .read_with(cx, |panel, _| panel.daemon().cloned())
        .unwrap();
    cx.update(|_, cx| SchemaCache::refresh_connection(&daemon, "c9", cx));
    let other = cx.update(|_, cx| SchemaCache::for_connection(&daemon, "c9", cx));
    other.update(cx, |cache, cx| cache.schema(cx));
    cx.run_until_parked();
    assert_eq!(
        fixture.calls_of("db/schema").last(),
        Some(&json!({"connectionId": "c9", "refresh": true}))
    );
}

#[gpui::test]
async fn a_failed_fetch_leaves_keywords_only_until_retried(cx: &mut TestAppContext) {
    let (fixture, cache, cx) = cache_for("c1", cx).await;
    fixture.state.fail("db/schema", "connection refused");
    cache.update(cx, |cache, cx| cache.schema(cx));
    cx.run_until_parked();
    cache.read_with(cx, |cache, _| {
        assert!(!cache.is_loading());
        assert_eq!(
            cache.error().map(|error| error.to_string()),
            Some("connection refused".to_string())
        );
    });
    // Every keystroke asks again; the daemon is not asked again.
    for _ in 0..3 {
        assert!(cache.update(cx, |cache, cx| cache.schema(cx)).is_none());
    }
    cx.run_until_parked();
    assert_eq!(fixture.calls_of("db/schema").len(), 1);

    // Once a statement ran fine, the database is reachable: the next request tries again.
    fixture.state.succeed("db/schema");
    cache.update(cx, |cache, _| cache.retry_after_failure());
    cache.update(cx, |cache, cx| cache.schema(cx));
    cx.run_until_parked();
    assert!(cache.update(cx, |cache, cx| cache.schema(cx)).is_some());
    assert_eq!(cache.read_with(cx, |cache, _| cache.error().cloned()), None);
    assert_eq!(fixture.calls_of("db/schema").len(), 2);
}

#[gpui::test]
async fn a_restarted_daemon_retries_a_failed_fetch(cx: &mut TestAppContext) {
    let (fixture, cache, cx) = cache_for("c1", cx).await;
    fixture.state.fail("db/schema", "the daemon stopped");
    cache.update(cx, |cache, cx| cache.schema(cx));
    cx.run_until_parked();
    assert!(cache.read_with(cx, |cache, _| cache.error().is_some()));

    fixture.state.succeed("db/schema");
    let daemon = fixture
        .panel
        .read_with(cx, |panel, _| panel.daemon().cloned())
        .unwrap();
    daemon.update(cx, |daemon, cx| daemon.reload(cx));
    cx.run_until_parked();
    assert_eq!(cache.read_with(cx, |cache, _| cache.error().cloned()), None);
    cache.update(cx, |cache, cx| cache.schema(cx));
    cx.run_until_parked();
    assert!(cache.update(cx, |cache, cx| cache.schema(cx)).is_some());
    assert_eq!(fixture.calls_of("db/schema").len(), 2);
}
