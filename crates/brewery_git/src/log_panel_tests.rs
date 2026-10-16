use super::*;
use fs::RealFs;
use gpui::{TestAppContext, VisualTestContext};
use project::FakeFs;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use util::path;
use workspace::{AppState, MultiWorkspace};
use worktree::WorktreeModelHandle;

struct TestRepo {
    _dir: TempDir,
    work: PathBuf,
    env: HashMap<String, String>,
}

impl TestRepo {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let work = dir.path().join("repo");
        std::fs::create_dir(&work).expect("repo dir");
        let global = dir.path().join("gitconfig");
        std::fs::write(&global, "").expect("global config");
        let env = HashMap::from([
            (
                "GIT_CONFIG_GLOBAL".to_string(),
                global.to_string_lossy().into_owned(),
            ),
            ("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string()),
        ]);
        let repo = Self {
            _dir: dir,
            work,
            env,
        };
        repo.git(&["init", "--quiet"]);
        repo.git(&["symbolic-ref", "HEAD", "refs/heads/main"]);
        for (key, value) in [
            ("core.autocrlf", "false"),
            ("user.name", "Brewery Test"),
            ("user.email", "test@brewery.dev"),
            ("commit.gpgsign", "false"),
        ] {
            repo.git(&["config", key, value]);
        }
        repo
    }

    fn git(&self, args: &[&str]) -> String {
        self.git_with_env(args, &[])
    }

    fn git_with_env(&self, args: &[&str], extra_env: &[(&str, &str)]) -> String {
        let output = smol::block_on(
            util::command::new_command("git")
                .current_dir(&self.work)
                .envs(&self.env)
                .envs(extra_env.iter().copied())
                .args(args)
                .output(),
        )
        .expect("spawn git");
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Commits a change and returns the new commit's hash.
    fn commit(&self, path: &str, content: &str, message: &str) -> String {
        std::fs::write(self.work.join(path), content).expect("write");
        self.git(&["add", "--", path]);
        self.git(&["commit", "--quiet", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }

    fn with_three_commits() -> (Self, Vec<String>) {
        let repo = Self::new();
        let shas = vec![
            repo.commit("a.txt", "one\n", "first"),
            repo.commit("a.txt", "two\n", "second"),
            repo.commit("b.txt", "three\n", "third"),
        ];
        (repo, shas)
    }
}

struct Fixture {
    workspace: Entity<Workspace>,
    panel: Entity<LogPanel>,
}

fn init_test(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let _ = AppState::test(cx);
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        release_channel::init(semver::Version::new(0, 0, 0), cx);
        editor::init(cx);
        super::init(cx);
    });
}

async fn workspace_with_panel(
    project: Entity<Project>,
    cx: &mut TestAppContext,
) -> (Fixture, &mut VisualTestContext) {
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
    let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());
    let panel = workspace.update_in(cx, |workspace, window, cx| {
        let weak = workspace.weak_handle();
        let project = workspace.project().clone();
        let panel = cx.new(|cx| LogPanel::new(weak, project, window, cx));
        workspace.add_panel(panel.clone(), window, cx);
        panel
    });
    cx.run_until_parked();
    (Fixture { workspace, panel }, cx)
}

async fn open_real<'a>(
    repo: &TestRepo,
    cx: &'a mut TestAppContext,
) -> (Fixture, &'a mut VisualTestContext) {
    open_real_at(repo, &repo.work, cx).await
}

/// A project of `root`, a folder in `repo`.
async fn open_real_at<'a>(
    repo: &TestRepo,
    root: &std::path::Path,
    cx: &'a mut TestAppContext,
) -> (Fixture, &'a mut VisualTestContext) {
    cx.executor().allow_parking();
    crate::cli::set_test_env(repo.env.clone());
    let project = Project::test(RealFs::new(None, cx.executor()), [root], cx).await;
    let tree = project.read_with(cx, |project, cx| {
        project.worktrees(cx).next().expect("worktree")
    });
    tree.flush_fs_events(cx).await;
    project
        .update(cx, |project, cx| project.git_scans_complete(cx))
        .await;
    cx.executor().run_until_parked();
    workspace_with_panel(project, cx).await
}

/// Polls (real time: git runs as real processes) until `done` holds.
fn wait_until(
    cx: &mut VisualTestContext,
    what: &str,
    mut done: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(25));
        cx.executor().advance_clock(Duration::from_millis(200));
    }
}

fn log_graph(fixture: &Fixture, cx: &VisualTestContext) -> Option<Entity<GitGraph>> {
    fixture
        .panel
        .read_with(cx, |panel, _| panel.log_graph().cloned())
}

fn wait_for_commits(fixture: &Fixture, count: usize, cx: &mut VisualTestContext) {
    wait_until(cx, "the graph to load", |cx| {
        let Some(graph) = log_graph(fixture, cx) else {
            return false;
        };
        graph.update(cx, |graph, cx| {
            graph.commit_count_and_loading_state_for_test(cx)
        }) == (count, false)
    });
}

fn selected_sha(fixture: &Fixture, cx: &VisualTestContext) -> Option<String> {
    let graph = log_graph(fixture, cx)?;
    graph.read_with(cx, |graph, _| {
        graph.selected_commit_sha().map(|oid| oid.to_string())
    })
}

fn panel_has_focus(fixture: &Fixture, cx: &mut VisualTestContext) -> bool {
    let panel = fixture.panel.clone();
    cx.update(|window, cx| panel.focus_handle(cx).contains_focused(window, cx))
}

#[gpui::test]
async fn registers_in_the_bottom_dock_as_log(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree(path!("/dir"), json!({ "file.txt": "text" }))
        .await;
    let project = Project::test(fs, [path!("/dir").as_ref()], cx).await;
    let (fixture, cx) = workspace_with_panel(project, cx).await;

    assert_eq!(LogPanel::persistent_name(), "Log Panel");
    fixture.panel.update_in(cx, |panel, window, cx| {
        assert_eq!(panel.position(window, cx), DockPosition::Bottom);
        assert_eq!(panel.icon(window, cx), Some(IconName::GitGraph));
        assert_eq!(panel.icon_tooltip(window, cx), Some("Log"));
        assert_eq!(panel.toggle_action().name(), "brewery::ToggleLogPanel");
    });
    fixture.workspace.read_with(cx, |workspace, cx| {
        let dock = workspace.bottom_dock().read(cx);
        assert!(
            dock.panel::<LogPanel>().is_some(),
            "added to the bottom dock"
        );
    });
}

#[gpui::test]
async fn toggle_opens_focuses_and_shows_the_commits(cx: &mut TestAppContext) {
    init_test(cx);
    let (repo, shas) = TestRepo::with_three_commits();
    let (fixture, cx) = open_real(&repo, cx).await;
    assert!(
        log_graph(&fixture, cx).is_none(),
        "no git log before the panel is shown"
    );

    cx.dispatch_action(ToggleLogPanel);
    wait_for_commits(&fixture, 3, cx);

    fixture.workspace.read_with(cx, |workspace, cx| {
        let dock = workspace.bottom_dock().read(cx);
        assert!(dock.is_open(), "the bottom dock should be open");
        let visible = dock.visible_panel().expect("a panel should be visible");
        assert_eq!(visible.panel_id(), fixture.panel.entity_id());
        assert_eq!(
            workspace.items_of_type::<GitGraph>(cx).count(),
            0,
            "the graph is not a pane item"
        );
    });
    assert!(panel_has_focus(&fixture, cx), "focus moves into the panel");

    // The graph's own key context is on the focus path: its actions work inside the panel.
    assert_eq!(selected_sha(&fixture, cx), None);
    cx.dispatch_action(git_ui::git_graph::ScrollDown);
    cx.run_until_parked();
    let selected = selected_sha(&fixture, cx).expect("ScrollDown selects a commit");
    assert!(shas.contains(&selected));

    // Toggling again leaves the panel; the graph stays alive while hidden.
    let graph = log_graph(&fixture, cx).expect("graph");
    cx.dispatch_action(ToggleLogPanel);
    cx.run_until_parked();
    assert!(!panel_has_focus(&fixture, cx));
    fixture.workspace.update_in(cx, |workspace, window, cx| {
        workspace.close_panel::<LogPanel>(window, cx)
    });
    cx.run_until_parked();
    cx.dispatch_action(ToggleLogPanel);
    cx.run_until_parked();
    assert_eq!(
        log_graph(&fixture, cx).map(|g| g.entity_id()),
        Some(graph.entity_id())
    );
}

#[gpui::test]
async fn show_commit_in_log_selects_the_commit(cx: &mut TestAppContext) {
    init_test(cx);
    let (repo, shas) = TestRepo::with_three_commits();
    let (fixture, cx) = open_real(&repo, cx).await;

    // Asked before the graph has loaded: selected once the commit arrives.
    cx.dispatch_action(ShowCommitInLog {
        sha: shas[1].clone(),
    });
    wait_for_commits(&fixture, 3, cx);
    wait_until(cx, "the commit to be selected", |cx| {
        selected_sha(&fixture, cx) == Some(shas[1].clone())
    });
    assert!(panel_has_focus(&fixture, cx));

    // And on a loaded graph.
    fixture.panel.update_in(cx, |panel, window, cx| {
        panel.select_commit(&shas[2], window, cx)
    });
    cx.run_until_parked();
    assert_eq!(selected_sha(&fixture, cx), Some(shas[2].clone()));
}

#[gpui::test]
async fn without_a_repository_shows_the_empty_state(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree(path!("/dir"), json!({ "file.txt": "text" }))
        .await;
    let project = Project::test(fs, [path!("/dir").as_ref()], cx).await;
    let (fixture, cx) = workspace_with_panel(project, cx).await;

    cx.dispatch_action(ToggleLogPanel);
    cx.run_until_parked();
    assert!(log_graph(&fixture, cx).is_none());
    assert!(panel_has_focus(&fixture, cx));
    assert_eq!(NO_REPOSITORY, "No Git repository");
}

#[gpui::test]
async fn follows_the_active_repository(cx: &mut TestAppContext) {
    init_test(cx);
    let fs = FakeFs::new(cx.executor());
    fs.insert_tree(
        path!("/root"),
        json!({
            "one": { ".git": {}, "a.txt": "a" },
            "two": { ".git": {}, "b.txt": "b" },
        }),
    )
    .await;
    let project = Project::test(
        fs,
        [path!("/root/one").as_ref(), path!("/root/two").as_ref()],
        cx,
    )
    .await;
    cx.run_until_parked();
    let worktrees = project.read_with(cx, |project, cx| {
        project
            .worktrees(cx)
            .map(|tree| tree.read(cx).id())
            .collect::<Vec<_>>()
    });
    let (fixture, cx) = workspace_with_panel(project.clone(), cx).await;
    let set_active = |worktree, cx: &mut VisualTestContext| {
        project.update(cx, |project, cx| {
            project.git_store().update(cx, |store, cx| {
                store.set_active_repo_for_worktree(worktree, cx)
            })
        });
        cx.run_until_parked();
    };
    set_active(worktrees[0], cx);

    cx.dispatch_action(ToggleLogPanel);
    cx.run_until_parked();
    let first = log_graph(&fixture, cx).expect("a graph for the first repository");

    // Visible: rebuilt at once, focus kept in the panel.
    set_active(worktrees[1], cx);
    let second = log_graph(&fixture, cx).expect("a graph for the second repository");
    assert_ne!(first.entity_id(), second.entity_id());
    assert!(panel_has_focus(&fixture, cx));

    // Hidden: dropped, rebuilt when shown again.
    fixture.workspace.update_in(cx, |workspace, window, cx| {
        workspace.close_panel::<LogPanel>(window, cx)
    });
    cx.run_until_parked();
    set_active(worktrees[0], cx);
    assert!(log_graph(&fixture, cx).is_none());
    cx.dispatch_action(ToggleLogPanel);
    cx.run_until_parked();
    let third = log_graph(&fixture, cx).expect("rebuilt on show");
    assert_ne!(third.entity_id(), second.entity_id());
}

/// The actions a JetBrains keymap resolves for `keystroke` with the context stack `contexts`.
/// Bindings for actions this test binary does not link are skipped.
fn jetbrains_bindings_for(
    keymap: &str,
    keystroke: &str,
    contexts: &[&str],
    cx: &mut TestAppContext,
) -> Vec<String> {
    cx.update(|cx| {
        let bindings = match settings::KeymapFile::load(keymap, cx) {
            settings::KeymapFileLoadResult::Success { key_bindings }
            | settings::KeymapFileLoadResult::SomeFailedToLoad { key_bindings, .. } => key_bindings,
            settings::KeymapFileLoadResult::JsonParseFailure { error } => {
                panic!("keymap does not parse: {error}")
            }
        };
        let contexts = contexts
            .iter()
            .map(|context| gpui::KeyContext::parse(context).expect("context"))
            .collect::<Vec<_>>();
        gpui::Keymap::new(bindings)
            .bindings_for_input(
                &[gpui::Keystroke::parse(keystroke).expect("keystroke")],
                &contexts,
            )
            .0
            .iter()
            .map(|binding| binding.action().name().to_string())
            .collect()
    })
}

#[gpui::test]
fn alt_9_toggles_the_log_in_the_jetbrains_keymaps(cx: &mut TestAppContext) {
    init_test(cx);
    for (keymap, key) in [
        (
            include_str!("../../../assets/keymaps/linux/jetbrains.json"),
            "alt-9",
        ),
        (
            include_str!("../../../assets/keymaps/macos/jetbrains.json"),
            "cmd-9",
        ),
    ] {
        for outside in [&["Workspace"][..], &["Workspace", "Pane", "Editor"][..]] {
            assert_eq!(
                jetbrains_bindings_for(keymap, key, outside, cx).first(),
                Some(&"brewery::ToggleLogPanel".to_string()),
                "{key} in {outside:?}"
            );
        }
        let inside = ["Workspace", "Dock", "LogPanel", "GitGraph", "Editor"];
        assert_eq!(
            jetbrains_bindings_for(keymap, key, &inside, cx).first(),
            Some(&"workspace::CloseActiveDock".to_string()),
            "{key} inside the Log panel"
        );
    }
}

// Filters.

use crate::log_filter::{DateBound, LogFilter};
use crate::log_filter_bar::{ChoiceKind, LogFilterBar, Person};
use git::repository::{LogOrder, LogSource, RepoPath};
use picker::PickerDelegate as _;
use project::git_store::Repository;
use std::collections::BTreeSet;

/// main: c1 (Alice, a.txt) - c2 (Bob, b.txt) - c4 (Alice, b.txt) - c5 merge (Alice)
/// feature (from c2): c3 (Bob, a.txt), merged by c5.
struct History {
    repo: TestRepo,
    c: [String; 5],
}

impl History {
    fn new() -> Self {
        let repo = TestRepo::new();
        let commit = |author: &str, date: &str, path: &str, content: &str, message: &str| {
            std::fs::write(repo.work.join(path), content).expect("write");
            repo.git(&["add", "--", path]);
            let email = format!("{}@example.com", author.to_lowercase());
            let env = [
                ("GIT_AUTHOR_NAME", author),
                ("GIT_AUTHOR_EMAIL", email.as_str()),
                ("GIT_AUTHOR_DATE", date),
                ("GIT_COMMITTER_DATE", date),
            ];
            repo.git_with_env(&["commit", "--quiet", "-m", message], &env);
            repo.git(&["rev-parse", "HEAD"])
        };
        let c1 = commit(
            "Alice",
            "2026-01-01T12:00:00+00:00",
            "a.txt",
            "1\n",
            "fix one",
        );
        let c2 = commit(
            "Bob",
            "2026-02-01T12:00:00+00:00",
            "b.txt",
            "1\n",
            "feat two",
        );
        repo.git(&["checkout", "--quiet", "-b", "feature"]);
        let c3 = commit(
            "Bob",
            "2026-02-15T12:00:00+00:00",
            "a.txt",
            "2\n",
            "feat three",
        );
        repo.git(&["checkout", "--quiet", "main"]);
        let c4 = commit(
            "Alice",
            "2026-03-01T12:00:00+00:00",
            "b.txt",
            "2\n",
            "fix four",
        );
        let date = "2026-03-10T12:00:00+00:00";
        repo.git_with_env(
            &[
                "merge",
                "--quiet",
                "--no-ff",
                "-m",
                "merge feature",
                "feature",
            ],
            &[
                ("GIT_AUTHOR_NAME", "Alice"),
                ("GIT_AUTHOR_EMAIL", "alice@example.com"),
                ("GIT_AUTHOR_DATE", date),
                ("GIT_COMMITTER_DATE", date),
            ],
        );
        let c5 = repo.git(&["rev-parse", "HEAD"]);
        Self {
            repo,
            c: [c1, c2, c3, c4, c5],
        }
    }

    /// The hashes of commits `cN` (1-based).
    fn shas(&self, ix: &[usize]) -> BTreeSet<String> {
        ix.iter().map(|ix| self.c[*ix - 1].clone()).collect()
    }
}

fn bar(fixture: &Fixture, cx: &VisualTestContext) -> Entity<LogFilterBar> {
    fixture
        .panel
        .read_with(cx, |panel, _| panel.filter_bar().clone())
}

fn apply(fixture: &Fixture, filter: LogFilter, cx: &mut VisualTestContext) {
    bar(fixture, cx).update_in(cx, |bar, window, cx| bar.set_filter(filter, window, cx));
    cx.run_until_parked();
}

/// The commits the graph has laid out (lanes assigned), once it has fully loaded.
fn wait_for_graph(fixture: &Fixture, cx: &mut VisualTestContext) -> BTreeSet<String> {
    wait_until(cx, "the graph to load", |cx| {
        let Some(graph) = log_graph(fixture, cx) else {
            return false;
        };
        !graph
            .update(cx, |graph, cx| {
                graph.commit_count_and_loading_state_for_test(cx)
            })
            .1
    });
    let graph = log_graph(fixture, cx).expect("graph");
    graph.read_with(cx, |graph, _| {
        graph
            .initial_commit_data_for_test()
            .iter()
            .map(|commit| commit.sha.to_string())
            .collect()
    })
}

fn panel_source(fixture: &Fixture, cx: &VisualTestContext) -> Option<LogSource> {
    fixture
        .panel
        .read_with(cx, |panel, _| panel.log_source().cloned())
}

fn repository(fixture: &Fixture, cx: &VisualTestContext) -> Entity<Repository> {
    fixture.workspace.read_with(cx, |workspace, cx| {
        workspace
            .project()
            .read(cx)
            .active_repository(cx)
            .expect("repository")
    })
}

fn is_cached(fixture: &Fixture, source: &LogSource, cx: &VisualTestContext) -> bool {
    repository(fixture, cx).read_with(cx, |repo, _| {
        repo.get_graph_data(source.clone(), LogOrder::default())
            .is_some()
    })
}

async fn open_history<'a>(
    history: &History,
    cx: &'a mut TestAppContext,
) -> (Fixture, &'a mut VisualTestContext) {
    let (fixture, cx) = open_real(&history.repo, cx).await;
    cx.dispatch_action(ToggleLogPanel);
    assert_eq!(wait_for_graph(&fixture, cx), history.shas(&[1, 2, 3, 4, 5]));
    (fixture, cx)
}

#[gpui::test]
async fn each_filter_yields_only_the_matching_commits(cx: &mut TestAppContext) {
    init_test(cx);
    let history = History::new();
    let (fixture, cx) = open_history(&history, cx).await;
    let day = DateBound::parse_day;
    let b_txt = || vec![RepoPath::new("b.txt").expect("path")];
    let cases: Vec<(&str, LogFilter, &[usize])> = vec![
        (
            "author, case-insensitive substring",
            LogFilter {
                authors: vec!["bob".into()],
                ..Default::default()
            },
            &[2, 3],
        ),
        (
            "author with a merge whose second parent is filtered out",
            LogFilter {
                authors: vec!["ALICE@example".into()],
                ..Default::default()
            },
            &[1, 4, 5],
        ),
        (
            "two authors",
            LogFilter {
                authors: vec!["Alice".into(), "Bob".into()],
                ..Default::default()
            },
            &[1, 2, 3, 4, 5],
        ),
        (
            "branch",
            LogFilter {
                branches: vec!["feature".into()],
                ..Default::default()
            },
            &[1, 2, 3],
        ),
        (
            "since",
            LogFilter {
                since: day("2026-02-10"),
                ..Default::default()
            },
            &[3, 4, 5],
        ),
        (
            "until",
            LogFilter {
                until: day("2026-02-01"),
                ..Default::default()
            },
            &[1, 2],
        ),
        (
            "path",
            LogFilter {
                paths: b_txt(),
                ..Default::default()
            },
            &[2, 4],
        ),
        (
            "text",
            LogFilter {
                text: Some("FIX".into()),
                ..Default::default()
            },
            &[1, 4],
        ),
        (
            "branch, author and dates",
            LogFilter {
                branches: vec!["main".into()],
                authors: vec!["alice".into()],
                since: day("2026-02-01"),
                until: day("2026-03-05"),
                ..Default::default()
            },
            &[4],
        ),
    ];
    for (what, filter, expected) in cases {
        apply(&fixture, filter.clone(), cx);
        assert_eq!(
            panel_source(&fixture, cx),
            filter.log_source().expect("valid"),
            "{what}: the graph is rebuilt with the filtered source"
        );
        assert_eq!(
            wait_for_graph(&fixture, cx),
            history.shas(expected),
            "{what}"
        );
    }

    // `--parents` rewrites parents through the path filter: c4's parent is c2, as git
    // simplifies the history of b.txt.
    apply(
        &fixture,
        LogFilter {
            paths: b_txt(),
            ..Default::default()
        },
        cx,
    );
    wait_for_graph(&fixture, cx);
    let graph = log_graph(&fixture, cx).expect("graph");
    let c4 = graph.read_with(cx, |graph, _| {
        graph
            .initial_commit_data_for_test()
            .into_iter()
            .find(|commit| commit.sha.to_string() == history.c[3])
            .expect("c4")
    });
    assert_eq!(
        c4.parents
            .iter()
            .map(|parent| parent.to_string())
            .collect::<Vec<_>>(),
        vec![history.c[1].clone()]
    );
}

#[gpui::test]
async fn the_graph_search_works_inside_a_text_filter(cx: &mut TestAppContext) {
    init_test(cx);
    let history = History::new();
    let (fixture, cx) = open_history(&history, cx).await;
    apply(
        &fixture,
        LogFilter {
            text: Some("fix".into()),
            ..Default::default()
        },
        cx,
    );
    assert_eq!(wait_for_graph(&fixture, cx), history.shas(&[1, 4]));
    let graph = log_graph(&fixture, cx).expect("graph");
    graph.update(cx, |graph, cx| graph.search_for_test("four".into(), cx));
    wait_until(cx, "search results", |cx| {
        graph.read_with(cx, |graph, _| !graph.search_matches_for_test().is_empty())
    });
    // Let a wrong second match arrive, if git sent one.
    cx.run_until_parked();
    let matches = graph.read_with(cx, |graph, _| graph.search_matches_for_test());
    assert_eq!(
        matches
            .iter()
            .map(|oid| oid.to_string())
            .collect::<Vec<_>>(),
        vec![history.c[3].clone()],
        "both the filter text and the query must match"
    );
}

#[gpui::test]
async fn user_filter_shows_the_count_and_reset_restores_the_log(cx: &mut TestAppContext) {
    init_test(cx);
    let history = History::new();
    let (fixture, cx) = open_history(&history, cx).await;
    let unfiltered = log_graph(&fixture, cx).expect("graph");
    let bar = bar(&fixture, cx);
    assert_eq!(bar.read_with(cx, |bar, _| bar.status_text()), None);
    wait_until(cx, "the User choices", |cx| {
        bar.read_with(cx, |bar, _| bar.me().is_some() && !bar.authors().is_empty())
    });
    bar.read_with(cx, |bar, _| {
        let me = bar.me().expect("me");
        assert_eq!(
            (me.name.as_str(), me.email.as_str()),
            ("Brewery Test", "test@brewery.dev"),
            "me = user.name and user.email"
        );
        assert_eq!(
            bar.authors().iter().map(Person::ident).collect::<Vec<_>>(),
            ["Alice <alice@example.com>", "Bob <bob@example.com>"]
        );
    });

    // User = Bob, picked in the User list.
    let picker = bar.update_in(cx, |bar, window, cx| {
        bar.choice_picker(ChoiceKind::User, window, cx)
    });
    picker.update_in(cx, |picker, window, cx| picker.set_query("bob", window, cx));
    cx.run_until_parked();
    picker.update_in(cx, |picker, window, cx| {
        // Bob, then the typed text as a substring choice.
        assert_eq!(picker.delegate.match_count(), 2);
        picker.delegate.set_selected_index(0, window, cx);
        picker.delegate.confirm(false, window, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        bar.read_with(cx, |bar, _| bar.filter().authors.clone()),
        vec!["<bob@example.com>".to_string()],
        "a picked person filters by exact email"
    );
    let bob = panel_source(&fixture, cx).expect("filtered");
    assert_eq!(wait_for_graph(&fixture, cx), history.shas(&[2, 3]));
    let filtered = log_graph(&fixture, cx).expect("graph");
    assert_ne!(filtered.entity_id(), unfiltered.entity_id(), "re-created");
    wait_until(cx, "the count", |cx| {
        bar.read_with(cx, |bar, _| bar.status_text()) == Some("Filtered · 2 commits".into())
    });

    // Another user: the Bob graph's data is dropped, the unfiltered one is kept.
    bar.update(cx, |bar, cx| {
        bar.toggle_author("<bob@example.com>", cx);
        bar.toggle_author("Alice", cx);
    });
    cx.run_until_parked();
    let alice = panel_source(&fixture, cx).expect("filtered");
    assert_eq!(wait_for_graph(&fixture, cx), history.shas(&[1, 4, 5]));
    assert!(
        !is_cached(&fixture, &bob, cx),
        "the replaced filter is forgotten"
    );
    assert!(is_cached(&fixture, &alice, cx));
    assert!(is_cached(&fixture, &LogSource::All, cx));

    // Reset: back to the unfiltered log, no chip, the filtered data forgotten.
    bar.update_in(cx, |bar, window, cx| bar.reset(window, cx));
    cx.run_until_parked();
    assert_eq!(panel_source(&fixture, cx), None);
    assert_eq!(wait_for_graph(&fixture, cx), history.shas(&[1, 2, 3, 4, 5]));
    assert_eq!(bar.read_with(cx, |bar, _| bar.status_text()), None);
    assert!(!is_cached(&fixture, &alice, cx));
}

#[gpui::test]
async fn a_hash_goes_to_the_commit_without_filtering(cx: &mut TestAppContext) {
    init_test(cx);
    let history = History::new();
    let (fixture, cx) = open_history(&history, cx).await;
    let graph = log_graph(&fixture, cx).expect("graph");
    apply(
        &fixture,
        LogFilter {
            text: Some(history.c[1][..9].to_uppercase()),
            ..Default::default()
        },
        cx,
    );
    wait_until(cx, "the commit to be selected", |cx| {
        selected_sha(&fixture, cx) == Some(history.c[1].clone())
    });
    assert_eq!(panel_source(&fixture, cx), None, "not filtered");
    assert_eq!(
        log_graph(&fixture, cx).map(|g| g.entity_id()),
        Some(graph.entity_id()),
        "the same graph"
    );

    // Hex text that is not a commit (a date, a ticket number) searches the messages instead.
    let bar = bar(&fixture, cx);
    bar.update(cx, |bar, cx| bar.set_text("0000000", cx));
    wait_until(cx, "the message search", |cx| {
        panel_source(&fixture, cx).is_some()
    });
    let LogSource::Args(args) = panel_source(&fixture, cx).expect("filtered") else {
        panic!("a filtered source");
    };
    assert!(args.iter().any(|arg| arg == "--grep=0000000"), "{args:?}");
    assert_eq!(wait_for_graph(&fixture, cx), BTreeSet::new());
    assert_eq!(bar.read_with(cx, |bar, _| bar.error().cloned()), None);

    // A new text is a hash again until looked up.
    bar.update(cx, |bar, cx| bar.set_text(&history.c[2][..9], cx));
    wait_until(cx, "the commit to be selected", |cx| {
        selected_sha(&fixture, cx) == Some(history.c[2].clone())
    });
    assert_eq!(panel_source(&fixture, cx), None, "not filtered");
}

#[gpui::test]
async fn showing_a_commit_the_filter_hides_resets_the_filter(cx: &mut TestAppContext) {
    init_test(cx);
    let history = History::new();
    let (fixture, cx) = open_history(&history, cx).await;
    let alice = LogFilter {
        authors: vec!["Alice".into()],
        ..Default::default()
    };

    // Asked while the filtered log is loading.
    apply(&fixture, alice.clone(), cx);
    fixture.panel.update_in(cx, |panel, window, cx| {
        panel.select_commit(&history.c[1], window, cx)
    });
    wait_until(cx, "the commit to be selected", |cx| {
        selected_sha(&fixture, cx) == Some(history.c[1].clone())
    });
    assert_eq!(panel_source(&fixture, cx), None, "the filter was reset");

    // Asked on a loaded filtered log.
    apply(&fixture, alice, cx);
    assert_eq!(wait_for_graph(&fixture, cx), history.shas(&[1, 4, 5]));
    fixture.panel.update_in(cx, |panel, window, cx| {
        panel.select_commit(&history.c[2], window, cx)
    });
    wait_until(cx, "the commit to be selected", |cx| {
        selected_sha(&fixture, cx) == Some(history.c[2].clone())
    });
    assert_eq!(panel_source(&fixture, cx), None, "the filter was reset");

    // A commit the filter shows keeps it.
    let bob = LogFilter {
        authors: vec!["Bob".into()],
        ..Default::default()
    };
    apply(&fixture, bob, cx);
    fixture.panel.update_in(cx, |panel, window, cx| {
        panel.select_commit(&history.c[2], window, cx)
    });
    wait_until(cx, "the commit to be selected", |cx| {
        selected_sha(&fixture, cx) == Some(history.c[2].clone())
    });
    cx.run_until_parked();
    assert!(panel_source(&fixture, cx).is_some(), "still filtered");
}

#[gpui::test]
async fn a_bad_branch_or_disabled_filters_leave_the_log_unfiltered(cx: &mut TestAppContext) {
    init_test(cx);
    let history = History::new();
    let (fixture, cx) = open_history(&history, cx).await;
    apply(
        &fixture,
        LogFilter {
            branches: vec!["--all".into()],
            ..Default::default()
        },
        cx,
    );
    assert_eq!(panel_source(&fixture, cx), None);
    let bar = bar(&fixture, cx);
    assert_eq!(
        bar.read_with(cx, |bar, _| bar.error().cloned()),
        Some("not a branch name: --all".into())
    );

    // Disabled, as in remote projects: filters are ignored.
    bar.update(cx, |bar, cx| bar.set_enabled(false, cx));
    apply(
        &fixture,
        LogFilter {
            authors: vec!["bob".into()],
            ..Default::default()
        },
        cx,
    );
    assert_eq!(panel_source(&fixture, cx), None);
}

fn has_commit_graph(repo: &TestRepo) -> bool {
    let info = repo.work.join(".git").join("objects").join("info");
    info.join("commit-graph").exists() || info.join("commit-graphs").exists()
}

fn notice(fixture: &Fixture, cx: &VisualTestContext) -> Option<CommitGraphNotice> {
    fixture
        .panel
        .read_with(cx, |panel, _| panel.commit_graph_notice().cloned())
}

fn commit_graph_checked(fixture: &Fixture, cx: &VisualTestContext) -> bool {
    fixture
        .panel
        .read_with(cx, |panel, _| !panel.commit_graph_checked.is_empty())
}

#[gpui::test]
async fn a_slow_first_chunk_without_a_commit_graph_offers_to_write_one(cx: &mut TestAppContext) {
    init_test(cx);
    let (repo, _) = TestRepo::with_three_commits();
    assert!(!has_commit_graph(&repo));
    let (fixture, cx) = open_real(&repo, cx).await;
    // Any first chunk counts as slow here.
    fixture
        .panel
        .update(cx, |panel, _| panel.slow_first_chunk = Duration::ZERO);

    cx.dispatch_action(ToggleLogPanel);
    wait_for_commits(&fixture, 3, cx);
    wait_until(cx, "the commit-graph notice", |cx| {
        notice(&fixture, cx) == Some(CommitGraphNotice::Offered)
    });

    fixture
        .panel
        .update_in(cx, |panel, window, cx| panel.write_commit_graph(window, cx));
    assert_eq!(notice(&fixture, cx), Some(CommitGraphNotice::Writing));
    wait_until(cx, "the commit-graph to be written", |cx| {
        notice(&fixture, cx).is_none()
    });
    assert!(
        has_commit_graph(&repo),
        "git commit-graph write --reachable ran"
    );

    // One time only: a new graph (here a filtered one) does not offer it again.
    let filter = LogFilter {
        authors: vec!["Brewery".into()],
        ..LogFilter::default()
    };
    apply(&fixture, filter, cx);
    wait_for_commits(&fixture, 3, cx);
    for _ in 0..10 {
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(200));
    }
    assert_eq!(notice(&fixture, cx), None);
}

#[gpui::test]
async fn the_commit_graph_notice_can_be_dismissed(cx: &mut TestAppContext) {
    init_test(cx);
    let (repo, _) = TestRepo::with_three_commits();
    let (fixture, cx) = open_real(&repo, cx).await;
    fixture
        .panel
        .update(cx, |panel, _| panel.slow_first_chunk = Duration::ZERO);
    cx.dispatch_action(ToggleLogPanel);
    wait_until(cx, "the commit-graph notice", |cx| {
        notice(&fixture, cx) == Some(CommitGraphNotice::Offered)
    });
    fixture
        .panel
        .update(cx, |panel, cx| panel.dismiss_commit_graph_notice(cx));
    assert_eq!(notice(&fixture, cx), None);
    assert!(!has_commit_graph(&repo));
}

#[gpui::test]
async fn no_commit_graph_notice_when_it_would_not_help(app: &mut TestAppContext) {
    init_test(app);

    // Fast first chunk: not even checked. The threshold is wall-clock time, so a loaded machine
    // could make three commits look slow; an unreachable threshold keeps this case fast.
    let (repo, _) = TestRepo::with_three_commits();
    let (fixture, cx) = open_real(&repo, app).await;
    fixture
        .panel
        .update(cx, |panel, _| panel.slow_first_chunk = Duration::MAX);
    cx.dispatch_action(ToggleLogPanel);
    wait_for_commits(&fixture, 3, cx);
    for _ in 0..10 {
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(200));
    }
    assert!(!commit_graph_checked(&fixture, cx));
    assert_eq!(notice(&fixture, cx), None);
    cx.update(|window, _| window.remove_window());

    // Slow, but a commit-graph file exists, or the user turned commit-graphs off.
    for setup in ["commit-graph", "core.commitGraph=false"] {
        let (repo, _) = TestRepo::with_three_commits();
        match setup {
            "commit-graph" => {
                repo.git(&["commit-graph", "write", "--reachable"]);
                assert!(has_commit_graph(&repo));
            }
            _ => {
                repo.git(&["config", "core.commitGraph", "false"]);
            }
        }
        let (fixture, cx) = open_real(&repo, app).await;
        fixture
            .panel
            .update(cx, |panel, _| panel.slow_first_chunk = Duration::ZERO);
        cx.dispatch_action(ToggleLogPanel);
        wait_for_commits(&fixture, 3, cx);
        wait_until(cx, "the commit-graph check", |cx| {
            commit_graph_checked(&fixture, cx)
        });
        assert_eq!(notice(&fixture, cx), None, "{setup}");
        cx.update(|window, _| window.remove_window());
    }
}

#[path = "log_scale_tests.rs"]
mod scale;

#[gpui::test]
async fn me_and_picked_users_match_exact_people(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.git(&["config", "user.name", "Max"]);
    repo.git(&["config", "user.email", "max@x"]);
    let commit = |name: &str, email: &str, path: &str| {
        std::fs::write(repo.work.join(path), name).expect("write");
        repo.git(&["add", "--", path]);
        repo.git_with_env(
            &["commit", "--quiet", "-m", path],
            &[("GIT_AUTHOR_NAME", name), ("GIT_AUTHOR_EMAIL", email)],
        );
        repo.git(&["rev-parse", "HEAD"])
    };
    let max = commit("Max", "max@x", "a.txt");
    let maxim = commit("Maxim", "maxim@x", "b.txt");
    let (fixture, cx) = open_real(&repo, cx).await;
    cx.dispatch_action(ToggleLogPanel);
    wait_for_graph(&fixture, cx);
    let bar = bar(&fixture, cx);
    wait_until(cx, "the User choices", |cx| {
        bar.read_with(cx, |bar, _| bar.me().is_some() && bar.authors().len() == 2)
    });

    let pick = |query: &str, cx: &mut VisualTestContext| {
        let picker = bar.update_in(cx, |bar, window, cx| {
            bar.choice_picker(ChoiceKind::User, window, cx)
        });
        picker.update_in(cx, |picker, window, cx| picker.set_query(query, window, cx));
        cx.run_until_parked();
        picker.update_in(cx, |picker, window, cx| {
            picker.delegate.set_selected_index(0, window, cx);
            picker.delegate.confirm(false, window, cx);
        });
        cx.run_until_parked();
    };
    let only = |shas: &[&String]| {
        shas.iter()
            .map(|sha| sha.to_string())
            .collect::<BTreeSet<_>>()
    };

    // "me" (Max) is not Maxim, although "Max" is a substring of "Maxim".
    pick("me (", cx);
    assert_eq!(
        bar.read_with(cx, |bar, _| bar.filter().authors.clone()),
        vec!["<max@x>".to_string()]
    );
    assert_eq!(wait_for_graph(&fixture, cx), only(&[&max]));
    assert_eq!(
        bar.read_with(cx, |bar, _| bar.user_label_for_test()),
        "User: me"
    );

    // Maxim picked from the list: Maxim only.
    bar.update_in(cx, |bar, window, cx| bar.reset(window, cx));
    cx.run_until_parked();
    pick("maxim@", cx);
    assert_eq!(wait_for_graph(&fixture, cx), only(&[&maxim]));
    assert_eq!(
        bar.read_with(cx, |bar, _| bar.user_label_for_test()),
        "User: Maxim"
    );

    // Typed text stays a substring: both.
    bar.update_in(cx, |bar, window, cx| bar.reset(window, cx));
    cx.run_until_parked();
    bar.update(cx, |bar, cx| bar.toggle_author("max", cx));
    cx.run_until_parked();
    assert_eq!(wait_for_graph(&fixture, cx), only(&[&max, &maxim]));
}

#[gpui::test]
async fn enter_in_the_text_field_applies_it(cx: &mut TestAppContext) {
    init_test(cx);
    let history = History::new();
    let (fixture, cx) = open_history(&history, cx).await;
    let bar = bar(&fixture, cx);
    let editor = bar.read_with(cx, |bar, _| bar.text_editor_for_test());
    editor.update_in(cx, |editor, window, cx| {
        editor.set_text("fix", window, cx);
        window.focus(&editor.focus_handle(cx), cx);
    });
    cx.run_until_parked();
    cx.dispatch_action(menu::Confirm);
    cx.run_until_parked();
    assert_eq!(
        bar.read_with(cx, |bar, _| bar.filter().text.clone()),
        Some("fix".to_string())
    );
    assert_eq!(wait_for_graph(&fixture, cx), history.shas(&[1, 4]));
}

#[gpui::test]
async fn an_option_like_branch_never_enters_the_filter(cx: &mut TestAppContext) {
    init_test(cx);
    let history = History::new();
    let (fixture, cx) = open_history(&history, cx).await;
    let bar = bar(&fixture, cx);
    bar.update(cx, |bar, cx| bar.toggle_branch("-foo", cx));
    cx.run_until_parked();
    bar.read_with(cx, |bar, _| {
        assert!(bar.filter().is_empty(), "no Reset, no Branch: -foo label");
        assert_eq!(bar.error().cloned(), Some("not a branch name: -foo".into()));
    });
    assert_eq!(panel_source(&fixture, cx), None);
}

#[gpui::test]
async fn paths_are_literal_and_relative_to_a_project_inside_the_repository(
    cx: &mut TestAppContext,
) {
    init_test(cx);
    let repo = TestRepo::new();
    std::fs::create_dir(repo.work.join("sub")).expect("sub dir");
    let c1 = repo.commit("sub/a.txt", "1\n", "a");
    let c2 = repo.commit("sub/[ab].txt", "1\n", "brackets");
    let _c3 = repo.commit("root.txt", "1\n", "root");
    let (fixture, cx) = open_real_at(&repo, &repo.work.join("sub"), cx).await;
    cx.dispatch_action(ToggleLogPanel);
    wait_for_commits(&fixture, 3, cx);
    let bar = bar(&fixture, cx);

    // Typed relative to the project, matched literally: `[ab]` is not a glob.
    let rejected = bar.update(cx, |bar, cx| bar.set_paths_text("[ab].txt", cx));
    assert!(rejected.is_empty());
    assert_eq!(
        bar.read_with(cx, |bar, _| bar.filter().paths.clone()),
        vec![RepoPath::new("sub/[ab].txt").expect("path")]
    );
    cx.run_until_parked();
    assert_eq!(wait_for_graph(&fixture, cx), BTreeSet::from([c2]));
    assert_eq!(
        bar.read_with(cx, |bar, cx| bar.path_texts(cx)),
        vec!["[ab].txt".to_string()],
        "shown as typed"
    );

    // "." is the project folder.
    bar.update(cx, |bar, cx| bar.set_paths_text(".", cx));
    cx.run_until_parked();
    let shas = wait_for_graph(&fixture, cx);
    assert!(shas.contains(&c1) && shas.len() == 2, "{shas:?}");
}
