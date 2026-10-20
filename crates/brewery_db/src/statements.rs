//! Splits console text into SQL statements: by `;` outside strings, quoted identifiers, `--` and
//! `/* */` comments (nested, as in PostgreSQL) and PostgreSQL dollar-quoted bodies (`$$`,
//! `$tag$`). All ranges are byte ranges of the given text, trimmed of whitespace and of leading
//! and trailing comments, without the `;`.
//!
//! Not handled (they split wrongly; selecting the whole text runs it as one statement): MySQL `#`
//! comments, backslash escapes in strings (`'it's'`, PostgreSQL `E'..'`) and compound bodies
//! with inner `;` that are not dollar-quoted (`BEGIN ... END` in SQLite triggers or MySQL
//! routines).

use std::ops::Range;

/// Every statement in `text`, in order; pieces with nothing but whitespace and comments are
/// skipped.
pub fn split_statements(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut statements = Vec::new();
    // The code seen so far in the current statement.
    let mut code: Option<Range<usize>> = None;
    let mark = |code: &mut Option<Range<usize>>, range: Range<usize>| match code {
        Some(code) => code.end = range.end,
        None => *code = Some(range),
    };
    let mut ix = 0;
    while ix < len {
        let next = bytes.get(ix + 1).copied();
        match bytes[ix] {
            b';' => {
                statements.extend(code.take());
                ix += 1;
            }
            b'-' if next == Some(b'-') => {
                ix = text[ix..].find('\n').map_or(len, |newline| ix + newline);
            }
            b'/' if next == Some(b'*') => ix = block_comment_end(bytes, ix),
            quote @ (b'\'' | b'"' | b'`') => {
                let start = ix;
                ix += 1;
                // A doubled quote closes and reopens, which leaves the split unaffected.
                while ix < len && bytes[ix] != quote {
                    ix += 1;
                }
                ix = (ix + 1).min(len);
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
pub fn statement_at(text: &str, offset: usize) -> Option<Range<usize>> {
    let offset = offset.min(text.len());
    let statements = split_statements(text);
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
pub fn statements_to_run(text: &str, selection: Range<usize>) -> Vec<Range<usize>> {
    let end = selection.end.min(text.len());
    let start = selection.start.min(end);
    if start == end {
        return statement_at(text, start).into_iter().collect();
    }
    let Some(selected) = text.get(start..end) else {
        return Vec::new();
    };
    split_statements(selected)
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
