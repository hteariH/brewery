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
/// `name` is empty for a derived table (`FROM (SELECT ...) d`): its columns are not known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    pub schema: Option<String>,
    pub name: String,
    pub alias: Option<String>,
}

/// The whole statement, as a scope.
const STATEMENT: Range<usize> = 0..usize::MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlContext {
    pub kind: ContextKind,
    /// Where the identifier being typed starts (at its opening quote, if it has one); the
    /// completion replaces the text from here to `replace_end`.
    pub prefix_start: usize,
    /// The cursor, or the end of a closed quoted name the cursor is in (so that its closing
    /// quote is replaced too).
    pub replace_end: usize,
    /// What was typed of that identifier before the cursor, without the quote.
    pub prefix: String,
    /// The rest of a bare word the cursor is inside (`ners` in `ow|ners`); see `replace_end_for`.
    suffix: String,
    /// The tables of the statement under the cursor, in order, subqueries included.
    pub tables: Vec<TableRef>,
    /// For each of `tables`, the inside of the subquery it belongs to (`STATEMENT` at the top).
    scopes: Vec<Range<usize>>,
    offset: usize,
}

impl SqlContext {
    /// Where accepting `label` ends its replacement: inside a bare word (`ow|ners`), through the
    /// end of the word when `label` ends with the rest of it (as Zed's default `lsp_insert_mode`,
    /// `replace_suffix`, does), else `replace_end`.
    pub fn replace_end_for(&self, label: &str) -> usize {
        let ends_with_suffix = !self.suffix.is_empty()
            && label
                .len()
                .checked_sub(self.suffix.len())
                .and_then(|start| label.get(start..))
                .is_some_and(|end| end.eq_ignore_ascii_case(&self.suffix));
        if ends_with_suffix {
            self.replace_end + self.suffix.len()
        } else {
            self.replace_end
        }
    }

    /// Indices of `tables` by level: the innermost subquery around the cursor first, then the
    /// levels around it; last, every table of the statement as a fallback.
    fn levels(&self) -> Vec<Vec<usize>> {
        let mut around: Vec<&Range<usize>> = self
            .scopes
            .iter()
            .filter(|scope| scope.start <= self.offset && self.offset <= scope.end)
            .collect();
        around.sort_by_key(|scope| (scope.end - scope.start, scope.start));
        around.dedup();
        let mut levels: Vec<Vec<usize>> = around
            .into_iter()
            .map(|level| {
                (0..self.tables.len())
                    .filter(|&ix| &self.scopes[ix] == level)
                    .collect()
            })
            .collect();
        levels.push((0..self.tables.len()).collect());
        levels
    }

    /// The tables a column at the cursor may come from: those of the subquery the cursor is in
    /// first, then those of the levels around it.
    pub fn visible_tables(&self) -> Vec<&TableRef> {
        let mut levels = self.levels();
        levels.pop();
        levels
            .into_iter()
            .flatten()
            .map(|ix| &self.tables[ix])
            .collect()
    }

    /// The table `qualifier` refers to: level by level from the cursor outwards, an alias first,
    /// then a table name; matched case-insensitively.
    pub fn resolve(&self, qualifier: &[String]) -> Option<&TableRef> {
        self.levels().iter().find_map(|level| {
            let tables = || level.iter().map(|&ix| &self.tables[ix]);
            match qualifier {
                [] => None,
                [single] => tables()
                    .find(|table| {
                        table
                            .alias
                            .as_ref()
                            .is_some_and(|alias| alias.eq_ignore_ascii_case(single))
                    })
                    .or_else(|| {
                        tables().find(|table| {
                            !table.name.is_empty() && table.name.eq_ignore_ascii_case(single)
                        })
                    }),
                [.., schema, name] => tables().find(|table| {
                    table.name.eq_ignore_ascii_case(name)
                        && table
                            .schema
                            .as_ref()
                            .is_some_and(|own| own.eq_ignore_ascii_case(schema))
                }),
            }
        })
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
        // `SELECT * |`, `SELECT t.* |`: the item is complete.
        Kind::Punct('*')
            if before.len() < 2
                || before[before.len() - 2].is_punct('.')
                || before[before.len() - 2].is_punct(',')
                || before[before.len() - 2].is_keyword_in(sql, &["SELECT", "DISTINCT"]) =>
        {
            Position::Keywords
        }
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

fn is_subquery_open(sql: &str, statement: &[&Token], ix: usize) -> bool {
    statement[ix].is_punct('(')
        && statement
            .get(ix + 1)
            .is_some_and(|next| next.is_keyword_in(sql, &["SELECT", "WITH", "VALUES"]))
}

/// For every `(` of `statement`, the index of its matching `)` (when it is closed).
fn closing_parens(statement: &[&Token]) -> Vec<Option<usize>> {
    let mut closes = vec![None; statement.len()];
    let mut open = Vec::new();
    for (ix, token) in statement.iter().enumerate() {
        if token.is_punct('(') {
            open.push(ix);
        } else if token.is_punct(')')
            && let Some(opened) = open.pop()
        {
            closes[opened] = Some(ix);
        }
    }
    closes
}

/// For every token of `statement`, the inside of the innermost subquery it is in (byte range,
/// open-ended while the subquery is not closed), or `STATEMENT`.
fn token_scopes(sql: &str, statement: &[&Token], closes: &[Option<usize>]) -> Vec<Range<usize>> {
    let mut scopes = Vec::with_capacity(statement.len());
    // Every open `(`, with the scope it opens when it is a subquery.
    let mut open: Vec<Option<Range<usize>>> = Vec::new();
    for (ix, token) in statement.iter().enumerate() {
        if token.is_punct(')') {
            open.pop();
        }
        let scope = open
            .iter()
            .rev()
            .flatten()
            .next()
            .cloned()
            .unwrap_or(STATEMENT);
        scopes.push(scope);
        if token.is_punct('(') {
            open.push(is_subquery_open(sql, statement, ix).then(|| {
                let end = closes[ix].map_or(usize::MAX, |close| statement[close].range.start);
                token.range.end..end
            }));
        }
    }
    scopes
}

/// The tables `statement` names after FROM, JOIN, INTO, UPDATE, TABLE and DESCRIBE, each with
/// its scope, skipping the token at `typing` (the identifier being typed is not a table yet).
fn table_refs(
    sql: &str,
    statement: &[&Token],
    typing: Option<usize>,
) -> (Vec<TableRef>, Vec<Range<usize>>) {
    let closes = closing_parens(statement);
    let scopes = token_scopes(sql, statement, &closes);
    let usable = |ix: usize| -> bool {
        Some(ix) != typing
            && statement.get(ix).is_some_and(|token| {
                token.kind == Kind::Quoted
                    || (token.kind == Kind::Word && !is_keyword(&sql[token.range.clone()]))
            })
    };
    // An alias at `ix` (after an optional AS): the alias and the index after it.
    let alias_at = |mut ix: usize| -> (Option<String>, usize) {
        let explicit = statement
            .get(ix)
            .and_then(|token| token.keyword(sql))
            .is_some_and(|word| word == "AS");
        if explicit {
            ix += 1;
        }
        if usable(ix) {
            (Some(statement[ix].identifier(sql)), ix + 1)
        } else if explicit && Some(ix) == typing {
            (None, ix + 1)
        } else {
            (None, ix)
        }
    };
    let mut tables = Vec::new();
    let mut table_scopes = Vec::new();
    for (keyword_ix, token) in statement.iter().enumerate() {
        let Some(keyword) = token
            .keyword(sql)
            .filter(|word| TABLE_KEYWORDS.contains(&word.as_str()))
        else {
            continue;
        };
        let mut ix = keyword_ix + 1;
        loop {
            // `FROM (owners o JOIN ...)`: the tables inside belong to this level.
            while statement.get(ix).is_some_and(|token| token.is_punct('('))
                && !is_subquery_open(sql, statement, ix)
            {
                ix += 1;
            }
            if ix < statement.len() && is_subquery_open(sql, statement, ix) {
                // A derived table; the tables inside are read at its own FROM.
                let scope = scopes[ix].clone();
                let Some(close) = closes[ix] else {
                    break;
                };
                let (alias, next) = alias_at(close + 1);
                ix = next;
                if alias.is_some() {
                    tables.push(TableRef {
                        schema: None,
                        name: String::new(),
                        alias,
                    });
                    table_scopes.push(scope);
                }
            } else {
                if !usable(ix) {
                    break;
                }
                let scope = scopes[ix].clone();
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
                let (alias, next) = alias_at(ix);
                ix = next;
                let name = parts.pop().unwrap_or_default();
                if !unfinished {
                    tables.push(TableRef {
                        schema: parts.pop(),
                        name,
                        alias,
                    });
                    table_scopes.push(scope);
                }
            }
            let more = keyword == "FROM" && statement.get(ix).is_some_and(|t| t.is_punct(','));
            if !more {
                break;
            }
            ix += 1;
        }
    }
    (tables, table_scopes)
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
        replace_end: offset,
        prefix: String::new(),
        suffix: String::new(),
        tables: Vec::new(),
        scopes: Vec::new(),
        offset,
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
    let mut replace_end = offset;
    let mut suffix = String::new();
    let (prefix_start, prefix, before_end) = match typing.map(|ix| (ix, statement[ix])) {
        Some((_, token)) if token.kind == Kind::Number => return nothing,
        Some((ix, token)) => {
            let prefix = if token.kind == Kind::Quoted {
                if token.closed {
                    replace_end = token.range.end;
                }
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
                suffix = sql[offset..token.range.end].to_string();
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
    let (tables, scopes) = table_refs(sql, statement, typing);

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
        replace_end,
        prefix,
        suffix,
        tables,
        scopes,
        offset,
    }
}

#[cfg(test)]
#[path = "sql_context_tests.rs"]
mod tests;
