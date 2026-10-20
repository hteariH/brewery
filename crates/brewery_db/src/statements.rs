//! Splits console text into SQL statements: by `;` outside strings, quoted identifiers, `--` and
//! `/* */` comments (nested, as in PostgreSQL), PostgreSQL dollar-quoted bodies (`$$`, `$tag$`)
//! and the `BEGIN ... END` bodies of triggers and routines. All ranges are byte ranges of the
//! given text, trimmed of whitespace and of leading and trailing comments, without the `;`.
//!
//! What else counts depends on the database (`SqlFlavor`): MySQL and MariaDB have `#` comments
//! and backslash escapes in strings, PostgreSQL has them in `E'..'` strings.
//!
//! Not handled: the `DELIMITER` command of MySQL clients (select the whole routine to run it).

use std::ops::Range;

/// The lexical rules of a database's SQL that change where statements and strings end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SqlFlavor {
    #[default]
    Standard,
    /// `E'it\'s'` strings.
    PostgreSql,
    /// `#` line comments and backslash escapes in `'..'` and `".."` strings.
    MySql,
}

impl SqlFlavor {
    pub fn for_driver(driver_id: &str) -> Self {
        match driver_id {
            "postgresql" => Self::PostgreSql,
            "mysql" | "mariadb" => Self::MySql,
            _ => Self::Standard,
        }
    }

    pub(crate) fn hash_comments(self) -> bool {
        self == Self::MySql
    }

    /// Whether a backslash escapes the next character in the string whose opening `quote` is at
    /// `start`.
    pub(crate) fn backslash_escapes(self, bytes: &[u8], start: usize) -> bool {
        let quote = bytes[start];
        match self {
            Self::MySql => quote == b'\'' || quote == b'"',
            // `E'..'`, where the `E` is not the end of a longer word.
            Self::PostgreSql => {
                quote == b'\''
                    && start > 0
                    && matches!(bytes[start - 1], b'E' | b'e')
                    && (start < 2
                        || !(is_identifier_byte(bytes[start - 2]) || bytes[start - 2] == b'$'))
            }
            Self::Standard => false,
        }
    }
}

/// Where the string or quoted name opening at `start` ends (after its closing quote, or at the end
/// of the text), and whether it was closed. A doubled quote does not close it.
pub(crate) fn quoted_end(bytes: &[u8], start: usize, backslash: bool) -> (usize, bool) {
    let quote = bytes[start];
    let mut ix = start + 1;
    while ix < bytes.len() {
        match bytes[ix] {
            b'\\' if backslash => ix += 2,
            byte if byte == quote => {
                if bytes.get(ix + 1) == Some(&quote) {
                    ix += 2;
                    continue;
                }
                return (ix + 1, true);
            }
            _ => ix += 1,
        }
    }
    (bytes.len(), false)
}

/// Follows the words of one statement to tell the `;` inside a `BEGIN ... END` body of a trigger
/// or routine (`CREATE TRIGGER`, `PROCEDURE`, `FUNCTION`, `EVENT`, MariaDB's `BEGIN NOT ATOMIC`)
/// from the one that ends the statement.
#[derive(Debug, Default)]
struct Body {
    words: usize,
    create: bool,
    routine: bool,
    begin_first: bool,
    /// Open `BEGIN` and `CASE` blocks.
    depth: usize,
    /// The last word was an `END` that closed a block.
    after_end: bool,
}

impl Body {
    fn word(&mut self, word: &str) {
        let upper = word.to_ascii_uppercase();
        let first = self.words;
        self.words += 1;
        if std::mem::take(&mut self.after_end) {
            match upper.as_str() {
                // `END IF` and the like close blocks that were never counted.
                "IF" | "LOOP" | "WHILE" | "REPEAT" | "FOR" => {
                    self.depth += 1;
                    return;
                }
                // `END CASE` closes the counted `CASE`.
                "CASE" => return,
                _ => {}
            }
        }
        match first {
            0 => {
                self.create = upper == "CREATE";
                self.begin_first = upper == "BEGIN";
                return;
            }
            1 if self.begin_first && upper == "NOT" => {
                self.depth = 1;
                return;
            }
            _ => {}
        }
        if self.create
            && self.depth == 0
            && matches!(
                upper.as_str(),
                "TRIGGER" | "PROCEDURE" | "FUNCTION" | "EVENT"
            )
        {
            self.routine = true;
        }
        match upper.as_str() {
            "BEGIN" if self.routine || self.depth > 0 => self.depth += 1,
            "CASE" if self.depth > 0 => self.depth += 1,
            "END" if self.depth > 0 => {
                self.depth -= 1;
                self.after_end = true;
            }
            _ => {}
        }
    }
}

/// `split_statements_for` with the rules every database shares.
pub fn split_statements(text: &str) -> Vec<Range<usize>> {
    split_statements_for(text, SqlFlavor::Standard)
}

/// Every statement in `text`, in order; pieces with nothing but whitespace and comments are
/// skipped.
pub fn split_statements_for(text: &str, flavor: SqlFlavor) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut statements = Vec::new();
    // The code seen so far in the current statement.
    let mut code: Option<Range<usize>> = None;
    let mark = |code: &mut Option<Range<usize>>, range: Range<usize>| match code {
        Some(code) => code.end = range.end,
        None => *code = Some(range),
    };
    let mut body = Body::default();
    let mut ix = 0;
    while ix < len {
        let next = bytes.get(ix + 1).copied();
        match bytes[ix] {
            b';' if body.depth == 0 => {
                statements.extend(code.take());
                body = Body::default();
                ix += 1;
            }
            b';' => {
                // `END;` closed a block: a word after it does not belong to that `END`.
                body.after_end = false;
                mark(&mut code, ix..ix + 1);
                ix += 1;
            }
            b'-' if next == Some(b'-') => {
                ix = text[ix..].find('\n').map_or(len, |newline| ix + newline);
            }
            b'#' if flavor.hash_comments() => {
                ix = text[ix..].find('\n').map_or(len, |newline| ix + newline);
            }
            b'/' if next == Some(b'*') => ix = block_comment_end(bytes, ix),
            b'\'' | b'"' | b'`' => {
                let start = ix;
                ix = quoted_end(bytes, ix, flavor.backslash_escapes(bytes, ix)).0;
                mark(&mut code, start..ix);
            }
            byte if byte.is_ascii_alphabetic() || byte == b'_' => {
                let start = ix;
                while ix < len && is_identifier_byte(bytes[ix]) {
                    ix += 1;
                }
                body.word(&text[start..ix]);
                mark(&mut code, start..ix);
            }
            b'$' => {
                let start = ix;
                ix = match dollar_tag(text, ix) {
                    Some(tag) => {
                        let body = ix + tag.len();
                        text[body..]
                            .find(tag)
                            .map_or(len, |close| body + close + tag.len())
                    }
                    None => ix + 1,
                };
                mark(&mut code, start..ix);
            }
            byte if byte.is_ascii_whitespace() => ix += 1,
            _ => {
                let start = ix;
                ix += text[ix..].chars().next().map_or(1, char::len_utf8);
                mark(&mut code, start..ix);
            }
        }
    }
    statements.extend(code);
    statements
}

/// Where a `/*` comment starting at `start` ends (after its `*/`, or at the end of the text).
pub(crate) fn block_comment_end(bytes: &[u8], start: usize) -> usize {
    let mut depth = 0usize;
    let mut ix = start;
    while ix < bytes.len() {
        match (bytes[ix], bytes.get(ix + 1).copied()) {
            (b'/', Some(b'*')) => {
                depth += 1;
                ix += 2;
            }
            (b'*', Some(b'/')) => {
                depth -= 1;
                ix += 2;
                if depth == 0 {
                    return ix;
                }
            }
            _ => ix += 1,
        }
    }
    bytes.len()
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The opening `$tag$` (or `$$`) of a dollar-quoted body at `start`. Not a tag: `$1` parameters
/// and `$` inside identifiers such as `a$b`.
pub(crate) fn dollar_tag(text: &str, start: usize) -> Option<&str> {
    let bytes = text.as_bytes();
    if start > 0 && (is_identifier_byte(bytes[start - 1]) || bytes[start - 1] == b'$') {
        return None;
    }
    let mut ix = start + 1;
    if bytes
        .get(ix)
        .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
    {
        while bytes.get(ix).copied().is_some_and(is_identifier_byte) {
            ix += 1;
        }
    }
    (bytes.get(ix) == Some(&b'$')).then(|| &text[start..=ix])
}

/// The statement to run for a cursor at `offset`: the one the cursor is in or touches; in the
/// space between statements (or after the last `;`) the one above, else the one below.
pub fn statement_at(text: &str, offset: usize, flavor: SqlFlavor) -> Option<Range<usize>> {
    let offset = offset.min(text.len());
    let statements = split_statements_for(text, flavor);
    if let Some(inside) = statements
        .iter()
        .find(|range| range.start <= offset && offset <= range.end)
    {
        return Some(inside.clone());
    }
    statements
        .iter()
        .rev()
        .find(|range| range.end <= offset)
        .or_else(|| statements.first())
        .cloned()
}

/// What Run executes: with an empty selection the statement at the cursor, otherwise the
/// statements inside the selection (in offsets of `text`).
pub fn statements_to_run(
    text: &str,
    selection: Range<usize>,
    flavor: SqlFlavor,
) -> Vec<Range<usize>> {
    let end = selection.end.min(text.len());
    let start = selection.start.min(end);
    if start == end {
        return statement_at(text, start, flavor).into_iter().collect();
    }
    let Some(selected) = text.get(start..end) else {
        return Vec::new();
    };
    split_statements_for(selected, flavor)
        .into_iter()
        .map(|range| range.start + start..range.end + start)
        .collect()
}

/// Keywords (reserved or not) of the supported databases. Quoting a word that would have worked
/// bare is harmless, so the list errs on the long side.
pub(crate) const RESERVED: &[&str] = &[
    "all",
    "analyse",
    "analyze",
    "and",
    "any",
    "array",
    "as",
    "asc",
    "asymmetric",
    "authorization",
    "between",
    "binary",
    "both",
    "by",
    "call",
    "case",
    "cast",
    "check",
    "collate",
    "column",
    "comment",
    "constraint",
    "create",
    "cross",
    "current",
    "current_catalog",
    "current_date",
    "current_role",
    "current_schema",
    "current_time",
    "current_timestamp",
    "current_user",
    "database",
    "databases",
    "day",
    "default",
    "deferrable",
    "delete",
    "desc",
    "describe",
    "distinct",
    "do",
    "drop",
    "else",
    "end",
    "except",
    "exists",
    "explain",
    "false",
    "fetch",
    "filter",
    "for",
    "foreign",
    "freeze",
    "from",
    "full",
    "function",
    "grant",
    "group",
    "groups",
    "having",
    "hour",
    "if",
    "ilike",
    "in",
    "index",
    "inner",
    "insert",
    "intersect",
    "interval",
    "into",
    "is",
    "isnull",
    "join",
    "key",
    "keys",
    "lateral",
    "leading",
    "left",
    "level",
    "like",
    "limit",
    "localtime",
    "localtimestamp",
    "minute",
    "month",
    "natural",
    "not",
    "notnull",
    "null",
    "offset",
    "on",
    "only",
    "or",
    "order",
    "outer",
    "over",
    "overlaps",
    "partition",
    "placing",
    "primary",
    "range",
    "rank",
    "read",
    "references",
    "returning",
    "right",
    "row",
    "rows",
    "schema",
    "second",
    "select",
    "session_user",
    "set",
    "show",
    "similar",
    "some",
    "symmetric",
    "system_user",
    "table",
    "tablesample",
    "then",
    "to",
    "trailing",
    "trigger",
    "true",
    "union",
    "unique",
    "update",
    "user",
    "using",
    "values",
    "variadic",
    "verbose",
    "view",
    "when",
    "where",
    "window",
    "with",
    "year",
];

/// `name` as SQL for `driver_id`: bare when it is a plain identifier, otherwise quoted with `"`
/// (PostgreSQL, SQLite and others) or `` ` `` (MySQL, MariaDB).
pub fn quote_identifier(driver_id: &str, name: &str) -> String {
    let quote = match driver_id {
        "mysql" | "mariadb" => '`',
        _ => '"',
    };
    // PostgreSQL folds unquoted names to lower case, so any upper case letter needs quotes.
    let folds_case = !matches!(driver_id, "mysql" | "mariadb" | "sqlite");
    let plain = name
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && name
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || char == '_')
        && !(folds_case && name.chars().any(|char| char.is_ascii_uppercase()))
        && !RESERVED.contains(&name.to_ascii_lowercase().as_str());
    if plain {
        return name.to_string();
    }
    let escaped = name.replace(quote, &format!("{quote}{quote}"));
    format!("{quote}{escaped}{quote}")
}

/// The name of the table at `path` (namespace, if any, then the table name) as SQL.
pub fn qualified_name(driver_id: &str, path: &[String]) -> String {
    path.iter()
        .map(|part| quote_identifier(driver_id, part))
        .collect::<Vec<_>>()
        .join(".")
}

/// `SELECT * FROM` the table at `path` (namespace, if any, then the table name).
pub fn select_all_from(driver_id: &str, path: &[String]) -> String {
    format!("SELECT * FROM {}", qualified_name(driver_id, path))
}

/// The first `limit` rows of the table at `path`. Drivers without a known `LIMIT` get the plain
/// `SELECT *`, which the result grid pages anyway.
pub fn select_top_from(driver_id: &str, path: &[String], limit: u32) -> String {
    let select = select_all_from(driver_id, path);
    match driver_id {
        "postgresql" | "mysql" | "mariadb" | "sqlite" => format!("{select} LIMIT {limit}"),
        _ => select,
    }
}

#[cfg(test)]
#[path = "statements_tests.rs"]
mod tests;
