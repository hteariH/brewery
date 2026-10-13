use super::*;
use crate::branches_popup::{OutcomeToast, SHOWN_TOASTS, ToastKind};
use crate::merge_view::MergeView;
use fs::RealFs;
use git_ui::git_panel::GitPanel;
use git_ui::project_diff::ProjectDiff;
use gpui::{TestAppContext, VisualTestContext};
use project::Project;
use settings::SettingsStore;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;
use workspace::{AppState, MultiWorkspace};
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
            ("core.safecrlf", "false"),
            ("user.name", "Brewery Test"),
            ("user.email", "test@brewery.dev"),
            ("commit.gpgsign", "false"),
        ] {
            repo.git(&["config", key, value]);
        }
        repo
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        smol::block_on(
            util::command::new_command("git")
                .current_dir(&self.work)
                .envs(&self.env)
                .env("GIT_EDITOR", "true")
                .args(args)
                .output(),
        )
        .expect("spawn git")
    }

    fn git(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn write(&self, path: &str, content: impl AsRef<[u8]>) {
        std::fs::write(self.work.join(path), content).expect("write file");
    }

    fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.work.join(path)).expect("read file")
    }

    fn commit_all(&self, files: &[(&str, &str)], message: &str) {
        for (path, content) in files {
            self.write(path, content);
            self.git(&["add", "--", path]);
        }
        self.git(&["commit", "--quiet", "-m", message]);
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    fn unmerged(&self) -> String {
        self.git(&["diff", "--name-only", "--diff-filter=U"])
    }

    fn exists(&self, rev: &str) -> bool {
        self.run(&["rev-parse", "--verify", "--quiet", rev])
            .status
            .success()
    }

    /// main and feature change `a.txt` and `c.txt` differently.
    fn diverged(&self) {
        self.commit_all(&[("a.txt", "a\nbase\nz\n"), ("c.txt", "base\n")], "base");
        self.git(&["checkout", "--quiet", "-b", "feature"]);
        self.commit_all(
            &[("a.txt", "a\nfeature\nz\n"), ("c.txt", "feature\n")],
            "feature change",
        );
        self.git(&["checkout", "--quiet", "main"]);
        self.commit_all(
            &[("a.txt", "a\nmain\nz\n"), ("c.txt", "main\n")],
            "main change",
        );
    }

    fn merge_with_conflicts(&self) {
        self.diverged();
        assert!(
            !self
                .run(&["merge", "--no-edit", "feature"])
                .status
                .success()
        );
        assert_eq!(self.unmerged(), "a.txt\nc.txt");
    }
}

struct Fixture {
    workspace: Entity<Workspace>,
    repository: Entity<Repository>,
}

fn init_test(cx: &mut TestAppContext, install_brewery: bool) {
    cx.update(|cx| {
        let _ = AppState::test(cx);
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        release_channel::init(semver::Version::new(0, 0, 0), cx);
        editor::init(cx);
        let _ = cx.try_global::<SettingsStore>();
        language_model::init(cx);
        git_ui::init(cx);
        if install_brewery {
            crate::init(cx);
        }
    });
    SHOWN_TOASTS.with(|shown| shown.borrow_mut().clear());
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
    cx.run_until_parked();
    (
        Fixture {
            workspace,
            repository,
        },
        cx,
    )
}

fn path(name: &str) -> RepoPath {
    RepoPath::new(name).expect("repo path")
}

fn paths(names: &[&str]) -> Vec<RepoPath> {
    names.iter().map(|name| path(name)).collect()
}

/// Runs the app (real git processes, real fs events) until `check` holds.
fn wait_until(
    cx: &mut VisualTestContext,
    what: &str,
    mut check: impl FnMut(&mut VisualTestContext) -> bool,
) {
    for _ in 0..400 {
        cx.run_until_parked();
        if check(cx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
        cx.executor().advance_clock(Duration::from_millis(25));
    }
    panic!("timed out waiting for {what}");
}

fn dialog(fixture: &Fixture, cx: &VisualTestContext) -> Option<Entity<ConflictsDialog>> {
    fixture.workspace.read_with(cx, |workspace, cx| {
        workspace.active_modal::<ConflictsDialog>(cx)
    })
}

fn dialog_paths(dialog: &Entity<ConflictsDialog>, cx: &VisualTestContext) -> Vec<RepoPath> {
    dialog.read_with(cx, |dialog, _| dialog.paths().to_vec())
}

fn shown_toasts() -> Vec<OutcomeToast> {
    SHOWN_TOASTS.with(|shown| shown.borrow().clone())
}

fn merge_views(fixture: &Fixture, cx: &VisualTestContext) -> Vec<Entity<MergeView>> {
    fixture.workspace.read_with(cx, |workspace, cx| {
        workspace.items_of_type::<MergeView>(cx).collect()
    })
}

/// Opens the dialog with `brewery::ResolveConflicts` and waits until it has listed the files.
fn resolve_conflicts(fixture: &Fixture, cx: &mut VisualTestContext) -> Entity<ConflictsDialog> {
    cx.dispatch_action(ResolveConflicts);
    wait_until(cx, "the dialog", |cx| {
        dialog(fixture, cx).is_some_and(|dialog| dialog.read_with(cx, |d, _| d.is_loaded()))
    });
    dialog(fixture, cx).expect("dialog")
}

#[gpui::test]
async fn lists_the_conflicts_and_accept_theirs_resolves_one(cx: &mut TestAppContext) {
    init_test(cx, true);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;

    let dialog = resolve_conflicts(&fixture, cx);
    assert_eq!(dialog_paths(&dialog, cx), paths(&["a.txt", "c.txt"]));
    dialog.read_with(cx, |dialog, _| {
        assert_eq!(dialog.header().as_ref(), "Merging feature into main");
        assert_eq!(dialog.operation(), Some(Operation::Merge));
        assert!(!dialog.can_continue());
    });
    wait_until(cx, "mergeable", |cx| {
        dialog.read_with(cx, |dialog, _| {
            dialog.can_merge(&path("a.txt")) == Some(true)
        })
    });

    let task = dialog.update_in(cx, |dialog, window, cx| {
        dialog.accept(path("c.txt"), Choice::Theirs, window, cx)
    });
    task.await.expect("accept theirs");
    cx.run_until_parked();
    assert_eq!(repo.read("c.txt"), "feature\n");
    assert_eq!(repo.unmerged(), "a.txt");
    wait_until(cx, "the list refresh", |cx| {
        dialog_paths(&dialog, cx) == paths(&["a.txt"])
    });
    dialog.read_with(cx, |dialog, _| assert!(!dialog.can_continue()));
}

#[gpui::test]
async fn accept_yours_in_a_rebase_takes_the_rebased_branch(cx: &mut TestAppContext) {
    init_test(cx, true);
    let repo = TestRepo::new();
    repo.diverged();
    assert!(!repo.run(&["rebase", "feature"]).status.success());
    let (fixture, cx) = open_real(&repo, cx).await;

    let dialog = resolve_conflicts(&fixture, cx);
    dialog.read_with(cx, |dialog, _| {
        assert_eq!(dialog.header().as_ref(), "Rebasing main onto feature");
    });
    let task = dialog.update_in(cx, |dialog, window, cx| {
        dialog.accept(path("c.txt"), Choice::Yours, window, cx)
    });
    task.await.expect("accept yours");
    assert_eq!(repo.read("c.txt"), "main\n");
}

#[gpui::test]
async fn merge_opens_the_merge_editor_and_apply_comes_back(cx: &mut TestAppContext) {
    init_test(cx, true);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;
    let dialog = resolve_conflicts(&fixture, cx);

    let task = dialog.update_in(cx, |dialog, window, cx| {
        dialog.merge(path("a.txt"), window, cx)
    });
    task.await.expect("merge");
    cx.run_until_parked();
    let views = merge_views(&fixture, cx);
    assert_eq!(views.len(), 1);
    assert_eq!(
        views[0].read_with(cx, |view, _| view.path().clone()),
        path("a.txt")
    );
    assert!(
        self::dialog(&fixture, cx).is_none(),
        "the editor replaces the dialog"
    );

    let view = views[0].clone();
    view.update(cx, |view, cx| view.accept_left(cx));
    let task = view.update_in(cx, |view, window, cx| view.apply(window, cx));
    task.await.expect("apply");
    wait_until(cx, "the dialog to come back", |cx| {
        self::dialog(&fixture, cx).is_some_and(|dialog| {
            dialog.read_with(cx, |d, _| d.is_loaded() && d.paths() == paths(&["c.txt"]))
        })
    });
    assert_eq!(repo.read("a.txt"), "a\nmain\nz\n");
}

#[gpui::test]
async fn continue_after_everything_is_resolved_completes_the_merge(cx: &mut TestAppContext) {
    init_test(cx, true);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;
    let dialog = resolve_conflicts(&fixture, cx);

    for (file, choice) in [("a.txt", Choice::Yours), ("c.txt", Choice::Theirs)] {
        let task = dialog.update_in(cx, |dialog, window, cx| {
            dialog.accept(path(file), choice, window, cx)
        });
        task.await.expect("accept");
    }
    wait_until(cx, "nothing left", |cx| {
        dialog.read_with(cx, |dialog, _| dialog.can_continue())
    });

    let task = dialog.update_in(cx, |dialog, window, cx| {
        dialog.continue_operation(window, cx)
    });
    task.await.expect("continue");
    cx.run_until_parked();
    assert!(!repo.exists("MERGE_HEAD"));
    let parents = repo.git(&["rev-list", "--parents", "-n", "1", "HEAD"]);
    assert_eq!(parents.split_whitespace().count(), 3, "a merge commit");
    assert_eq!(repo.read("a.txt"), "a\nmain\nz\n");
    assert_eq!(repo.read("c.txt"), "feature\n");
    assert!(self::dialog(&fixture, cx).is_none());
    assert!(shown_toasts().contains(&OutcomeToast::success("Merge completed")));
}

#[gpui::test]
async fn abort_restores_the_pre_merge_head(cx: &mut TestAppContext) {
    init_test(cx, true);
    let repo = TestRepo::new();
    repo.diverged();
    let before = repo.head();
    assert!(
        !repo
            .run(&["merge", "--no-edit", "feature"])
            .status
            .success()
    );
    let (fixture, cx) = open_real(&repo, cx).await;

    // A merge editor left open for the aborted merge closes too, without asking.
    let repository = fixture.repository.clone();
    let open = fixture.workspace.update_in(cx, |workspace, window, cx| {
        MergeView::open(
            workspace.weak_handle(),
            repository,
            path("a.txt"),
            window,
            cx,
        )
    });
    let view = open.await.expect("merge view");
    view.update(cx, |view, cx| view.accept_left(cx));

    let dialog = resolve_conflicts(&fixture, cx);
    // Declining the confirmation keeps the merge.
    let task = dialog.update_in(cx, |dialog, window, cx| dialog.abort(window, cx));
    cx.run_until_parked();
    cx.simulate_prompt_answer("Cancel");
    task.await.expect("abort cancelled");
    assert!(repo.exists("MERGE_HEAD"));

    let task = dialog.update_in(cx, |dialog, window, cx| dialog.abort(window, cx));
    cx.run_until_parked();
    cx.simulate_prompt_answer("Abort");
    task.await.expect("abort");
    cx.run_until_parked();
    assert_eq!(repo.head(), before);
    assert!(!repo.exists("MERGE_HEAD"));
    assert_eq!(repo.unmerged(), "");
    assert_eq!(repo.read("a.txt"), "a\nmain\nz\n");
    assert!(self::dialog(&fixture, cx).is_none());
    assert!(!cx.has_pending_prompt());
    assert!(merge_views(&fixture, cx).is_empty());
}

#[gpui::test]
async fn resolve_conflicts_without_conflicts_says_so(cx: &mut TestAppContext) {
    init_test(cx, true);
    let repo = TestRepo::new();
    repo.diverged();
    let (fixture, cx) = open_real(&repo, cx).await;

    cx.dispatch_action(ResolveConflicts);
    wait_until(cx, "the toast", |_| !shown_toasts().is_empty());
    assert_eq!(shown_toasts(), vec![OutcomeToast::success("No conflicts")]);
    assert!(dialog(&fixture, cx).is_none());
}

#[gpui::test]
async fn binary_conflicts_offer_only_a_side(cx: &mut TestAppContext) {
    init_test(cx, true);
    let repo = TestRepo::new();
    repo.commit_all(&[("bin.dat", "\0base\n")], "base");
    repo.git(&["checkout", "--quiet", "-b", "feature"]);
    repo.commit_all(&[("bin.dat", "\0feature\n")], "feature");
    repo.git(&["checkout", "--quiet", "main"]);
    repo.commit_all(&[("bin.dat", "\0main\n")], "main");
    assert!(
        !repo
            .run(&["merge", "--no-edit", "feature"])
            .status
            .success()
    );
    let (fixture, cx) = open_real(&repo, cx).await;

    let dialog = resolve_conflicts(&fixture, cx);
    wait_until(cx, "mergeable", |cx| {
        dialog.read_with(cx, |dialog, _| dialog.can_merge(&path("bin.dat")).is_some())
    });
    dialog.read_with(cx, |dialog, _| {
        assert_eq!(dialog.can_merge(&path("bin.dat")), Some(false))
    });
}

/// Adds Zed's git panel, selects `file` and presses Enter in it.
async fn confirm_in_git_panel(fixture: &Fixture, file: &str, cx: &mut VisualTestContext) {
    let (weak, async_cx) = fixture.workspace.update_in(cx, |workspace, window, cx| {
        (workspace.weak_handle(), window.to_async(cx))
    });
    let git_panel = GitPanel::load(weak, async_cx).await.expect("git panel");
    fixture.workspace.update_in(cx, |workspace, window, cx| {
        workspace.add_panel(git_panel.clone(), window, cx);
        workspace.focus_panel::<GitPanel>(window, cx);
    });
    cx.executor().advance_clock(Duration::from_millis(100));
    cx.run_until_parked();
    let project_path = fixture.repository.read_with(cx, |repository, cx| {
        repository
            .repo_path_to_project_path(&path(file), cx)
            .expect("project path")
    });
    git_panel.update_in(cx, |panel, window, cx| {
        panel.select_entry_by_path(project_path, window, cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.dispatch_action(Box::new(menu::Confirm), cx));
    cx.run_until_parked();
}

#[gpui::test]
async fn git_panel_opens_conflicts_in_the_merge_editor(cx: &mut TestAppContext) {
    init_test(cx, true);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;

    confirm_in_git_panel(&fixture, "a.txt", cx).await;
    wait_until(cx, "the merge editor", |cx| {
        !merge_views(&fixture, cx).is_empty()
    });
    let views = merge_views(&fixture, cx);
    assert_eq!(
        views[0].read_with(cx, |view, _| view.path().clone()),
        path("a.txt")
    );
    fixture.workspace.read_with(cx, |workspace, cx| {
        assert!(workspace.item_of_type::<ProjectDiff>(cx).is_none());
    });
}

#[gpui::test]
async fn without_the_hook_the_git_panel_opens_its_diff(cx: &mut TestAppContext) {
    init_test(cx, false);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;

    confirm_in_git_panel(&fixture, "a.txt", cx).await;
    wait_until(cx, "the project diff", |cx| {
        fixture.workspace.read_with(cx, |workspace, cx| {
            workspace.item_of_type::<ProjectDiff>(cx).is_some()
        })
    });
    assert!(merge_views(&fixture, cx).is_empty());
}

#[gpui::test]
async fn conflicts_from_outside_brewery_show_one_toast_per_episode(cx: &mut TestAppContext) {
    init_test(cx, true);
    let repo = TestRepo::new();
    repo.diverged();
    let (fixture, cx) = open_real(&repo, cx).await;
    let conflict_toasts = || {
        shown_toasts()
            .into_iter()
            .filter(|toast| toast.kind == ToastKind::Conflicts)
            .count()
    };
    assert_eq!(conflict_toasts(), 0);

    // A merge Zed ran (e.g. its own Pull) stops on conflicts: a toast, no dialog.
    assert!(
        !repo
            .run(&["merge", "--no-edit", "feature"])
            .status
            .success()
    );
    wait_until(cx, "the conflicts toast", |_| conflict_toasts() == 1);
    assert!(
        dialog(&fixture, cx).is_none(),
        "the dialog never opens by itself"
    );
    assert_eq!(
        shown_toasts().last().map(|toast| toast.message.clone()),
        Some("Conflicts in 2 files".to_string())
    );

    // Further changes in the same episode stay quiet.
    repo.write("new.txt", "x\n");
    repo.git(&["checkout", "--theirs", "--", "c.txt"]);
    repo.git(&["add", "c.txt"]);
    let repository = &fixture.repository;
    wait_until(cx, "the rescan", |cx| {
        repository.read_with(cx, |repository, _| {
            repository
                .snapshot()
                .status_for_path(&path("c.txt"))
                .is_some_and(|entry| !entry.status.is_conflicted())
        })
    });
    assert_eq!(conflict_toasts(), 1);

    // Once the conflicts are gone, the next conflicts are a new episode.
    repo.git(&["merge", "--abort"]);
    wait_until(cx, "no conflicts", |cx| {
        repository.read_with(cx, |repository, _| {
            !repository
                .cached_status()
                .any(|entry| entry.status.is_conflicted())
        })
    });
    assert!(
        !repo
            .run(&["merge", "--no-edit", "feature"])
            .status
            .success()
    );
    wait_until(cx, "the second toast", |_| conflict_toasts() == 2);
}
