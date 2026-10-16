//! SQL completion in the query console: what `sql_context` says the cursor wants, filled in from
//! the data source's `SchemaCache`, as an `editor::CompletionProvider`.

use crate::schema_cache::SchemaCache;
use crate::sql_context::{ContextKind, KEYWORDS, SqlContext, TableRef, context_at};
use crate::statements::quote_identifier;
use anyhow::Result;
use brewery_daemon_client::{ConnectionInfo, DbSchema, TableInfo, TableKind};
use editor::{CompletionProvider, Editor};
use gpui::{Context, Entity, Task, Window};
use language::{Buffer, CodeLabel, ToOffset as _};
use project::lsp_store::CompletionDocumentation;
use project::{Completion, CompletionDisplayOptions, CompletionResponse, CompletionSource};
use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;

/// One item of the menu: what it shows, what it inserts and its detail (a column's type, "table",
/// "view", "schema" or "keyword").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlCompletion {
    pub label: String,
    pub insert: String,
    pub detail: String,
}

/// How names are written for one data source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dialect {
    pub driver_id: String,
    /// The schema unqualified names resolve to; tables of other schemas are inserted as
    /// `schema.table`. `None` never qualifies (SQLite, or nothing known), except for MySQL and
    /// MariaDB, where it means no database is selected and every table is qualified.
    pub default_schema: Option<String>,
}

impl Dialect {
    /// PostgreSQL: `currentSchema` of the URL (or the options), else `public`. MySQL and MariaDB:
    /// the database of the URL. Anything else: unknown.
    pub fn for_connection(connection: &ConnectionInfo) -> Self {
        let url = connection.url.as_str();
        let query_param = |name: &str| -> Option<String> {
            let (_, query) = url.split_once('?')?;
            query.split(['&', ';']).find_map(|pair| {
                let (key, value) = pair.split_once('=')?;
                key.eq_ignore_ascii_case(name).then(|| value.to_string())
            })
        };
        let default_schema = match connection.driver_id.as_str() {
            "postgresql" => {
                let current = query_param("currentSchema")
                    .or_else(|| connection.options.get("currentSchema").cloned());
                Some(
                    current
                        .and_then(|value| {
                            value
                                .split(',')
                                .next()
                                .map(|first| first.trim().to_string())
                        })
                        .filter(|first| !first.is_empty())
                        .unwrap_or_else(|| "public".to_string()),
                )
            }
            "mysql" | "mariadb" => url
                .split_once("//")
                .and_then(|(_, rest)| rest.split_once('/'))
                .map(|(_, path)| {
                    path.split(['?', ';', '/'])
                        .next()
                        .unwrap_or_default()
                        .to_string()
                })
                .filter(|database| !database.is_empty()),
            _ => None,
        };
        Self {
            driver_id: connection.driver_id.clone(),
            default_schema,
        }
    }

    /// Whether a table of `schema` can be written unqualified. MySQL and MariaDB connected
    /// without a database have none selected, so every table needs its database.
    fn is_default(&self, schema: &str) -> bool {
        if schema.is_empty() {
            return true;
        }
        match self.default_schema.as_deref() {
            Some(default) => default.eq_ignore_ascii_case(schema),
            None => !matches!(self.driver_id.as_str(), "mysql" | "mariadb"),
        }
    }
}

/// Case-insensitive (ASCII) prefix match, without allocating.
fn has_prefix(name: &str, prefix: &str) -> bool {
    name.get(..prefix.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
}

/// The table `name` (in `schema`, or in any schema when `None`, the default one first), with the
/// name of its schema.
fn find_table<'a>(
    db: &'a DbSchema,
    dialect: &Dialect,
    schema: Option<&str>,
    name: &str,
) -> Option<(&'a str, &'a TableInfo)> {
    let in_schema = |wanted: Option<&str>| {
        db.schemas
            .iter()
            .filter(move |candidate| {
                wanted.is_none_or(|wanted| candidate.name.eq_ignore_ascii_case(wanted))
            })
            .find_map(|candidate| {
                candidate
                    .tables
                    .iter()
                    .find(|table| table.name.eq_ignore_ascii_case(name))
                    .map(|table| (candidate.name.as_str(), table))
            })
    };
    match schema {
        Some(schema) => in_schema(Some(schema)),
        None => dialect
            .default_schema
            .as_deref()
            .and_then(|default| in_schema(Some(default)))
            .or_else(|| in_schema(None)),
    }
}

fn find_ref<'a>(
    db: &'a DbSchema,
    dialect: &Dialect,
    table: &TableRef,
) -> Option<(&'a str, &'a TableInfo)> {
    if table.name.is_empty() {
        return None;
    }
    find_table(db, dialect, table.schema.as_deref(), &table.name)
}

/// The menu for `context`, in the order it is shown: tables of the statement before the other
/// tables, columns before keywords. Without a schema (not fetched yet, or failed) only keywords.
pub fn completions_for(
    context: &SqlContext,
    schema: Option<&DbSchema>,
    dialect: &Dialect,
) -> Vec<SqlCompletion> {
    let driver_id = dialect.driver_id.as_str();
    let prefix = context.prefix.as_str();
    let mut items = Vec::new();
    let identifier = |name: &str, detail: String| SqlCompletion {
        label: name.to_string(),
        insert: quote_identifier(driver_id, name),
        detail,
    };
    let visible: Vec<&TableRef> = context.visible_tables();
    match &context.kind {
        ContextKind::Nothing => {}
        ContextKind::Keywords => push_keywords(&mut items, prefix),
        ContextKind::Tables { schema: only } => {
            let Some(db) = schema else {
                return items;
            };
            let several_schemas = db
                .schemas
                .iter()
                .filter(|schema| !schema.name.is_empty())
                .count()
                > 1;
            let detail = |schema: &str, table: &TableInfo| {
                let kind = match table.kind {
                    TableKind::View => "view",
                    TableKind::Table | TableKind::Other => "table",
                };
                if several_schemas {
                    format!("{kind} · {schema}")
                } else {
                    kind.to_string()
                }
            };
            let in_scope = |schema: &str| {
                only.as_deref()
                    .is_none_or(|only| schema.eq_ignore_ascii_case(only))
            };
            let mut listed: HashSet<*const TableInfo> = HashSet::default();
            let statement_tables = visible
                .iter()
                .filter_map(|table| find_ref(db, dialect, table));
            let all_tables = db.schemas.iter().flat_map(|schema| {
                schema
                    .tables
                    .iter()
                    .map(|table| (schema.name.as_str(), table))
            });
            for (schema, table) in statement_tables.chain(all_tables) {
                if !in_scope(schema)
                    || !has_prefix(&table.name, prefix)
                    || !listed.insert(table as *const TableInfo)
                {
                    continue;
                }
                let mut item = identifier(&table.name, detail(schema, table));
                // After `schema.` the schema is written already.
                if only.is_none() && !dialect.is_default(schema) {
                    item.insert =
                        format!("{}.{}", quote_identifier(driver_id, schema), item.insert);
                }
                items.push(item);
            }
            if only.is_none() {
                for schema in &db.schemas {
                    if !schema.name.is_empty() && has_prefix(&schema.name, prefix) {
                        items.push(identifier(&schema.name, "schema".to_string()));
                    }
                }
            }
        }
        ContextKind::Columns => {
            if let Some(db) = schema {
                let mut seen: HashSet<*const TableInfo> = HashSet::default();
                let tables: Vec<&TableInfo> = visible
                    .iter()
                    .filter_map(|table| find_ref(db, dialect, table))
                    .map(|(_, table)| table)
                    .filter(|table| seen.insert(*table as *const TableInfo))
                    .collect();
                let several = tables.len() > 1;
                for table in tables {
                    for column in &table.columns {
                        if has_prefix(&column.name, prefix) {
                            let detail = if several {
                                format!("{} · {}", column.type_name, table.name)
                            } else {
                                column.type_name.clone()
                            };
                            items.push(identifier(&column.name, detail));
                        }
                    }
                }
            }
            push_keywords(&mut items, prefix);
        }
        ContextKind::ColumnsOf { qualifier } => {
            let Some(db) = schema else {
                return items;
            };
            // A derived table (`FROM (SELECT ..) d`) resolves but has no known columns.
            let table = match context.resolve(qualifier) {
                Some(table) => find_ref(db, dialect, table),
                None => match qualifier.as_slice() {
                    [name] => find_table(db, dialect, None, name),
                    [.., schema, name] => find_table(db, dialect, Some(schema), name),
                    [] => None,
                },
            };
            if let Some((_, table)) = table {
                for column in &table.columns {
                    if has_prefix(&column.name, prefix) {
                        items.push(identifier(&column.name, column.type_name.clone()));
                    }
                }
            }
        }
    }
    items
}

/// Keywords in upper case when the prefix is empty or upper case, else in lower case.
fn push_keywords(items: &mut Vec<SqlCompletion>, prefix: &str) {
    let upper = !prefix.chars().any(char::is_lowercase);
    for keyword in KEYWORDS {
        if has_prefix(keyword, prefix) {
            let text = if upper {
                keyword.to_string()
            } else {
                keyword.to_lowercase()
            };
            items.push(SqlCompletion {
                label: text.clone(),
                insert: text,
                detail: "keyword".to_string(),
            });
        }
    }
}

/// Completion for one console's editor.
pub struct SqlCompletionProvider {
    cache: Entity<SchemaCache>,
    dialect: Dialect,
    /// The last answer went out without the schema because it was still being fetched; the
    /// console asks again when it arrives.
    waiting: Rc<Cell<bool>>,
}

impl SqlCompletionProvider {
    pub fn new(cache: Entity<SchemaCache>, dialect: Dialect) -> Self {
        Self {
            cache,
            dialect,
            waiting: Rc::default(),
        }
    }

    /// Set while the menu shows less than it will once the schema is there.
    pub fn waiting(&self) -> Rc<Cell<bool>> {
        self.waiting.clone()
    }
}

impl CompletionProvider for SqlCompletionProvider {
    fn completions(
        &self,
        buffer: &Entity<Buffer>,
        buffer_position: language::Anchor,
        _trigger: editor::CompletionContext,
        _window: &mut Window,
        cx: &mut Context<Editor>,
    ) -> Task<Result<Vec<CompletionResponse>>> {
        let buffer = buffer.read(cx).snapshot();
        let offset = buffer_position.to_offset(&buffer);
        let context = context_at(&buffer.text(), offset);
        let start = buffer.anchor_before(context.prefix_start.min(offset));
        // Inside `"Fi|"` the closing quote is replaced too, inside `ow|ners` the rest of the
        // word when the name ends with it.
        let replace_range = |label: &str| {
            let end = context.replace_end_for(label);
            let end = if end > offset {
                buffer.anchor_after(end)
            } else {
                buffer_position
            };
            start..end
        };
        let schema = if context.kind == ContextKind::Nothing {
            None
        } else {
            self.cache.update(cx, |cache, cx| cache.schema(cx))
        };
        let loading = schema.is_none() && self.cache.read(cx).is_loading();
        // Worth reopening the menu for once the schema is there: a name being typed, or a place
        // where only names fit.
        let wants_names = !context.prefix.is_empty()
            || matches!(
                context.kind,
                ContextKind::Tables { .. } | ContextKind::ColumnsOf { .. }
            );
        self.waiting.set(loading && wants_names);
        let completions = completions_for(&context, schema.as_deref(), &self.dialect)
            .into_iter()
            .map(|item| Completion {
                replace_range: replace_range(&item.label),
                new_text: item.insert,
                label: CodeLabel::plain(item.label, None),
                documentation: Some(CompletionDocumentation::SingleLine(item.detail.into())),
                source: CompletionSource::Custom,
                icon_path: None,
                icon_color: None,
                match_start: None,
                snippet_deduplication_key: None,
                insert_text_mode: None,
                confirm: None,
                group: None,
            })
            .collect();
        // One response even when empty, so that buffer words do not stand in for it.
        Task::ready(Ok(vec![CompletionResponse {
            completions,
            display_options: CompletionDisplayOptions::default(),
            // The menu is never filtered by Zed (see `filter_completions`), so every keystroke
            // has to ask again for it to narrow.
            is_incomplete: true,
        }]))
    }

    fn is_completion_trigger(
        &self,
        _buffer: &Entity<Buffer>,
        _position: language::Anchor,
        text: &str,
        trigger_in_words: bool,
        _cx: &mut Context<Editor>,
    ) -> bool {
        text == "."
            || (trigger_in_words
                && text
                    .chars()
                    .next()
                    .is_some_and(|char| char.is_alphanumeric() || char == '_'))
    }

    /// The items come in the order `completions_for` gives them.
    fn sort_completions(&self) -> bool {
        false
    }

    /// `completions_for` already keeps only the names with the typed prefix. Zed's fuzzy filter
    /// would order the menu by match score instead (keywords among columns, a view before its
    /// table), so it is off. Zed then only asks again when the last answer was incomplete, which
    /// every answer is; that is cheap, the schema is in memory.
    fn filter_completions(&self) -> bool {
        false
    }
}

#[cfg(test)]
#[path = "completion_tests.rs"]
mod tests;
