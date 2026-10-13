//! SQL completion in the query console: what `sql_context` says the cursor wants, filled in from
//! the data source's `SchemaCache`, as an `editor::CompletionProvider`.

use crate::schema_cache::SchemaCache;
use crate::sql_context::{ContextKind, KEYWORDS, SqlContext, TableRef, context_at};
use crate::statements::quote_identifier;
use anyhow::Result;
use brewery_daemon_client::{DbSchema, TableInfo, TableKind};
use editor::{CompletionProvider, Editor};
use gpui::{Context, Entity, Task, Window};
use language::{Buffer, CodeLabel, ToOffset as _};
use project::lsp_store::CompletionDocumentation;
use project::{Completion, CompletionDisplayOptions, CompletionResponse, CompletionSource};
use std::cell::Cell;
use std::rc::Rc;

/// One item of the menu: what it shows, what it inserts and its detail (a column's type, "table",
/// "view", "schema" or "keyword").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlCompletion {
    pub label: String,
    pub insert: String,
    pub detail: String,
}

fn has_prefix(name: &str, prefix: &str) -> bool {
    name.to_lowercase().starts_with(&prefix.to_lowercase())
}

/// The table `name` (in `schema`, or in any schema when `None`), with the name of its schema.
fn find_table<'a>(
    db: &'a DbSchema,
    schema: Option<&str>,
    name: &str,
) -> Option<(&'a str, &'a TableInfo)> {
    db.schemas
        .iter()
        .filter(|candidate| schema.is_none_or(|schema| candidate.name.eq_ignore_ascii_case(schema)))
        .find_map(|candidate| {
            candidate
                .tables
                .iter()
                .find(|table| table.name.eq_ignore_ascii_case(name))
                .map(|table| (candidate.name.as_str(), table))
        })
}

fn find_ref<'a>(db: &'a DbSchema, table: &TableRef) -> Option<(&'a str, &'a TableInfo)> {
    find_table(db, table.schema.as_deref(), &table.name)
}

/// The menu for `context`, in the order it is shown: tables of the statement before the other
/// tables, columns before keywords. Without a schema (not fetched yet, or failed) only keywords.
pub fn completions_for(
    context: &SqlContext,
    schema: Option<&DbSchema>,
    driver_id: &str,
) -> Vec<SqlCompletion> {
    let prefix = context.prefix.as_str();
    let mut items = Vec::new();
    let identifier = |name: &str, detail: String| SqlCompletion {
        label: name.to_string(),
        insert: quote_identifier(driver_id, name),
        detail,
    };
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
            let mut listed: Vec<&TableInfo> = Vec::new();
            let statement_tables = context
                .tables
                .iter()
                .filter_map(|table| find_ref(db, table));
            let all_tables = db.schemas.iter().flat_map(|schema| {
                schema
                    .tables
                    .iter()
                    .map(|table| (schema.name.as_str(), table))
            });
            for (schema, table) in statement_tables.chain(all_tables) {
                let seen = listed.iter().any(|listed| std::ptr::eq(*listed, table));
                if seen || !in_scope(schema) || !has_prefix(&table.name, prefix) {
                    continue;
                }
                listed.push(table);
                items.push(identifier(&table.name, detail(schema, table)));
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
                let mut tables: Vec<&TableInfo> = Vec::new();
                for (_, table) in context
                    .tables
                    .iter()
                    .filter_map(|table| find_ref(db, table))
                {
                    if !tables.iter().any(|listed| std::ptr::eq(*listed, table)) {
                        tables.push(table);
                    }
                }
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
            let table = match context.resolve(qualifier) {
                Some(table) => find_ref(db, table),
                None => match qualifier.as_slice() {
                    [name] => find_table(db, None, name),
                    [.., schema, name] => find_table(db, Some(schema), name),
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
    driver_id: String,
    /// The last answer went out without the schema because it was still being fetched; the
    /// console asks again when it arrives.
    waiting: Rc<Cell<bool>>,
}

impl SqlCompletionProvider {
    pub fn new(cache: Entity<SchemaCache>, driver_id: String) -> Self {
        Self {
            cache,
            driver_id,
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
        let (context, replace_range) = {
            let buffer = buffer.read(cx);
            let offset = buffer_position.to_offset(buffer);
            let context = context_at(&buffer.text(), offset);
            let start = buffer.anchor_before(context.prefix_start.min(offset));
            (context, start..buffer_position)
        };
        let schema = if context.kind == ContextKind::Nothing {
            None
        } else {
            self.cache.update(cx, |cache, cx| cache.schema(cx))
        };
        let loading = schema.is_none() && self.cache.read(cx).is_loading();
        self.waiting.set(loading);
        let completions = completions_for(&context, schema.as_deref(), &self.driver_id)
            .into_iter()
            .map(|item| Completion {
                replace_range: replace_range.clone(),
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
            // Asked again on the next keystroke while the schema is on its way.
            is_incomplete: loading,
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
    /// table), so it is off and every keystroke asks again; that is cheap, the schema is in
    /// memory.
    fn filter_completions(&self) -> bool {
        false
    }
}

#[cfg(test)]
#[path = "completion_tests.rs"]
mod tests;
