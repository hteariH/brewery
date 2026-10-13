//! The schema SQL completion knows for one data source: fetched with `db/schema` on the first
//! completion request, shared by every console of that data source, refreshed by the Database
//! tool window's Refresh, by saving the data source and by DDL run in a console.

use brewery_daemon_client::{DbSchema, ProjectDaemon, SchemaParams, daemon_error_message};
use gpui::{
    App, AppContext as _, Context, Entity, EntityId, Global, SharedString, Task, WeakEntity,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

type Key = (EntityId, String);

/// Every cache by daemon and connection. Weak: a cache lives as long as a console holds it.
#[derive(Default)]
struct SchemaCaches {
    caches: HashMap<Key, WeakEntity<SchemaCache>>,
    /// Refreshed while no cache existed: the first fetch rebuilds the daemon's snapshot too.
    stale: HashSet<Key>,
}

impl Global for SchemaCaches {}

pub struct SchemaCache {
    daemon: Entity<ProjectDaemon>,
    connection_id: String,
    /// The last snapshot that arrived; still served while a refresh is under way.
    schema: Option<Arc<DbSchema>>,
    fetch: Option<Task<()>>,
    /// Why the last fetch failed. No new fetch starts until `refresh` or `retry_after_failure`.
    error: Option<SharedString>,
    /// The next fetch asks the daemon to rebuild its snapshot.
    refresh_next: bool,
}

impl SchemaCache {
    /// The cache of `connection_id`, shared with the other consoles of that data source.
    pub fn for_connection(
        daemon: &Entity<ProjectDaemon>,
        connection_id: &str,
        cx: &mut App,
    ) -> Entity<Self> {
        let key = (daemon.entity_id(), connection_id.to_string());
        let caches = cx.default_global::<SchemaCaches>();
        if let Some(cache) = caches.caches.get(&key).and_then(WeakEntity::upgrade) {
            return cache;
        }
        let refresh_next = caches.stale.remove(&key);
        let daemon = daemon.clone();
        let connection_id = connection_id.to_string();
        let cache = cx.new(|_| Self {
            daemon,
            connection_id,
            schema: None,
            fetch: None,
            error: None,
            refresh_next,
        });
        let caches = cx.default_global::<SchemaCaches>();
        caches.caches.retain(|_, cache| cache.upgrade().is_some());
        caches.caches.insert(key, cache.downgrade());
        cache
    }

    /// The schema of `connection_id` changed or may have: its cache, if there is one, fetches
    /// again; otherwise the first fetch will rebuild the daemon's snapshot.
    pub fn refresh_connection(daemon: &Entity<ProjectDaemon>, connection_id: &str, cx: &mut App) {
        let key = (daemon.entity_id(), connection_id.to_string());
        let cache = cx
            .default_global::<SchemaCaches>()
            .caches
            .get(&key)
            .and_then(WeakEntity::upgrade);
        match cache {
            Some(cache) => cache.update(cx, |cache, cx| cache.refresh(cx)),
            None => {
                cx.default_global::<SchemaCaches>().stale.insert(key);
            }
        }
    }

    /// The schema, if it arrived. The first call starts the fetch and returns `None`; the cache
    /// notifies its observers once the schema is there.
    pub fn schema(&mut self, cx: &mut Context<Self>) -> Option<Arc<DbSchema>> {
        if self.schema.is_none() && self.fetch.is_none() && self.error.is_none() {
            self.start_fetch(cx);
        }
        self.schema.clone()
    }

    pub fn is_loading(&self) -> bool {
        self.fetch.is_some()
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// Fetches again, asking the daemon to rebuild its snapshot. The old schema is served until
    /// the new one arrives. Before the first fetch nothing is sent; that fetch rebuilds instead.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.refresh_next = true;
        self.error = None;
        if self.schema.is_some() || self.fetch.is_some() {
            self.start_fetch(cx);
        }
    }

    /// A statement ran fine, so the database is reachable again: after a failed fetch, the next
    /// request tries again.
    pub fn retry_after_failure(&mut self) {
        self.error = None;
    }

    fn start_fetch(&mut self, cx: &mut Context<Self>) {
        // A daemon that is still starting cannot answer; the next request tries again.
        if !self.daemon.read(cx).is_ready() {
            return;
        }
        let refresh = std::mem::take(&mut self.refresh_next);
        let request = self.daemon.read(cx).db_schema(
            SchemaParams {
                connection_id: self.connection_id.clone(),
                refresh: refresh.then_some(true),
            },
            cx,
        );
        // Replacing the task drops a fetch that was under way; its answer would be older.
        self.fetch = Some(cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                this.fetch = None;
                match result {
                    Ok(schema) => {
                        this.schema = Some(Arc::new(schema));
                        this.error = None;
                    }
                    Err(error) => {
                        this.error = Some(daemon_error_message(&error).into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }
}

#[cfg(test)]
#[path = "schema_cache_tests.rs"]
mod tests;
