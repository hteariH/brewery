use super::*;
use time::macros::{date, datetime};

fn path(text: &str) -> RepoPath {
    RepoPath::new(text).expect("repo path")
}

fn args(filter: &LogFilter) -> Vec<String> {
    filter.to_git_args().expect("valid filter")
}

#[test]
fn an_empty_filter_lists_every_ref_like_log_source_all() {
    let filter = LogFilter::default();
    assert!(filter.is_empty());
    assert_eq!(
        args(&filter),
        [
            "--ignore-missing",
            "--branches",
            "--remotes",
            "--tags",
            "HEAD",
            "--"
        ]
    );
    assert_eq!(filter.log_source().expect("valid"), None);
}

#[test]
fn branches_replace_the_default_refs() {
    let filter = LogFilter {
        branches: vec!["main".into(), "origin/feature/x".into()],
        ..Default::default()
    };
    assert_eq!(
        args(&filter),
        ["--ignore-missing", "main", "origin/feature/x", "--"]
    );
}

#[test]
fn a_branch_starting_with_a_dash_is_refused() {
    for name in ["-x", "--all", "--output=/tmp/x"] {
        let filter = LogFilter {
            branches: vec!["main".into(), name.into()],
            ..Default::default()
        };
        assert_eq!(
            filter.to_git_args(),
            Err(LogFilterError::OptionLikeRef(name.to_string())),
            "{name}"
        );
        assert!(filter.log_source().is_err());
    }
}

#[test]
fn blank_values_are_ignored() {
    let filter = LogFilter {
        branches: vec!["  ".into()],
        authors: vec![String::new(), " ".into()],
        text: Some("   ".into()),
        ..Default::default()
    };
    assert!(filter.is_empty());
    assert_eq!(filter.log_source().expect("valid"), None);
}

#[test]
fn authors_are_fixed_case_insensitive_strings() {
    let filter = LogFilter {
        authors: vec!["Alice A.B".into(), "bob@example.com".into()],
        ..Default::default()
    };
    assert_eq!(
        args(&filter),
        [
            "--ignore-missing",
            "--fixed-strings",
            "--regexp-ignore-case",
            "--author=Alice A.B",
            "--author=bob@example.com",
            "--branches",
            "--remotes",
            "--tags",
            "HEAD",
            "--",
        ]
    );
}

#[test]
fn user_values_stay_single_arguments_even_when_they_look_like_options() {
    // `--author=` and `--grep=` carry the value in the same argv element: git never reads it
    // as an option, and nothing is escaped or split.
    let filter = LogFilter {
        authors: vec!["--all *(x) \"y\" $HOME".into()],
        text: Some("-n 1; rm -rf /".into()),
        ..Default::default()
    };
    let args = args(&filter);
    assert!(args.contains(&"--author=--all *(x) \"y\" $HOME".to_string()));
    assert!(args.contains(&"--grep=-n 1; rm -rf /".to_string()));
}

#[test]
fn a_nul_byte_is_refused() {
    let filter = LogFilter {
        authors: vec!["a\0b".into()],
        ..Default::default()
    };
    assert_eq!(filter.to_git_args(), Err(LogFilterError::NulByte));
}

#[test]
fn dates_are_iso_8601() {
    let filter = LogFilter {
        since: Some(DateBound::Day(date!(2026 - 01 - 05))),
        until: Some(DateBound::Day(date!(2026 - 02 - 28))),
        ..Default::default()
    };
    assert_eq!(
        &args(&filter)[1..3],
        ["--since=2026-01-05T00:00:00", "--until=2026-02-28T23:59:59"]
    );

    let filter = LogFilter {
        since: Some(DateBound::At(datetime!(2026-10-12 09:05:03 UTC))),
        ..Default::default()
    };
    assert_eq!(args(&filter)[1], "--since=2026-10-12T09:05:03+00:00");
}

#[test]
fn days_parse_from_yyyy_mm_dd() {
    assert_eq!(
        DateBound::parse_day(" 2026-03-09 "),
        Some(DateBound::Day(date!(2026 - 03 - 09)))
    );
    for bad in [
        "",
        "2026-3-9",
        "2026-13-01",
        "2026-02-30",
        "yesterday",
        "2026-03-09x",
    ] {
        assert_eq!(DateBound::parse_day(bad), None, "{bad}");
    }
}

#[test]
fn presets_count_back_from_now() {
    let now = datetime!(2026-10-13 12:00:00 UTC);
    assert_eq!(
        DatePreset::Last24Hours.since(now),
        DateBound::At(datetime!(2026-10-12 12:00:00 UTC))
    );
    assert_eq!(
        DatePreset::Last7Days.since(now),
        DateBound::At(datetime!(2026-10-06 12:00:00 UTC))
    );
    assert_eq!(
        DatePreset::Last30Days.since(now),
        DateBound::At(datetime!(2026-09-13 12:00:00 UTC))
    );
}

#[test]
fn text_greps_fixed_strings_and_must_match_with_the_graph_search() {
    let filter = LogFilter {
        text: Some("fix: (crash)".into()),
        ..Default::default()
    };
    assert_eq!(
        args(&filter),
        [
            "--ignore-missing",
            "--fixed-strings",
            "--regexp-ignore-case",
            "--all-match",
            "--grep=fix: (crash)",
            "--branches",
            "--remotes",
            "--tags",
            "HEAD",
            "--",
        ]
    );
    assert_eq!(filter.commit_hash(), None);
}

#[test]
fn a_hash_goes_to_the_commit_instead_of_grepping() {
    for hash in [
        "abc1234",
        "ABCDEF0123",
        "0123456789abcdef0123456789abcdef01234567",
    ] {
        let filter = LogFilter {
            text: Some(format!(" {hash} ")),
            ..Default::default()
        };
        assert_eq!(filter.commit_hash(), Some(hash), "{hash}");
        assert!(!args(&filter).iter().any(|arg| arg.starts_with("--grep")));
        assert_eq!(
            filter.log_source().expect("valid"),
            None,
            "a hash alone does not filter the graph"
        );
        assert!(!filter.is_empty(), "but it is still something the user set");
    }
    for not_hash in ["abc123", "abc1234g", "deadbeef cafe"] {
        let filter = LogFilter {
            text: Some(not_hash.into()),
            ..Default::default()
        };
        assert_eq!(filter.commit_hash(), None, "{not_hash}");
    }
}

#[test]
fn a_hash_like_text_that_is_not_a_commit_greps_the_messages() {
    let filter = LogFilter {
        text: Some("20261013".into()),
        text_is_hash: Some(false),
        ..Default::default()
    };
    assert_eq!(filter.commit_hash(), None);
    assert!(args(&filter).contains(&"--grep=20261013".to_string()));
    assert!(filter.log_source().expect("valid").is_some(), "it filters");
}

#[test]
fn paths_rewrite_parents_and_follow_the_separator() {
    let filter = LogFilter {
        paths: vec![path("src/main.rs"), path("-odd dir/x")],
        ..Default::default()
    };
    assert_eq!(
        args(&filter),
        [
            "--ignore-missing",
            "--parents",
            "--branches",
            "--remotes",
            "--tags",
            "HEAD",
            "--",
            "src/main.rs",
            "-odd dir/x",
        ]
    );
}

#[test]
fn every_field_combined() {
    let filter = LogFilter {
        branches: vec!["main".into()],
        authors: vec!["alice".into()],
        since: Some(DateBound::Day(date!(2026 - 01 - 01))),
        until: Some(DateBound::Day(date!(2026 - 12 - 31))),
        paths: vec![path("b.txt")],
        text: Some("fix".into()),
        text_is_hash: None,
    };
    assert_eq!(
        args(&filter),
        [
            "--ignore-missing",
            "--fixed-strings",
            "--regexp-ignore-case",
            "--author=alice",
            "--since=2026-01-01T00:00:00",
            "--until=2026-12-31T23:59:59",
            "--all-match",
            "--grep=fix",
            "--parents",
            "main",
            "--",
            "b.txt",
        ]
    );
    let source = filter.log_source().expect("valid").expect("filtered");
    let LogSource::Args(source_args) = source else {
        panic!("expected LogSource::Args");
    };
    assert_eq!(
        source_args
            .iter()
            .map(|arg| arg.to_string())
            .collect::<Vec<_>>(),
        args(&filter)
    );
}

#[test]
fn a_hash_with_other_filters_keeps_the_others() {
    let filter = LogFilter {
        authors: vec!["alice".into()],
        text: Some("abc1234".into()),
        ..Default::default()
    };
    assert_eq!(filter.commit_hash(), Some("abc1234"));
    assert!(matches!(
        filter.log_source().expect("valid"),
        Some(LogSource::Args(_))
    ));
}

#[test]
fn ref_names_from_the_snapshot() {
    use git::repository::Branch;
    let branch = |name: &str| Branch {
        is_head: false,
        ref_name: name.to_string().into(),
        upstream: None,
        most_recent_commit: None,
    };
    let names = branch_choices(&[
        branch("refs/heads/main"),
        branch("refs/remotes/origin/HEAD"),
        branch("refs/remotes/origin/main"),
        branch("refs/heads/feature"),
    ]);
    assert_eq!(names, ["feature", "main", "origin/main"]);
}
