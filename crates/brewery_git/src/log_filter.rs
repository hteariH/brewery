//! The Log tool window's filters, and how they become `git log` arguments.
//!
//! `LogFilter::to_git_args` is pure. The arguments go to `git log` verbatim through
//! `LogSource::Args`, after Zed's own format and order arguments, and also after the graph's
//! own search arguments (`--fixed-strings [--regexp-ignore-case] --grep <query>`), so:
//!
//! - every pattern is a fixed string. `--fixed-strings` and `--regexp-ignore-case` are global
//!   to git's limiting patterns (author and grep alike), and the graph search adds the first
//!   anyway; a regex-escaped author would break under it. Authors therefore match as
//!   case-insensitive substrings of `Name <email>`.
//! - a text filter adds `--all-match`, so a graph search on a text-filtered log finds commits
//!   matching both the filter and the query (git ORs several `--grep`s otherwise).
//! - each user value is one argv element (`--author=<v>`, `--grep=<v>`), never split and never
//!   passed through a shell; ref names starting with `-` are refused, and paths come after `--`.

use git::repository::{Branch, LogSource, RepoPath, commit_hash_search_query};
use gpui::SharedString;
use std::sync::Arc;
use time::{Date, Duration, OffsetDateTime};

/// One end of a date range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateBound {
    /// A calendar day, in git's local time: from its start for `since`, to its end for `until`.
    Day(Date),
    /// An exact instant (the presets).
    At(OffsetDateTime),
}

impl DateBound {
    /// Parses `YYYY-MM-DD` (surrounding whitespace allowed).
    pub fn parse_day(text: &str) -> Option<Self> {
        let text = text.trim();
        let bytes = text.as_bytes();
        let shape_ok = bytes.len() == 10
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes
                .iter()
                .enumerate()
                .all(|(ix, byte)| ix == 4 || ix == 7 || byte.is_ascii_digit());
        if !shape_ok {
            return None;
        }
        let year = text[0..4].parse::<i32>().ok()?;
        let month = time::Month::try_from(text[5..7].parse::<u8>().ok()?).ok()?;
        let day = text[8..10].parse::<u8>().ok()?;
        Date::from_calendar_date(year, month, day)
            .ok()
            .map(Self::Day)
    }

    fn since_arg(&self) -> String {
        match self {
            Self::Day(day) => format!("--since={}T00:00:00", iso_day(*day)),
            Self::At(at) => format!("--since={}", iso_instant(*at)),
        }
    }

    fn until_arg(&self) -> String {
        match self {
            Self::Day(day) => format!("--until={}T23:59:59", iso_day(*day)),
            Self::At(at) => format!("--until={}", iso_instant(*at)),
        }
    }

    /// Short text for the filter bar.
    pub fn label(&self) -> String {
        match self {
            Self::Day(day) => iso_day(*day),
            Self::At(at) => iso_day(at.date()),
        }
    }
}

fn iso_day(day: Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        day.year(),
        u8::from(day.month()),
        day.day()
    )
}

fn iso_instant(at: OffsetDateTime) -> String {
    let offset = at.offset();
    let (hours, minutes, _) = offset.as_hms();
    let sign = if offset.is_negative() { '-' } else { '+' };
    format!(
        "{}T{:02}:{:02}:{:02}{sign}{:02}:{:02}",
        iso_day(at.date()),
        at.hour(),
        at.minute(),
        at.second(),
        hours.unsigned_abs(),
        minutes.unsigned_abs()
    )
}

/// The Date filter's presets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatePreset {
    Last24Hours,
    Last7Days,
    Last30Days,
}

impl DatePreset {
    pub const ALL: [DatePreset; 3] = [Self::Last24Hours, Self::Last7Days, Self::Last30Days];

    pub fn label(self) -> &'static str {
        match self {
            Self::Last24Hours => "Last 24 hours",
            Self::Last7Days => "Last 7 days",
            Self::Last30Days => "Last 30 days",
        }
    }

    pub fn since(self, now: OffsetDateTime) -> DateBound {
        let back = match self {
            Self::Last24Hours => Duration::hours(24),
            Self::Last7Days => Duration::days(7),
            Self::Last30Days => Duration::days(30),
        };
        DateBound::At(now - back)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogFilterError {
    /// A ref name git would read as an option.
    OptionLikeRef(String),
    /// A value with a NUL byte cannot be an argument.
    NulByte,
}

impl std::fmt::Display for LogFilterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OptionLikeRef(name) => write!(f, "not a branch name: {name}"),
            Self::NulByte => write!(f, "filter values cannot contain NUL"),
        }
    }
}

impl std::error::Error for LogFilterError {}

/// What the Log tool window shows. Blank strings count as unset.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogFilter {
    /// Ref names (`main`, `origin/main`); empty means every branch, remote branch and tag.
    pub branches: Vec<String>,
    /// Fixed, case-insensitive substrings of `Name <email>`; any of them matches.
    pub authors: Vec<String>,
    pub since: Option<DateBound>,
    pub until: Option<DateBound>,
    /// Repository-relative paths; any of them matches.
    pub paths: Vec<RepoPath>,
    /// Text in the commit message, or a commit hash (7 or more hex digits) to go to.
    pub text: Option<String>,
}

fn non_blank(values: &[String]) -> impl Iterator<Item = &str> {
    values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
}

impl LogFilter {
    /// Nothing is set: neither a filter nor a hash to go to.
    pub fn is_empty(&self) -> bool {
        !self.limits_commits() && self.commit_hash().is_none()
    }

    /// The text, when it is a commit hash: the Log goes to that commit instead of grepping.
    pub fn commit_hash(&self) -> Option<&str> {
        self.text.as_deref().and_then(commit_hash_search_query)
    }

    fn grep_text(&self) -> Option<&str> {
        let text = self.text.as_deref()?.trim();
        if text.is_empty() || commit_hash_search_query(text).is_some() {
            return None;
        }
        Some(text)
    }

    /// Whether the arguments differ from the unfiltered log.
    pub fn limits_commits(&self) -> bool {
        non_blank(&self.branches).next().is_some()
            || non_blank(&self.authors).next().is_some()
            || self.since.is_some()
            || self.until.is_some()
            || !self.paths.is_empty()
            || self.grep_text().is_some()
    }

    /// The `git log` arguments after the format and order, ending with `--` and the paths.
    pub fn to_git_args(&self) -> Result<Vec<String>, LogFilterError> {
        let mut args = vec!["--ignore-missing".to_string()];
        let authors = non_blank(&self.authors).collect::<Vec<_>>();
        let grep = self.grep_text();
        if !authors.is_empty() || grep.is_some() {
            args.push("--fixed-strings".into());
            args.push("--regexp-ignore-case".into());
        }
        args.extend(authors.iter().map(|author| format!("--author={author}")));
        if let Some(since) = &self.since {
            args.push(since.since_arg());
        }
        if let Some(until) = &self.until {
            args.push(until.until_arg());
        }
        if let Some(grep) = grep {
            args.push("--all-match".into());
            args.push(format!("--grep={grep}"));
        }
        if !self.paths.is_empty() {
            // Rewrites parents to the commits that touch the paths, so the lanes connect.
            args.push("--parents".into());
        }
        let branches = non_blank(&self.branches).collect::<Vec<_>>();
        if let Some(bad) = branches.iter().find(|name| name.starts_with('-')) {
            return Err(LogFilterError::OptionLikeRef(bad.to_string()));
        }
        if branches.is_empty() {
            args.extend(["--branches", "--remotes", "--tags", "HEAD"].map(String::from));
        } else {
            args.extend(branches.iter().map(|name| name.to_string()));
        }
        args.push("--".into());
        args.extend(self.paths.iter().map(|path| path.as_unix_str().to_string()));
        if args.iter().any(|arg| arg.contains('\0')) {
            return Err(LogFilterError::NulByte);
        }
        Ok(args)
    }

    /// The graph source: `None` for the unfiltered log (`LogSource::All`, shared with Zed's
    /// Git Graph cache), `Args` otherwise.
    pub fn log_source(&self) -> Result<Option<LogSource>, LogFilterError> {
        let args = self.to_git_args()?;
        if !self.limits_commits() {
            return Ok(None);
        }
        let args: Arc<[SharedString]> = args.into_iter().map(SharedString::from).collect();
        Ok(Some(LogSource::Args(args)))
    }
}

/// Branch filter choices from the repository snapshot: local then remote names, sorted, without
/// `origin/HEAD`-style symbolic refs.
pub fn branch_choices(branches: &[Branch]) -> Vec<String> {
    let mut local = Vec::new();
    let mut remote = Vec::new();
    for branch in branches {
        if branch.is_remote() {
            if !branch.name().ends_with("/HEAD") {
                remote.push(branch.name().to_string());
            }
        } else {
            local.push(branch.name().to_string());
        }
    }
    local.sort();
    remote.sort();
    local.dedup();
    remote.dedup();
    local.extend(remote);
    local
}

#[cfg(test)]
#[path = "log_filter_tests.rs"]
mod tests;
