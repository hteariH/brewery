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
        let output = smol::block_on(
            util::command::new_command("git")
                .current_dir(&self.work)
                .envs(&self.env)
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
