use super::*;
use crate::revision_diff_view::ResolvedSide;
use editor::Editor;
use fs::RealFs;
use gpui::{TestAppContext, VisualTestContext};
use project::Project;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;
use tempfile::TempDir;
use workspace::{AppState, Item as _, MultiWorkspace};
use worktree::WorktreeModelHandle;

/// A real git repository in a temp dir, isolated from the user's global and system config.
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
        let output = smol::block_on(
            util::command::new_command("git")
                .current_dir(&self.work)
                .envs(&self.env)
                .args(args)
                .output(),
        )
        .expect("spawn git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn write(&self, path: &str, content: &str) {
        std::fs::write(self.work.join(path), content).expect("write file");
    }

    fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.work.join(path)).expect("read file")
    }

    /// Commits a change and returns the new commit's hash.
    fn commit(&self, path: &str, content: &str, message: &str) -> String {
        self.write(path, content);
        self.git(&["add", "--", path]);
        self.git(&["commit", "--quiet", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }
}

struct Fixture {
    workspace: Entity<Workspace>,
    repository: Entity<Repository>,
    panel: Entity<LogPanel>,
}

fn init_test(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let _ = AppState::test(cx);
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        release_channel::init(semver::Version::new(0, 0, 0), cx);
        editor::init(cx);
        // Upstream's handlers and opener, which Brewery's must win over.
        language_model::init(cx);
        git_ui::init(cx);
        git_ui_core::set_file_history_opener(|_, _, _, _| panic!("git_ui's opener ran"), cx);
        crate::log_panel::init(cx);
        super::init(cx);
    });
}

async fn open_real<'a>(
    repo: &TestRepo,
    cx: &'a mut TestAppContext,
) -> (Fixture, &'a mut VisualTestContext) {
    cx.executor().allow_parking();
    crate::cli::set_test_env(repo.env.clone());
    let project = Project::test(RealFs::new(None, cx.executor()), [repo.work.as_path()], cx).await;
    let tree = project.read_with(cx, |project, cx| {
        project.worktrees(cx).next().expect("worktree")
    });
    tree.flush_fs_events(cx).await;
    project
        .update(cx, |project, cx| project.git_scans_complete(cx))
        .await;
    cx.executor().run_until_parked();
    let repository = project.read_with(cx, |project, cx| {
        project
            .repositories(cx)
            .values()
            .next()
            .expect("repository")
            .clone()
    });
    let (multi_workspace, cx) = cx.add_window_view({
        let project = project.clone();
        |window, cx| MultiWorkspace::test_new(project, window, cx)
    });
    let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());
    let panel = workspace.update_in(cx, |workspace, window, cx| {
        let weak = workspace.weak_handle();
        let project = workspace.project().clone();
        let panel = cx.new(|cx| LogPanel::new(weak, project, window, cx));
        workspace.add_panel(panel.clone(), window, cx);
        panel
    });
    cx.run_until_parked();
    (
        Fixture {
            workspace,
            repository,
            panel,
        },
        cx,
    )
}

fn repo_path(name: &str) -> RepoPath {
    RepoPath::new(name).expect("repo path")
}

fn project_path(fixture: &Fixture, name: &str, cx: &VisualTestContext) -> ProjectPath {
    fixture.repository.read_with(cx, |repo, cx| {
        repo.repo_path_to_project_path(&repo_path(name), cx)
            .expect("project path")
    })
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

/// Opens the history of `name` through the opener the Project panel uses.
fn open_history(fixture: &Fixture, name: &str, cx: &mut VisualTestContext) {
    let path = project_path(fixture, name, cx);
    fixture.workspace.update_in(cx, |workspace, window, cx| {
        git_ui_core::open_file_history(workspace, &path, window, cx)
    });
    cx.run_until_parked();
}

fn active_history(fixture: &Fixture, cx: &VisualTestContext) -> Entity<FileHistoryView> {
    fixture
        .panel
        .read_with(cx, |panel, _| panel.active_history().cloned())
        .expect("an active history tab")
}

fn wait_for_commits(history: &Entity<FileHistoryView>, count: usize, cx: &mut VisualTestContext) {
    let graph = history.read_with(cx, |history, _| history.graph().clone());
    wait_until(cx, "the history to load", |cx| {
        graph.update(cx, |graph, cx| {
            graph.commit_count_and_loading_state_for_test(cx)
        }) == (count, false)
    });
}

fn select(history: &Entity<FileHistoryView>, sha: &str, cx: &mut VisualTestContext) {
    let graph = history.read_with(cx, |history, _| history.graph().clone());
    graph.update(cx, |graph, cx| graph.select_commit_by_sha(sha, cx));
    cx.run_until_parked();
    assert_eq!(
        history.read_with(cx, |history, cx| history
            .selected_sha(cx)
            .map(|sha| sha.to_string())),
        Some(sha.to_string())
    );
}

fn center_graphs(fixture: &Fixture, cx: &VisualTestContext) -> usize {
    fixture.workspace.read_with(cx, |workspace, cx| {
        workspace.items_of_type::<GitGraph>(cx).count()
    })
}

fn diff_views(fixture: &Fixture, cx: &VisualTestContext) -> Vec<Entity<RevisionDiffView>> {
    fixture.workspace.read_with(cx, |workspace, cx| {
        workspace.items_of_type::<RevisionDiffView>(cx).collect()
    })
}

fn revision(sha: &str) -> ResolvedSide {
    ResolvedSide::Revision {
        rev: sha.to_string(),
        sha: sha.parse().expect("sha"),
    }
}

#[test]
fn parses_the_rename_following_log() {
    let sha = |c: char| c.to_string().repeat(40);
    let (a, b, c, d) = (sha('a'), sha('b'), sha('c'), sha('d'));
    // Newest first: d modifies, c merges (no status), b renames old.txt -> new.txt, a adds.
    let output = format!(
        "\x01{d}\0{c}\0\nM\0new.txt\0\x01{c}\0{b} {a}\0\x01{b}\0{a}\0\nR087\0old.txt\0new.txt\0\x01{a}\0\0\nA\0old.txt\0"
    );
    let commits = parse_follow_log(&output, &repo_path("new.txt"));
    let summary = commits
        .iter()
        .map(|commit| {
            (
                commit.sha.to_string(),
                commit
                    .parents
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>(),
                commit.path.as_unix_str().to_string(),
                commit.parent_path.as_unix_str().to_string(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        summary,
        vec![
            (d, vec![c.clone()], "new.txt".into(), "new.txt".into()),
            (
                c,
                vec![b.clone(), a.clone()],
                "new.txt".into(),
                "new.txt".into()
            ),
            (b, vec![a.clone()], "new.txt".into(), "old.txt".into()),
            (a, vec![], "old.txt".into(), "old.txt".into()),
        ]
    );
}

#[gpui::test]
async fn history_opens_tabs_in_the_log_panel(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.commit("a.txt", "one\n", "first");
    repo.commit("b.txt", "two\n", "second");
    repo.commit("a.txt", "three\n", "third");
    let (fixture, cx) = open_real(&repo, cx).await;

    open_history(&fixture, "a.txt", cx);
    let a = active_history(&fixture, cx);
    a.read_with(cx, |history, _| {
        assert_eq!(history.log_source(), LogSource::Path(repo_path("a.txt")));
        assert_eq!(history.title(), "History: a.txt");
    });
    wait_for_commits(&a, 2, cx);
    fixture.panel.read_with(cx, |panel, cx| {
        assert_eq!(panel.tab_titles(cx), vec!["Log", "History: a.txt"]);
        assert!(panel.shows_tab_strip());
        // Showing the panel for a history tab does not load the full log.
        assert!(panel.log_graph().is_none());
    });
    let panel_focused = cx.update(|window, cx| {
        fixture
            .panel
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
    });
    assert!(panel_focused, "the panel is focused");
    assert_eq!(
        center_graphs(&fixture, cx),
        0,
        "nothing opens in the center"
    );

    // The same file again reuses its tab; another file opens a second one.
    open_history(&fixture, "a.txt", cx);
    open_history(&fixture, "b.txt", cx);
    let b = active_history(&fixture, cx);
    assert_ne!(a.entity_id(), b.entity_id());
    fixture.panel.read_with(cx, |panel, cx| {
        assert_eq!(
            panel.tab_titles(cx),
            vec!["Log", "History: a.txt", "History: b.txt"]
        );
    });

    // Closing the active tab activates the one before it, down to the Log tab.
    let b_id = b.entity_id();
    drop(b);
    fixture.panel.update_in(cx, |panel, window, cx| {
        panel.close_file_history(b_id, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(active_history(&fixture, cx).entity_id(), a.entity_id());
    let has_b_data = fixture.repository.read_with(cx, |repo, _| {
        repo.get_graph_data(LogSource::Path(repo_path("b.txt")), Default::default())
            .is_some()
    });
    assert!(!has_b_data, "a closed tab's graph data is dropped");
    let a_id = a.entity_id();
    drop(a);
    fixture.panel.update_in(cx, |panel, window, cx| {
        panel.close_file_history(a_id, window, cx)
    });
    cx.run_until_parked();
    fixture.panel.read_with(cx, |panel, cx| {
        assert!(panel.active_history().is_none());
        assert_eq!(panel.tab_titles(cx), vec!["Log"]);
        assert!(!panel.shows_tab_strip());
        assert!(panel.log_graph().is_some(), "the Log tab is shown");
    });
}

#[gpui::test]
async fn file_history_action_from_an_editor_opens_a_tab(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.commit("a.txt", "one\n", "first");
    repo.commit("b.txt", "two\n", "second");
    let (fixture, cx) = open_real(&repo, cx).await;

    for (file, action) in [
        ("a.txt", Box::new(git::FileHistory) as Box<dyn gpui::Action>),
        ("b.txt", Box::new(ShowFileHistory)),
    ] {
        let path = project_path(&fixture, file, cx);
        fixture
            .workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.open_path(path, None, true, window, cx)
            })
            .await
            .expect("open editor");
        cx.run_until_parked();
        cx.update(|window, cx| window.dispatch_action(action, cx));
        cx.run_until_parked();
        let history = active_history(&fixture, cx);
        assert_eq!(
            history.read_with(cx, |history, _| history.path().clone()),
            repo_path(file)
        );
    }
    assert_eq!(
        center_graphs(&fixture, cx),
        0,
        "git_ui's graph did not open"
    );
    fixture.panel.read_with(cx, |panel, cx| {
        assert_eq!(
            panel.tab_titles(cx),
            vec!["Log", "History: a.txt", "History: b.txt"]
        );
    });
}

#[gpui::test]
async fn toolbar_actions_need_a_selection(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    let first = repo.commit("a.txt", "one\n", "first");
    let (fixture, cx) = open_real(&repo, cx).await;
    open_history(&fixture, "a.txt", cx);
    let history = active_history(&fixture, cx);
    wait_for_commits(&history, 1, cx);

    assert!(!history.read_with(cx, |history, cx| history.actions_enabled(cx)));
    let result = history
        .update_in(cx, |history, window, cx| {
            history.compare_with_local(window, cx)
        })
        .await;
    assert!(result.is_err(), "no commit selected");
    assert!(diff_views(&fixture, cx).is_empty());

    select(&history, &first, cx);
    assert!(history.read_with(cx, |history, cx| history.actions_enabled(cx)));
}

#[gpui::test]
async fn compare_with_local_and_show_diff(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    let first = repo.commit("a.txt", "one\n", "first");
    repo.commit("b.txt", "unrelated\n", "unrelated");
    let unrelated = repo.git(&["rev-parse", "HEAD"]);
    let second = repo.commit("a.txt", "two\n", "second");
    repo.write("a.txt", "local\n");
    let (fixture, cx) = open_real(&repo, cx).await;
    open_history(&fixture, "a.txt", cx);
    let history = active_history(&fixture, cx);
    wait_for_commits(&history, 2, cx);

    select(&history, &first, cx);
    history
        .update_in(cx, |history, window, cx| {
            history.compare_with_local(window, cx)
        })
        .await
        .expect("compare with local");
    cx.run_until_parked();
    let views = diff_views(&fixture, cx);
    assert_eq!(views.len(), 1);
    views[0].read_with(cx, |view, _| {
        assert_eq!(view.old_side(), &revision(&first));
        assert_eq!(view.new_side(), &ResolvedSide::WorkingCopy);
        assert_eq!(view.path(), &repo_path("a.txt"));
    });

    // Show Diff: the commit against its first parent (not the file's previous commit).
    select(&history, &second, cx);
    history
        .update_in(cx, |history, window, cx| history.show_diff(window, cx))
        .await
        .expect("show diff");
    cx.run_until_parked();
    let views = diff_views(&fixture, cx);
    assert_eq!(views.len(), 2);
    views[1].read_with(cx, |view, cx| {
        assert_eq!(view.old_side(), &revision(&unrelated));
        assert_eq!(view.new_side(), &revision(&second));
        let buffer = view.new_buffer().expect("a diff").read(cx);
        assert_eq!(buffer.text(), "two\n");
    });

    // A root commit is compared with nothing.
    select(&history, &first, cx);
    history
        .update_in(cx, |history, window, cx| history.show_diff(window, cx))
        .await
        .expect("show diff of the root commit");
    cx.run_until_parked();
    let views = diff_views(&fixture, cx);
    assert_eq!(views.len(), 3);
    views[2].read_with(cx, |view, _| {
        assert_eq!(view.old_side(), &ResolvedSide::Empty);
        assert_eq!(view.new_side(), &revision(&first));
    });
}

#[gpui::test]
async fn actions_follow_renames(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    let first = repo.commit("old.txt", "one\ntwo\nthree\nfour\n", "first");
    repo.git(&["mv", "old.txt", "new.txt"]);
    repo.write("new.txt", "one\ntwo\nthree\nfour\nfive\n");
    repo.git(&["add", "--", "new.txt"]);
    repo.git(&["commit", "--quiet", "-m", "rename"]);
    let renamed = repo.git(&["rev-parse", "HEAD"]);
    let (fixture, cx) = open_real(&repo, cx).await;
    open_history(&fixture, "new.txt", cx);
    let history = active_history(&fixture, cx);
    wait_for_commits(&history, 2, cx);
    let walks = || FOLLOW_LOGS_RUN.with(|runs| runs.get());
    let walks_before = walks();

    select(&history, &first, cx);
    history
        .update_in(cx, |history, window, cx| {
            history.compare_with_local(window, cx)
        })
        .await
        .expect("compare with local");
    assert_eq!(
        walks() - walks_before,
        1,
        "new.txt is not at the first commit"
    );
    select(&history, &renamed, cx);
    history
        .update_in(cx, |history, window, cx| history.show_diff(window, cx))
        .await
        .expect("show diff");
    cx.run_until_parked();
    let views = diff_views(&fixture, cx);
    assert_eq!(views.len(), 2);
    views[0].read_with(cx, |view, cx| {
        assert_eq!(view.old_path(), &repo_path("old.txt"));
        assert_eq!(view.path(), &repo_path("new.txt"));
        assert_eq!(view.old_side(), &revision(&first));
        assert!(
            view.tab_tooltip_text(cx)
                .expect("tooltip")
                .starts_with("old.txt → new.txt\n")
        );
    });
    views[1].read_with(cx, |view, _| {
        assert_eq!(view.old_path(), &repo_path("old.txt"));
        assert_eq!(view.path(), &repo_path("new.txt"));
        assert_eq!(view.old_side(), &revision(&first));
        assert_eq!(view.new_side(), &revision(&renamed));
    });

    // Get across the rename writes the old content to the current path.
    select(&history, &first, cx);
    let get = history.update_in(cx, |history, window, cx| history.get(window, cx));
    cx.run_until_parked();
    cx.simulate_prompt_answer(REPLACE);
    get.await.expect("get");
    assert_eq!(repo.read("new.txt"), "one\ntwo\nthree\nfour\n");
    assert!(!repo.work.join("old.txt").exists());
    assert_eq!(
        walks() - walks_before,
        1,
        "the tab reuses the walk while HEAD stays"
    );

    // A new commit moves HEAD: the walk is read again.
    repo.git(&["commit", "--quiet", "-am", "get the first version"]);
    let head = repo.git(&["rev-parse", "HEAD"]);
    let repository = fixture.repository.clone();
    wait_until(cx, "the new HEAD", |cx| {
        repository.read_with(cx, |repository, _| {
            repository
                .head_commit
                .as_ref()
                .is_some_and(|commit| commit.sha.as_ref() == head)
        })
    });
    // The history reloads with the new commit.
    wait_for_commits(&history, 3, cx);
    select(&history, &first, cx);
    history
        .update_in(cx, |history, window, cx| {
            history.compare_with_local(window, cx)
        })
        .await
        .expect("compare with local");
    assert_eq!(walks() - walks_before, 2);
}

#[gpui::test]
async fn actions_without_a_rename_skip_the_history_walk(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    let first = repo.commit("a.txt", "one\n", "first");
    let second = repo.commit("a.txt", "two\n", "second");
    let (fixture, cx) = open_real(&repo, cx).await;
    open_history(&fixture, "a.txt", cx);
    let history = active_history(&fixture, cx);
    wait_for_commits(&history, 2, cx);
    let walks_before = FOLLOW_LOGS_RUN.with(|runs| runs.get());

    select(&history, &second, cx);
    history
        .update_in(cx, |history, window, cx| history.show_diff(window, cx))
        .await
        .expect("show diff");
    history
        .update_in(cx, |history, window, cx| {
            history.compare_with_local(window, cx)
        })
        .await
        .expect("compare with local");
    // The root commit has no parent to look at.
    select(&history, &first, cx);
    history
        .update_in(cx, |history, window, cx| history.show_diff(window, cx))
        .await
        .expect("show diff of the root commit");
    cx.run_until_parked();
    assert_eq!(FOLLOW_LOGS_RUN.with(|runs| runs.get()), walks_before);
    let views = diff_views(&fixture, cx);
    assert_eq!(views.len(), 3);
    views[0].read_with(cx, |view, _| {
        assert_eq!(view.old_side(), &revision(&first));
        assert_eq!(view.new_side(), &revision(&second));
        assert_eq!(view.old_path(), &repo_path("a.txt"));
    });
}

#[gpui::test]
async fn get_restores_the_revision_after_confirming(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    let first = repo.commit("a.txt", "one\n", "first");
    repo.commit("a.txt", "two\n", "second");
    let (fixture, cx) = open_real(&repo, cx).await;
    open_history(&fixture, "a.txt", cx);
    let history = active_history(&fixture, cx);
    wait_for_commits(&history, 2, cx);
    select(&history, &first, cx);

    // Cancel leaves the file alone.
    let get = history.update_in(cx, |history, window, cx| history.get(window, cx));
    cx.run_until_parked();
    let (message, _) = cx.pending_prompt().expect("a confirmation");
    assert_eq!(
        message,
        format!(
            "Replace the working copy of a.txt with its version from {}?",
            &first[..8]
        )
    );
    cx.simulate_prompt_answer("Cancel");
    get.await.expect("cancelled get");
    assert_eq!(repo.read("a.txt"), "two\n");

    let get = history.update_in(cx, |history, window, cx| history.get(window, cx));
    cx.run_until_parked();
    cx.simulate_prompt_answer(REPLACE);
    get.await.expect("get");
    assert_eq!(repo.read("a.txt"), "one\n");
    // The Git panel's source, the repository status, sees the change.
    let path = repo_path("a.txt");
    wait_until(cx, "the status to show the change", |cx| {
        fixture
            .repository
            .read_with(cx, |repo, _| repo.status_for_path(&path).is_some())
    });
}

async fn get_confirmed(history: &Entity<FileHistoryView>, cx: &mut VisualTestContext) {
    let get = history.update_in(cx, |history, window, cx| history.get(window, cx));
    cx.run_until_parked();
    cx.simulate_prompt_answer(REPLACE);
    get.await.expect("get");
    cx.run_until_parked();
}

#[gpui::test]
async fn get_changes_only_the_working_copy(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    let first = repo.commit("a.txt", "one\n", "first");
    repo.commit("a.txt", "two\n", "second");
    repo.write("a.txt", "staged\n");
    repo.git(&["add", "--", "a.txt"]);
    repo.write("a.txt", "local\n");
    let (fixture, cx) = open_real(&repo, cx).await;
    open_history(&fixture, "a.txt", cx);
    let history = active_history(&fixture, cx);
    wait_for_commits(&history, 2, cx);
    select(&history, &first, cx);

    get_confirmed(&history, cx).await;
    assert_eq!(repo.read("a.txt"), "one\n");
    assert_eq!(
        repo.git(&["show", ":a.txt"]),
        "staged",
        "the index is untouched"
    );
}

#[gpui::test]
async fn get_edits_an_open_buffer(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    let first = repo.commit("a.txt", "one\n", "first");
    repo.commit("a.txt", "two\n", "second");
    let (fixture, cx) = open_real(&repo, cx).await;
    let path = project_path(&fixture, "a.txt", cx);
    let editor = fixture
        .workspace
        .update_in(cx, |workspace, window, cx| {
            workspace.open_path(path, None, true, window, cx)
        })
        .await
        .expect("open editor")
        .downcast::<Editor>()
        .expect("an editor");
    editor.update_in(cx, |editor, window, cx| {
        editor.move_to_end(&editor::actions::MoveToEnd, window, cx);
        editor.insert("unsaved\n", window, cx);
    });
    open_history(&fixture, "a.txt", cx);
    let history = active_history(&fixture, cx);
    wait_for_commits(&history, 2, cx);
    select(&history, &first, cx);

    get_confirmed(&history, cx).await;
    assert_eq!(repo.read("a.txt"), "one\n");
    let buffer = editor.read_with(cx, |editor, cx| {
        editor.buffer().read(cx).as_singleton().expect("a buffer")
    });
    buffer.read_with(cx, |buffer, _| {
        assert_eq!(buffer.text(), "one\n");
        assert!(!buffer.is_dirty(), "saved");
        assert!(!buffer.has_conflict());
    });
    // The replacement is an edit: undo brings the unsaved text back.
    editor.update_in(cx, |editor, window, cx| {
        editor.undo(&editor::actions::Undo, window, cx)
    });
    buffer.read_with(cx, |buffer, _| assert_eq!(buffer.text(), "two\nunsaved\n"));
}

#[gpui::test]
async fn get_across_a_rename_applies_checkout_filters(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.commit(".gitattributes", "*.txt text eol=crlf\n", "attributes");
    let first = repo.commit("old.txt", "one\r\ntwo\r\n", "first");
    repo.git(&["mv", "old.txt", "new.txt"]);
    repo.git(&["commit", "--quiet", "-m", "rename"]);
    repo.write("new.txt", "local\r\n");
    let (fixture, cx) = open_real(&repo, cx).await;
    open_history(&fixture, "new.txt", cx);
    let history = active_history(&fixture, cx);
    wait_for_commits(&history, 2, cx);
    select(&history, &first, cx);

    get_confirmed(&history, cx).await;
    // Stored with LF; checked out with CRLF.
    assert_eq!(repo.read("new.txt"), "one\r\ntwo\r\n");
    assert!(!repo.work.join("old.txt").exists());
    assert_eq!(
        repo.git(&["status", "--porcelain"]),
        "",
        "matches HEAD's checkout"
    );
}

#[gpui::test]
async fn git_panel_marked_files_open_a_tab_each(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.commit("a.txt", "one\n", "first");
    repo.commit("b.txt", "two\n", "second");
    repo.commit("c.txt", "three\n", "third");
    for file in ["a.txt", "b.txt", "c.txt"] {
        repo.write(file, "changed\n");
    }
    let (fixture, cx) = open_real(&repo, cx).await;
    let (weak, async_cx) = fixture.workspace.update_in(cx, |workspace, window, cx| {
        (workspace.weak_handle(), window.to_async(cx))
    });
    let git_panel = GitPanel::load(weak, async_cx).await.expect("git panel");
    fixture.workspace.update_in(cx, |workspace, window, cx| {
        workspace.add_panel(git_panel.clone(), window, cx)
    });
    cx.executor().advance_clock(Duration::from_millis(100));
    cx.run_until_parked();
    fixture.workspace.update_in(cx, |workspace, window, cx| {
        workspace.focus_panel::<GitPanel>(window, cx);
    });
    cx.run_until_parked();
    let c = project_path(&fixture, "c.txt", cx);
    git_panel.update_in(cx, |panel, window, cx| {
        panel.select_entry_by_path(c, window, cx);
        panel.mark_paths_for_test(&[repo_path("a.txt"), repo_path("b.txt")], cx);
    });
    let focused = cx.update(|window, cx| git_panel.focus_handle(cx).contains_focused(window, cx));
    assert!(focused, "the Git panel has focus");
    cx.run_until_parked();

    cx.update(|window, cx| window.dispatch_action(Box::new(git::FileHistory), cx));
    cx.run_until_parked();

    fixture.panel.read_with(cx, |panel, cx| {
        assert_eq!(
            panel.tab_titles(cx),
            vec!["Log", "History: a.txt", "History: b.txt"]
        );
    });
    assert_eq!(
        center_graphs(&fixture, cx),
        0,
        "git_ui's graphs did not open"
    );
}
