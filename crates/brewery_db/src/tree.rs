//! The Database panel's tree as plain data: data sources, the schema nodes listed so far by
//! `db/introspect`, and which of them are expanded. The panel renders `DbTree::rows` and asks the
//! daemon for the children whenever `expand` or `refresh` says so.

use brewery_daemon_client::{ConnectionInfo, DbNode, DbNodeKind};
use std::collections::{HashMap, HashSet};

/// What `db/introspect` takes to list children; empty for the top of a data source.
pub type NodePath = Vec<String>;

/// A data source (empty `path`) or one of its schema nodes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RowKey {
    pub connection_id: String,
    pub path: NodePath,
}

impl RowKey {
    pub fn new(connection_id: impl Into<String>, path: NodePath) -> Self {
        Self {
            connection_id: connection_id.into(),
            path,
        }
    }

    pub fn data_source(connection_id: impl Into<String>) -> Self {
        Self::new(connection_id, Vec::new())
    }

    pub fn is_data_source(&self) -> bool {
        self.path.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    DataSource {
        connection: ConnectionInfo,
        expanded: bool,
        /// Listing the top of the data source failed.
        error: Option<String>,
    },
    Node {
        connection_id: String,
        node: DbNode,
        depth: usize,
        expanded: bool,
    },
    Loading {
        parent: RowKey,
        depth: usize,
    },
    Empty {
        parent: RowKey,
        depth: usize,
    },
    Error {
        parent: RowKey,
        depth: usize,
        message: String,
    },
}

impl Row {
    /// Rows that stand for something the user can select and act on.
    pub fn key(&self) -> Option<RowKey> {
        match self {
            Row::DataSource { connection, .. } => Some(RowKey::data_source(connection.id.clone())),
            Row::Node {
                connection_id,
                node,
                ..
            } => Some(RowKey::new(connection_id.clone(), node.path.clone())),
            Row::Loading { .. } | Row::Empty { .. } | Row::Error { .. } => None,
        }
    }
}

#[derive(Debug, Clone)]
enum Children {
    Loading,
    Loaded(Vec<DbNode>),
    Failed(String),
}

/// Invariant: every expanded key has a `children` entry, so an open node never waits for a
/// listing nobody requested.
#[derive(Debug, Default)]
pub struct DbTree {
    sources: Vec<ConnectionInfo>,
    expanded: HashSet<RowKey>,
    children: HashMap<RowKey, Children>,
}

/// How a node reads in the tree: the table's folders get the short names IntelliJ uses.
pub fn node_label(node: &DbNode) -> &str {
    match (node.kind, node.name.as_str()) {
        (DbNodeKind::Folder, "foreignKeys") => "keys",
        _ => &node.name,
    }
}

impl DbTree {
    pub fn sources(&self) -> &[ConnectionInfo] {
        &self.sources
    }

    pub fn source(&self, connection_id: &str) -> Option<&ConnectionInfo> {
        self.sources
            .iter()
            .find(|source| source.id == connection_id)
    }

    /// Keeps what is known about the data sources that are still there.
    pub fn set_sources(&mut self, sources: Vec<ConnectionInfo>) {
        let ids: HashSet<&str> = sources.iter().map(|source| source.id.as_str()).collect();
        self.expanded
            .retain(|key| ids.contains(key.connection_id.as_str()));
        self.children
            .retain(|key, _| ids.contains(key.connection_id.as_str()));
        self.sources = sources;
    }

    pub fn is_expanded(&self, key: &RowKey) -> bool {
        self.expanded.contains(key)
    }

    /// True when the children have to be fetched: never listed, or the last listing failed.
    pub fn expand(&mut self, key: RowKey) -> bool {
        let fetch = matches!(self.children.get(&key), None | Some(Children::Failed(_)));
        if fetch {
            self.children.insert(key.clone(), Children::Loading);
        }
        self.expanded.insert(key);
        fetch
    }

    /// The children stay cached, and so does the expansion below this node.
    pub fn collapse(&mut self, key: &RowKey) {
        self.expanded.remove(key);
    }

    /// True when expanding needs a fetch.
    pub fn toggle(&mut self, key: RowKey) -> bool {
        if self.is_expanded(&key) {
            self.collapse(&key);
            false
        } else {
            self.expand(key)
        }
    }

    /// Only a listing that was asked for and is still awaited is taken.
    pub fn set_children(&mut self, key: &RowKey, result: Result<Vec<DbNode>, String>) {
        if let Some(children) = self.children.get_mut(key)
            && matches!(children, Children::Loading)
        {
            *children = match result {
                Ok(nodes) => Children::Loaded(nodes),
                Err(message) => Children::Failed(message),
            };
        }
    }

    pub fn root_error(&self, connection_id: &str) -> Option<String> {
        match self.children.get(&RowKey::data_source(connection_id)) {
            Some(Children::Failed(message)) => Some(message.clone()),
            _ => None,
        }
    }

    /// Drops the cache of one data source. What is open and visible stays open and is returned
    /// to be fetched again, shortest paths first; nodes hidden under a collapsed parent close.
    pub fn refresh(&mut self, connection_id: &str) -> Vec<NodePath> {
        let mut refetch: Vec<NodePath> = self
            .reset(|key| key.connection_id == connection_id)
            .into_iter()
            .map(|key| key.path)
            .collect();
        refetch.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
        refetch
    }

    /// `refresh` for every data source, after the daemon restarted.
    pub fn refresh_all(&mut self) -> Vec<RowKey> {
        let mut refetch = self.reset(|_| true);
        refetch.sort_by(|a, b| a.path.len().cmp(&b.path.len()).then_with(|| a.cmp(b)));
        refetch
    }

    fn reset(&mut self, affected: impl Fn(&RowKey) -> bool) -> Vec<RowKey> {
        let visible: Vec<RowKey> = self
            .expanded
            .iter()
            .filter(|key| affected(key) && self.ancestors_expanded(key))
            .cloned()
            .collect();
        self.expanded
            .retain(|key| !affected(key) || visible.contains(key));
        self.children.retain(|key, _| !affected(key));
        for key in &visible {
            self.children.insert(key.clone(), Children::Loading);
        }
        visible
    }

    fn ancestors_expanded(&self, key: &RowKey) -> bool {
        (0..key.path.len()).all(|len| {
            self.expanded.contains(&RowKey::new(
                key.connection_id.clone(),
                key.path[..len].to_vec(),
            ))
        })
    }

    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for source in &self.sources {
            let key = RowKey::data_source(source.id.clone());
            let expanded = self.is_expanded(&key);
            rows.push(Row::DataSource {
                connection: source.clone(),
                expanded,
                error: self.root_error(&source.id),
            });
            if expanded {
                self.push_children(&key, 1, &mut rows);
            }
        }
        rows
    }

    fn push_children(&self, parent: &RowKey, depth: usize, rows: &mut Vec<Row>) {
        match self.children.get(parent) {
            None | Some(Children::Loading) => rows.push(Row::Loading {
                parent: parent.clone(),
                depth,
            }),
            Some(Children::Failed(message)) => rows.push(Row::Error {
                parent: parent.clone(),
                depth,
                message: message.clone(),
            }),
            Some(Children::Loaded(nodes)) if nodes.is_empty() => rows.push(Row::Empty {
                parent: parent.clone(),
                depth,
            }),
            Some(Children::Loaded(nodes)) => {
                for node in nodes {
                    let key = RowKey::new(parent.connection_id.clone(), node.path.clone());
                    let expanded = node.has_children && self.is_expanded(&key);
                    rows.push(Row::Node {
                        connection_id: parent.connection_id.clone(),
                        node: node.clone(),
                        depth,
                        expanded,
                    });
                    if expanded {
                        self.push_children(&key, depth + 1, rows);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "tree_tests.rs"]
mod tests;
