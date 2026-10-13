//! What SQL completion should offer at a cursor: tables, columns (of every table in the statement
//! or of one alias), keywords, or nothing (inside a string or a comment). Only the statement under
//! the cursor is read, including the part after the cursor (`SELECT | FROM owners`).
//!
//! The lexer knows the same strings, quoted identifiers, comments and dollar-quoted bodies as
//! `statements::split_statements`, and shares its helpers.

use crate::statements::{RESERVED, block_comment_end, dollar_tag};
use std::ops::Range;

/// What the completion menu lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextKind {
    /// Inside a string literal, a comment or a number.
    Nothing,
    Keywords,
    /// Tables and views, and schemas; with `schema`, only the tables of that schema.
    Tables {
        schema: Option<String>,
    },
    /// Columns of every table the statement names, then keywords.
    Columns,
    /// Columns of the table `qualifier` (an alias or a table name, possibly with its schema)
    /// stands for.
    ColumnsOf {
        qualifier: Vec<String>,
    },
}

/// A table the statement names, with its alias when it has one. Quoted names are unquoted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    pub schema: Option<String>,
    pub name: String,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlContext {
    pub kind: ContextKind,
    /// Where the identifier being typed starts (at its opening quote, if it has one); the
    /// completion replaces the text from here to the cursor.
    pub prefix_start: usize,
    /// What was typed of that identifier before the cursor, without the quote.
    pub prefix: String,
    /// The tables of the statement under the cursor, in order.
    pub tables: Vec<TableRef>,
}

impl SqlContext {
    /// The table `qualifier` refers to: an alias first, then a table name; matched
    /// case-insensitively.
    pub fn resolve(&self, qualifier: &[String]) -> Option<&TableRef> {
        match qualifier {
            [] => None,
            [single] => self
                .tables
                .iter()
                .find(|table| {
                    table
                        .alias
                        .as_ref()
                        .is_some_and(|alias| alias.eq_ignore_ascii_case(single))
                })
                .or_else(|| {
                    self.tables
                        .iter()
                        .find(|table| table.name.eq_ignore_ascii_case(single))
                }),
            [.., schema, name] => self.tables.iter().find(|table| {
                table.name.eq_ignore_ascii_case(name)
                    && table
                        .schema
                        .as_ref()
                        .is_some_and(|own| own.eq_ignore_ascii_case(schema))
            }),
        }
    }
}

/// Keywords after which a table name comes.
const TABLE_KEYWORDS: &[&str] = &["FROM", "JOIN", "INTO", "UPDATE", "TABLE", "DESCRIBE"];

/// Keywords after which a column (or an expression) comes.
const COLUMN_KEYWORDS: &[&str] = &[
    "SELECT",
    "WHERE",
    "AND",
    "OR",
    "ON",
    "BY",
    "SET",
    "HAVING",
    "DISTINCT",
    "NOT",
    "WHEN",
    "THEN",
    "ELSE",
    "CASE",
    "RETURNING",
];

/// Keywords the completion offers, in the order they are listed.
pub const KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "AND",
    "OR",
    "NOT",
    "NULL",
    "IS",
    "IN",
    "LIKE",
    "BETWEEN",
    "EXISTS",
    "AS",
    "DISTINCT",
    "JOIN",
    "INNER",
    "LEFT",
    "RIGHT",
    "FULL",
    "OUTER",
    "CROSS",
    "ON",
    "USING",
    "GROUP",
    "BY",
    "ORDER",
    "ASC",
    "DESC",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "UNION",
    "ALL",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "INSERT",
    "INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE",
    "RETURNING",
    "WITH",
    "CREATE",
    "ALTER",
    "DROP",
    "TRUNCATE",
    "RENAME",
    "TABLE",
    "VIEW",
    "INDEX",
    "COLUMN",
    "ADD",
    "PRIMARY",
    "KEY",
    "FOREIGN",
    "REFERENCES",
    "DEFAULT",
    "UNIQUE",
    "CHECK",
    "CONSTRAINT",
    "DESCRIBE",
    "EXPLAIN",
    "TRUE",
    "FALSE",
];

/// Leading keywords of statements that change the schema.
const SCHEMA_CHANGES: &[&str] = &["CREATE", "ALTER", "DROP", "RENAME", "TRUNCATE"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Word,
    /// `"name"` or `` `name` ``.
    Quoted,
    /// `'text'` or a dollar-quoted body.
    Str,
    Number,
    LineComment,
    BlockComment,
    Punct(char),
}

#[derive(Debug, Clone)]
struct Token {
    kind: Kind,
    range: Range<usize>,
    /// Strings, quoted names and block comments: whether the closing delimiter is there.
    closed: bool,
}

impl Token {
    fn is_comment(&self) -> bool {
        matches!(self.kind, Kind::LineComment | Kind::BlockComment)
    }

    fn is_punct(&self, char: char) -> bool {
        self.kind == Kind::Punct(char)
    }

    fn is_identifier(&self) -> bool {
        matches!(self.kind, Kind::Word | Kind::Quoted)
    }

    /// The cursor is inside, where nothing should be completed.
    fn swallows(&self, offset: usize) -> bool {
        let Range { start, end } = self.range;
        match self.kind {
            Kind::LineComment => start < offset && offset <= end,
            Kind::Str | Kind::BlockComment => {
                start < offset && (offset < end || (!self.closed && offset == end))
            }
            _ => false,
        }
    }

    /// A word in upper case, for keyword comparisons; `None` for anything else.
    fn keyword(&self, sql: &str) -> Option<String> {
        (self.kind == Kind::Word).then(|| sql[self.range.clone()].to_ascii_uppercase())
    }

    fn is_keyword_in(&self, sql: &str, set: &[&str]) -> bool {
        self.keyword(sql)
            .is_some_and(|word| set.contains(&word.as_str()))
    }

    /// The name an identifier stands for: a word as written, a quoted name unquoted.
    fn identifier(&self, sql: &str) -> String {
        let text = &sql[self.range.clone()];
        match self.kind {
            Kind::Quoted => {
                let quote = text.chars().next().unwrap_or('"');
                let inner = &text[quote.len_utf8()..];
                let inner = if self.closed {
                    inner.strip_suffix(quote).unwrap_or(inner)
                } else {
                    inner
                };
                inner.replace(&format!("{quote}{quote}"), &quote.to_string())
            }
            _ => text.to_string(),
        }
    }
}

fn is_word_start(char: char) -> bool {
    char.is_alphabetic() || char == '_'
}

fn is_word_char(char: char) -> bool {
    char.is_alphanumeric() || char == '_' || char == '$'
}

fn tokenize(sql: &str) -> Vec<Token> {
    let bytes = sql.as_bytes();
    let len = bytes.len();
    let mut tokens = Vec::new();
    let mut ix = 0;
    while ix < len {
        let Some(char) = sql[ix..].chars().next() else {
            break;
        };
        let next = bytes.get(ix + 1).copied();
        let start = ix;
        let (kind, closed) = match char {
            _ if char.is_whitespace() => {
                ix += char.len_utf8();
                continue;
            }
            '-' if next == Some(b'-') => {
                ix = sql[ix..].find('\n').map_or(len, |newline| ix + newline);
                (Kind::LineComment, true)
            }
            '/' if next == Some(b'*') => {
                ix = block_comment_end(bytes, ix);
                let closed = ix >= start + 4 && sql[..ix].ends_with("*/");
                (Kind::BlockComment, closed)
            }
            '\'' | '"' | '`' => {
                let quote = bytes[ix];
                ix += 1;
                let mut closed = false;
                while ix < len {
                    if bytes[ix] == quote {
                        if bytes.get(ix + 1) == Some(&quote) {
                            ix += 2;
                            continue;
                        }
                        ix += 1;
                        closed = true;
                        break;
                    }
                    ix += 1;
                }
                let kind = if char == '\'' {
                    Kind::Str
                } else {
                    Kind::Quoted
                };
                (kind, closed)
            }
            '$' => match dollar_tag(sql, ix) {
                Some(tag) => {
                    let body = ix + tag.len();
                    match sql[body..].find(tag) {
                        Some(close) => {
                            ix = body + close + tag.len();
                            (Kind::Str, true)
                        }
                        None => {
                            ix = len;
                            (Kind::Str, false)
                        }
                    }
                }
                None => {
                    ix += 1;
                    (Kind::Punct('$'), true)
                }
            },
            _ if is_word_start(char) => {
                ix += sql[ix..]
                    .find(|char: char| !is_word_char(char))
                    .unwrap_or(len - ix);
                (Kind::Word, true)
            }
            _ if char.is_ascii_digit() => {
                ix += sql[ix..]
                    .find(|char: char| !(char.is_alphanumeric() || char == '.' || char == '_'))
                    .unwrap_or(len - ix);
                (Kind::Number, true)
            }
            _ => {
                ix += char.len_utf8();
                (Kind::Punct(char), true)
            }
        };
        tokens.push(Token {
            kind,
            range: start..ix,
            closed,
        });
    }
    tokens
}

/// Whether `sql` starts (after comments) with a keyword that changes the schema, so that what
/// completion knows about the schema is out of date once it ran.
pub fn is_schema_change(sql: &str) -> bool {
    tokenize(sql)
        .iter()
        .find(|token| !token.is_comment())
        .is_some_and(|token| token.is_keyword_in(sql, SCHEMA_CHANGES))
}

fn is_keyword(word: &str) -> bool {
    let upper = word.to_ascii_uppercase();
    KEYWORDS.contains(&upper.as_str()) || RESERVED.contains(&word.to_ascii_lowercase().as_str())
}

/// What may follow the code in `before` (the statement up to the identifier being typed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Position {
    Tables,
    Columns,
    Keywords,
}

fn position_after(sql: &str, before: &[&Token]) -> Position {
    let Some(last) = before.last() else {
        return Position::Keywords;
    };
    match last.kind {
        Kind::Word if last.is_keyword_in(sql, TABLE_KEYWORDS) => Position::Tables,
        Kind::Word if last.is_keyword_in(sql, COLUMN_KEYWORDS) => Position::Columns,
        Kind::Punct('(') => Position::Columns,
        Kind::Punct(',') => list_position(sql, &before[..before.len() - 1]),
        Kind::Punct('=' | '<' | '>' | '!' | '+' | '-' | '*' | '/' | '%' | '|') => Position::Columns,
        _ => Position::Keywords,
    }
}

/// After a `,`: a FROM list continues with a table, anything else (a select list, values, a
/// SET list, arguments) with a column.
fn list_position(sql: &str, before: &[&Token]) -> Position {
    let mut depth = 0usize;
    for token in before.iter().rev() {
        match token.kind {
            Kind::Punct(')') => depth += 1,
            Kind::Punct('(') => {
                if depth == 0 {
                    return Position::Columns;
                }
                depth -= 1;
            }
            Kind::Word if depth == 0 => match token.keyword(sql).as_deref() {
                Some("FROM") => return Position::Tables,
                Some(
                    "SELECT" | "WHERE" | "BY" | "SET" | "HAVING" | "ON" | "VALUES" | "RETURNING",
                ) => return Position::Columns,
                _ => {}
            },
            _ => {}
        }
    }
    Position::Columns
}

/// The tables `statement` names after FROM, JOIN, INTO, UPDATE, TABLE and DESCRIBE, skipping
/// the token at `typing` (the identifier being typed is not a table yet).
fn table_refs(sql: &str, statement: &[&Token], typing: Option<usize>) -> Vec<TableRef> {
    let usable = |ix: usize| -> bool {
        Some(ix) != typing
            && statement.get(ix).is_some_and(|token| {
                token.kind == Kind::Quoted
                    || (token.kind == Kind::Word && !is_keyword(&sql[token.range.clone()]))
            })
    };
    let mut tables = Vec::new();
    let mut ix = 0;
    while ix < statement.len() {
        let keyword = statement[ix].keyword(sql);
        ix += 1;
        let Some(keyword) = keyword.filter(|word| TABLE_KEYWORDS.contains(&word.as_str())) else {
            continue;
        };
        loop {
            if !usable(ix) {
                break;
            }
            let mut parts = vec![statement[ix].identifier(sql)];
            // `schema.tab|`: the table is still being typed.
            let mut unfinished = false;
            ix += 1;
            while statement.get(ix).is_some_and(|token| token.is_punct('.'))
                && statement
                    .get(ix + 1)
                    .is_some_and(|token| token.is_identifier())
            {
                unfinished |= Some(ix + 1) == typing;
                parts.push(statement[ix + 1].identifier(sql));
                ix += 2;
            }
            let explicit_alias = statement
                .get(ix)
                .and_then(|token| token.keyword(sql))
                .is_some_and(|word| word == "AS");
            if explicit_alias {
                ix += 1;
            }
            let alias = if usable(ix) {
                ix += 1;
                Some(statement[ix - 1].identifier(sql))
            } else {
                if explicit_alias && Some(ix) == typing {
                    ix += 1;
                }
                None
            };
            let name = parts.pop().unwrap_or_default();
            if !unfinished {
                tables.push(TableRef {
                    schema: parts.pop(),
                    name,
                    alias,
                });
            }
            let more = keyword == "FROM" && statement.get(ix).is_some_and(|t| t.is_punct(','));
            if !more {
                break;
            }
            ix += 1;
        }
    }
    tables
}

/// The completion context for a cursor at byte `offset` of `sql`.
pub fn context_at(sql: &str, offset: usize) -> SqlContext {
    let mut offset = offset.min(sql.len());
    while !sql.is_char_boundary(offset) {
        offset -= 1;
    }
    let nothing = SqlContext {
        kind: ContextKind::Nothing,
        prefix_start: offset,
        prefix: String::new(),
        tables: Vec::new(),
    };
    let tokens = tokenize(sql);
    if tokens.iter().any(|token| token.swallows(offset)) {
        return nothing;
    }
    let code: Vec<&Token> = tokens.iter().filter(|token| !token.is_comment()).collect();
    let start = code
        .iter()
        .rposition(|token| token.is_punct(';') && token.range.end <= offset)
        .map_or(0, |ix| ix + 1);
    let end = code[start..]
        .iter()
        .position(|token| token.is_punct(';'))
        .map_or(code.len(), |ix| start + ix);
    let statement = &code[start..end];

    // The token the cursor is in or right after.
    let typing = statement.iter().position(|token| {
        token.range.start < offset
            && offset <= token.range.end
            && matches!(token.kind, Kind::Word | Kind::Quoted | Kind::Number)
    });
    let (prefix_start, prefix, before_end) = match typing.map(|ix| (ix, statement[ix])) {
        Some((_, token)) if token.kind == Kind::Number => return nothing,
        Some((ix, token)) => {
            let prefix = if token.kind == Kind::Quoted {
                let quote_len = sql[token.range.start..]
                    .chars()
                    .next()
                    .map_or(1, char::len_utf8);
                if token.closed && offset == token.range.end {
                    token.identifier(sql)
                } else {
                    sql[token.range.start + quote_len..offset].to_string()
                }
            } else {
                sql[token.range.start..offset].to_string()
            };
            (token.range.start, prefix, ix)
        }
        None => (
            offset,
            String::new(),
            statement
                .iter()
                .position(|token| token.range.end > offset)
                .unwrap_or(statement.len()),
        ),
    };
    let before = &statement[..before_end];
    let tables = table_refs(sql, statement, typing);

    let kind = if before.last().is_some_and(|token| token.is_punct('.')) {
        // `a.b.` : the qualifier parts, then what comes before them.
        let mut qualifier = Vec::new();
        let mut lead = before.len();
        while lead >= 2 && before[lead - 1].is_punct('.') && before[lead - 2].is_identifier() {
            qualifier.insert(0, before[lead - 2].identifier(sql));
            lead -= 2;
        }
        if qualifier.is_empty() {
            ContextKind::Keywords
        } else if position_after(sql, &before[..lead]) == Position::Tables {
            ContextKind::Tables {
                schema: qualifier.pop(),
            }
        } else {
            ContextKind::ColumnsOf { qualifier }
        }
    } else {
        match position_after(sql, before) {
            Position::Tables => ContextKind::Tables { schema: None },
            Position::Columns => ContextKind::Columns,
            Position::Keywords => ContextKind::Keywords,
        }
    };
    SqlContext {
        kind,
        prefix_start,
        prefix,
        tables,
    }
}

#[cfg(test)]
#[path = "sql_context_tests.rs"]
mod tests;
