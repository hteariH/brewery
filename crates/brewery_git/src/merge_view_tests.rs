use super::*;
use editor::SelectionEffects;
use fs::RealFs;
use gpui::{TestAppContext, VisualTestContext};
use language::Point;
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

    fn commit(&self, path: &str, content: impl AsRef<[u8]>, message: &str) {
        self.write(path, content);
        self.git(&["add", "--", path]);
        self.git(&["commit", "--quiet", "-m", message]);
    }

    fn unmerged(&self) -> String {
        self.git(&["diff", "--name-only", "--diff-filter=U"])
    }

    /// `a.txt` on main and feature: both change `b` and `o` differently (two conflicts), main
    /// alone changes `h`, feature alone changes `e`.
    fn diverged(&self) {
        self.commit("a.txt", lines(BASE), "base");
        self.git(&["checkout", "--quiet", "-b", "feature"]);
        self.commit("a.txt", lines(FEATURE), "feature change");
        self.git(&["checkout", "--quiet", "main"]);
        self.commit("a.txt", lines(MAIN), "main change");
    }

    /// Merges feature into main, stopping on the conflicts.
    fn merge_with_conflicts(&self) {
        self.diverged();
        let output = self.run(&["merge", "--no-edit", "feature"]);
        assert!(!output.status.success(), "the merge should conflict");
        assert_eq!(self.unmerged(), "a.txt");
    }
}

const BASE: &[&str] = &[
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q",
];
const MAIN: &[&str] = &[
    "a", "B-main", "c", "d", "e", "f", "g", "H", "i", "j", "k", "l", "m", "n", "O-main", "p", "q",
];
const FEATURE: &[&str] = &[
    "a",
    "B-feature",
    "c",
    "d",
    "E",
    "f",
    "g",
    "h",
    "i",
    "j",
    "k",
    "l",
    "m",
    "n",
    "O-feature",
    "p",
    "q",
];

fn lines(lines: &[&str]) -> String {
    lines.iter().map(|line| format!("{line}\n")).collect()
}

/// BASE with the one-sided changes applied and `replace` (line index → text) on top.
fn result_with(replace: &[(usize, &str)]) -> String {
    let mut lines = BASE.iter().map(|line| line.to_string()).collect::<Vec<_>>();
    lines[4] = "E".into();
    lines[7] = "H".into();
    for (index, text) in replace {
        lines[*index] = text.to_string();
    }
    lines.iter().map(|line| format!("{line}\n")).collect()
}

struct Fixture {
    workspace: Entity<Workspace>,
    repository: Entity<Repository>,
}

fn init_test(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let _ = AppState::test(cx);
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        release_channel::init(semver::Version::new(0, 0, 0), cx);
        editor::init(cx);
        let _ = cx.try_global::<SettingsStore>();
    });
}

async fn open_real<'a>(
    repo: &TestRepo,
    cx: &'a mut TestAppContext,
) -> (Fixture, &'a mut VisualTestContext) {
    cx.executor().allow_parking();
    crate::cli::set_test_env(&repo.work, repo.env.clone());
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

async fn open_view(
    fixture: &Fixture,
    file: &str,
    cx: &mut VisualTestContext,
) -> Result<Entity<MergeView>> {
    let repository = fixture.repository.clone();
    let task = fixture.workspace.update_in(cx, |workspace, window, cx| {
        MergeView::open(workspace.weak_handle(), repository, path(file), window, cx)
    });
    let view = task.await;
    cx.run_until_parked();
    view
}

fn result_text(view: &Entity<MergeView>, cx: &VisualTestContext) -> String {
    view.read_with(cx, |view, cx| view.result_text(cx))
}

fn conflicts(view: &Entity<MergeView>, cx: &VisualTestContext) -> Vec<usize> {
    view.read_with(cx, |view, _| view.conflict_chunks())
}

fn resolution(view: &Entity<MergeView>, chunk: usize, cx: &VisualTestContext) -> Resolution {
    view.read_with(cx, |view, _| view.resolutions()[chunk])
}

fn cursor_row(view: &Entity<MergeView>, cx: &mut VisualTestContext) -> u32 {
    let editor = view.read_with(cx, |view, _| view.result_editor().clone());
    editor.update(cx, |editor, cx| {
        let snapshot = editor.display_snapshot(cx);
        editor.selections.newest::<Point>(&snapshot).head().row
    })
}

#[gpui::test]
async fn opens_with_one_sided_changes_applied_and_conflicts_showing_base(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;

    let view = open_view(&fixture, "a.txt", cx).await.expect("open");

    assert_eq!(result_text(&view, cx), result_with(&[]));
    let conflicts = conflicts(&view, cx);
    assert_eq!(conflicts.len(), 2);
    view.read_with(cx, |view, cx| {
        assert_eq!(view.unresolved_count(), 2);
        assert!(view.sides().operation == Some(Operation::Merge));
        assert_eq!(view.left_label().as_ref(), "Yours (main)");
        assert_eq!(view.right_label().as_ref(), "Theirs (feature)");
        assert_eq!(view.left_editor().read(cx).text(cx), lines(MAIN));
        assert_eq!(view.right_editor().read(cx).text(cx), lines(FEATURE));
        assert!(view.left_editor().read(cx).read_only(cx));
        assert!(view.right_editor().read(cx).read_only(cx));
        assert!(!view.result_editor().read(cx).read_only(cx));
        assert_eq!(view.tab_content_text(0, cx).as_ref(), "a.txt (merge)");
    });
    // The view is in the workspace's active pane.
    fixture.workspace.read_with(cx, |workspace, cx| {
        let active = workspace.active_item(cx).expect("active item");
        assert_eq!(active.item_id(), view.entity_id());
    });
}

#[gpui::test]
async fn accepting_a_side_and_both(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;
    let view = open_view(&fixture, "a.txt", cx).await.expect("open");
    let conflicts = conflicts(&view, cx);

    view.update(cx, |view, cx| {
        view.accept_chunk(conflicts[0], Side::Ours, cx)
    });
    assert_eq!(result_text(&view, cx), result_with(&[(1, "B-main")]));
    assert_eq!(resolution(&view, conflicts[0], cx), Resolution::Ours);
    assert_eq!(resolution(&view, conflicts[1], cx), Resolution::Unresolved);

    view.update(cx, |view, cx| {
        view.accept_chunk(conflicts[1], Side::Both, cx)
    });
    assert_eq!(
        result_text(&view, cx),
        result_with(&[(1, "B-main"), (14, "O-main\nO-feature")])
    );
    assert_eq!(resolution(&view, conflicts[1], cx), Resolution::Both);
    view.read_with(cx, |view, _| assert_eq!(view.unresolved_count(), 0));

    // Taking the other side replaces the chunk's lines again, wherever they moved.
    view.update(cx, |view, cx| {
        view.accept_chunk(conflicts[0], Side::Theirs, cx)
    });
    assert_eq!(
        result_text(&view, cx),
        result_with(&[(1, "B-feature"), (14, "O-main\nO-feature")])
    );
    view.update(cx, |view, cx| view.ignore_chunk(conflicts[1], cx));
    assert_eq!(resolution(&view, conflicts[1], cx), Resolution::Ignored);
}

#[gpui::test]
async fn accept_left_and_right_take_the_whole_file(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;
    let view = open_view(&fixture, "a.txt", cx).await.expect("open");

    view.update(cx, |view, cx| view.accept_left(cx));
    assert_eq!(result_text(&view, cx), lines(MAIN));
    view.read_with(cx, |view, _| assert_eq!(view.unresolved_count(), 0));

    view.update(cx, |view, cx| view.accept_right(cx));
    assert_eq!(result_text(&view, cx), lines(FEATURE));
}

#[gpui::test]
async fn accept_left_and_right_keep_hand_edited_chunks(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;
    let view = open_view(&fixture, "a.txt", cx).await.expect("open");
    let conflicts = conflicts(&view, cx);
    let editor = view.read_with(cx, |view, _| view.result_editor().clone());
    editor.update_in(cx, |editor, window, cx| {
        editor.change_selections(SelectionEffects::no_scroll(), window, cx, |selections| {
            selections.select_ranges([Point::new(1, 1)..Point::new(1, 1)])
        });
        editor.insert("X", window, cx);
    });
    cx.run_until_parked();
    assert_eq!(resolution(&view, conflicts[0], cx), Resolution::Edited);

    view.update(cx, |view, cx| view.accept_left(cx));
    assert_eq!(
        result_text(&view, cx),
        result_with(&[(1, "bX"), (4, "e"), (14, "O-main")]),
        "the hand-edited conflict is kept, every other change takes yours"
    );
    assert_eq!(resolution(&view, conflicts[0], cx), Resolution::Edited);
    assert_eq!(resolution(&view, conflicts[1], cx), Resolution::Ours);

    view.update(cx, |view, cx| view.accept_right(cx));
    assert_eq!(
        result_text(&view, cx),
        result_with(&[(1, "bX"), (7, "h"), (14, "O-feature")])
    );
}

#[gpui::test]
async fn manual_edit_inside_a_conflict_marks_it_resolved(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;
    let view = open_view(&fixture, "a.txt", cx).await.expect("open");
    let conflicts = conflicts(&view, cx);
    let editor = view.read_with(cx, |view, _| view.result_editor().clone());

    editor.update_in(cx, |editor, window, cx| {
        editor.change_selections(SelectionEffects::no_scroll(), window, cx, |selections| {
            selections.select_ranges([Point::new(1, 1)..Point::new(1, 1)])
        });
        editor.insert("X", window, cx);
    });
    cx.run_until_parked();

    assert_eq!(resolution(&view, conflicts[0], cx), Resolution::Edited);
    assert_eq!(resolution(&view, conflicts[1], cx), Resolution::Unresolved);
    // Chunks next to the edit are untouched.
    assert_eq!(resolution(&view, conflicts[0] - 1, cx), Resolution::Auto);
    assert_eq!(resolution(&view, conflicts[0] + 1, cx), Resolution::Auto);
    view.read_with(cx, |view, _| assert_eq!(view.unresolved_count(), 1));

    // The later conflict's lines are still found after the edit (and after a new line).
    editor.update_in(cx, |editor, window, cx| {
        editor.change_selections(SelectionEffects::no_scroll(), window, cx, |selections| {
            selections.select_ranges([Point::new(1, 0)..Point::new(1, 0)])
        });
        editor.insert("new line\n", window, cx);
    });
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        view.accept_chunk(conflicts[1], Side::Theirs, cx)
    });
    assert_eq!(
        result_text(&view, cx),
        result_with(&[(1, "new line\nbX"), (14, "O-feature")])
    );
}

#[gpui::test]
async fn next_and_previous_conflict_move_the_cursor(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;
    let view = open_view(&fixture, "a.txt", cx).await.expect("open");

    // The view opens on the first conflict.
    assert_eq!(cursor_row(&view, cx), 1);
    view.update_in(cx, |view, window, cx| view.next_conflict(window, cx));
    assert_eq!(cursor_row(&view, cx), 14);
    view.update_in(cx, |view, window, cx| view.next_conflict(window, cx));
    assert_eq!(
        cursor_row(&view, cx),
        14,
        "no wrapping past the last conflict"
    );
    view.update_in(cx, |view, window, cx| view.previous_conflict(window, cx));
    assert_eq!(cursor_row(&view, cx), 1);
    view.update_in(cx, |view, window, cx| view.previous_conflict(window, cx));
    assert_eq!(cursor_row(&view, cx), 1);
}

#[gpui::test]
async fn scrolling_one_side_aligns_the_others_by_chunk(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;
    let view = open_view(&fixture, "a.txt", cx).await.expect("open");
    let conflicts = conflicts(&view, cx);
    let (left, result) = view.read_with(cx, |view, _| {
        (view.left_editor().clone(), view.result_editor().clone())
    });
    // Make the second conflict two lines long in the result, so rows differ between panes.
    view.update(cx, |view, cx| {
        view.accept_chunk(conflicts[0], Side::Both, cx)
    });
    cx.run_until_parked();

    let left_row = view.read_with(cx, |view, _| view.merge().chunks[conflicts[1]].ours.start);
    let target = view.update(cx, |view, cx| {
        view.display_row_of(Column::Left, left_row as u32, cx)
    });
    left.update_in(cx, |editor, window, cx| {
        editor.set_scroll_position(gpui::point(0., target as f64), window, cx);
    });
    cx.run_until_parked();

    let result_top = result.update(cx, |editor, cx| {
        let snapshot = editor.display_snapshot(cx);
        let y = editor.scroll_position(cx).y;
        snapshot
            .display_point_to_point(
                editor::DisplayPoint::new(editor::display_map::DisplayRow(y as u32), 0),
                editor::Bias::Left,
            )
            .row
    });
    let expected = view.read_with(cx, |view, cx| view.result_rows(cx)[conflicts[1]].start);
    assert_eq!(result_top, expected);
    assert_eq!(expected, 15, "the earlier chunk grew by one line");
}

#[test]
fn aligned_row_maps_by_chunk() {
    // Three chunks: lines 0..2, 2..5, 5..6 on one side and 0..2, 2..3, 3..7 on the other.
    let from = [0..2, 2..5, 5..6];
    let to = [0..2, 2..3, 3..7];
    assert_eq!(aligned_row(&from, &to, 0.0), 0.0);
    assert_eq!(aligned_row(&from, &to, 1.5), 1.5);
    assert_eq!(aligned_row(&from, &to, 2.0), 2.0);
    // Deeper than the other side's chunk: stays on its last line.
    assert_eq!(aligned_row(&from, &to, 4.25), 2.0);
    assert_eq!(aligned_row(&from, &to, 5.0), 3.0);
    // Past the end: the last chunk's offset.
    assert_eq!(aligned_row(&from, &to, 7.0), 5.0);
    // An empty chunk on the other side maps to its position.
    let to = [0..2, 2..2, 2..3];
    assert_eq!(aligned_row(&from, &to, 3.0), 2.0);
}

#[gpui::test]
async fn apply_writes_stages_and_closes(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;
    let view = open_view(&fixture, "a.txt", cx).await.expect("open");
    let conflicts = conflicts(&view, cx);
    view.update(cx, |view, cx| {
        view.accept_chunk(conflicts[0], Side::Ours, cx);
        view.accept_chunk(conflicts[1], Side::Theirs, cx);
    });

    let task = view.update_in(cx, |view, window, cx| view.apply(window, cx));
    task.await.expect("apply");
    cx.run_until_parked();

    let expected = result_with(&[(1, "B-main"), (14, "O-feature")]);
    assert_eq!(repo.read("a.txt"), expected);
    assert_eq!(repo.unmerged(), "");
    assert_eq!(repo.git(&["show", ":a.txt"]), expected.trim_end());
    fixture.workspace.read_with(cx, |workspace, cx| {
        assert!(
            workspace.items_of_type::<MergeView>(cx).next().is_none(),
            "the view closes after Apply"
        );
    });

    // Zed's repository state follows once it rescans.
    let repository = fixture.repository.clone();
    for _ in 0..100 {
        let conflicted = repository.read_with(cx, |repository, _| {
            repository
                .snapshot()
                .status_for_path(&path("a.txt"))
                .is_some_and(|entry| entry.status.is_conflicted())
        });
        if !conflicted {
            return;
        }
        cx.executor().timer(Duration::from_millis(50)).await;
        cx.run_until_parked();
    }
    panic!("the repository still reports a conflict for a.txt");
}

#[gpui::test]
async fn apply_through_an_open_project_buffer(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;
    let project = fixture
        .workspace
        .read_with(cx, |workspace, _| workspace.project().clone());
    let project_path = fixture.repository.read_with(cx, |repository, cx| {
        repository
            .repo_path_to_project_path(&path("a.txt"), cx)
            .expect("project path")
    });
    let buffer = project
        .update(cx, |project, cx| project.open_buffer(project_path, cx))
        .await
        .expect("open buffer");
    let view = open_view(&fixture, "a.txt", cx).await.expect("open");
    view.update(cx, |view, cx| view.accept_left(cx));

    let task = view.update_in(cx, |view, window, cx| view.apply(window, cx));
    task.await.expect("apply");
    cx.run_until_parked();

    assert_eq!(repo.read("a.txt"), lines(MAIN));
    buffer.read_with(cx, |buffer, _| {
        assert_eq!(buffer.text(), lines(MAIN));
        assert!(!buffer.is_dirty());
    });
    assert_eq!(repo.unmerged(), "");
}

#[gpui::test]
async fn rebase_conflict_keeps_yours_on_the_left(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.diverged();
    repo.git(&["checkout", "--quiet", "feature"]);
    let output = repo.run(&["rebase", "main"]);
    assert!(!output.status.success(), "the rebase should conflict");
    let (fixture, cx) = open_real(&repo, cx).await;

    let view = open_view(&fixture, "a.txt", cx).await.expect("open");

    view.read_with(cx, |view, cx| {
        assert!(view.sides().operation == Some(Operation::Rebase));
        assert_eq!(view.left_label().as_ref(), "Yours (feature)");
        assert_eq!(view.right_label().as_ref(), "Theirs (main)");
        assert_eq!(view.left_editor().read(cx).text(cx), lines(FEATURE));
        assert_eq!(view.right_editor().read(cx).text(cx), lines(MAIN));
    });
    let conflicts = conflicts(&view, cx);
    view.update(cx, |view, cx| {
        view.accept_chunk(conflicts[0], Side::Ours, cx)
    });
    assert!(result_text(&view, cx).contains("B-feature\n"));
}

#[gpui::test]
async fn binary_conflicts_are_not_offered(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    let both = |bin: &[u8], text: &str, message: &str| {
        repo.write("bin.dat", bin);
        repo.write("text.txt", text);
        repo.git(&["add", "--", "bin.dat", "text.txt"]);
        repo.git(&["commit", "--quiet", "-m", message]);
    };
    both(b"base\0data", "base\n", "base");
    repo.git(&["checkout", "--quiet", "-b", "feature"]);
    both(b"feature\0data", "feature\n", "feature");
    repo.git(&["checkout", "--quiet", "main"]);
    both(b"main\0data", "main\n", "main");
    let output = repo.run(&["merge", "--no-edit", "feature"]);
    assert!(!output.status.success(), "the merge should conflict");
    let (fixture, cx) = open_real(&repo, cx).await;

    let repository = fixture.repository.clone();
    let mergeable = cx
        .update(|_, cx| MergeView::can_merge(&repository, path("bin.dat"), cx))
        .await
        .expect("can merge");
    assert!(!mergeable);
    let mergeable = cx
        .update(|_, cx| MergeView::can_merge(&repository, path("text.txt"), cx))
        .await
        .expect("can merge");
    assert!(mergeable);

    let error = open_view(&fixture, "bin.dat", cx)
        .await
        .err()
        .expect("binary files do not open");
    assert!(error.to_string().contains("binary"), "{error}");
    fixture.workspace.read_with(cx, |workspace, cx| {
        assert!(workspace.items_of_type::<MergeView>(cx).next().is_none());
    });
}

/// Commits `path` as a symlink to `target` (mode 120000) straight into the index, so the test
/// needs no symlink support, then checks it out.
fn commit_symlink(repo: &TestRepo, path: &str, target: &str, message: &str) {
    repo.write(".link-target", target);
    let oid = repo.git(&["hash-object", "-w", "--", ".link-target"]);
    std::fs::remove_file(repo.work.join(".link-target")).expect("remove target");
    let info = format!("120000,{oid},{path}");
    repo.git(&["update-index", "--add", "--cacheinfo", &info]);
    repo.git(&["commit", "--quiet", "-m", message]);
    repo.git(&["reset", "--quiet", "--hard"]);
}

#[gpui::test]
async fn symlink_conflicts_are_not_offered(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    commit_symlink(&repo, "link", "base.txt", "base");
    repo.git(&["checkout", "--quiet", "-b", "feature"]);
    commit_symlink(&repo, "link", "feature.txt", "feature");
    repo.git(&["checkout", "--quiet", "main"]);
    commit_symlink(&repo, "link", "main.txt", "main");
    let output = repo.run(&["merge", "--no-edit", "feature"]);
    assert!(!output.status.success(), "the merge should conflict");
    let (fixture, cx) = open_real(&repo, cx).await;

    let repository = fixture.repository.clone();
    let mergeable = cx
        .update(|_, cx| MergeView::can_merge(&repository, path("link"), cx))
        .await
        .expect("can merge");
    assert!(!mergeable);
    let error = open_view(&fixture, "link", cx)
        .await
        .err()
        .expect("symlinks do not open");
    assert!(error.to_string().contains("symlink"), "{error}");
    assert_eq!(open_views(&fixture, cx), 0);
}

#[cfg(unix)]
#[gpui::test]
async fn apply_refuses_to_write_through_a_symlink(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;
    let view = open_view(&fixture, "a.txt", cx).await.expect("open");
    view.update(cx, |view, cx| view.accept_left(cx));

    let outside = tempfile::tempdir().expect("temp dir");
    let target = outside.path().join("target.txt");
    std::fs::write(&target, "outside\n").expect("write target");
    std::fs::remove_file(repo.work.join("a.txt")).expect("remove");
    std::os::unix::fs::symlink(&target, repo.work.join("a.txt")).expect("symlink");

    let task = view.update_in(cx, |view, window, cx| view.apply(window, cx));
    let error = task.await.err().expect("apply should fail");
    assert!(error.to_string().contains("symlink"), "{error}");
    assert_eq!(std::fs::read_to_string(&target).expect("read"), "outside\n");
}

fn close_view(
    fixture: &Fixture,
    view: &Entity<MergeView>,
    cx: &mut VisualTestContext,
) -> Task<Result<()>> {
    let pane = fixture
        .workspace
        .read_with(cx, |workspace, _| workspace.active_pane().clone());
    let id = view.entity_id();
    pane.update_in(cx, |pane, window, cx| {
        pane.close_item_by_id(id, workspace::SaveIntent::Close, window, cx)
    })
}

fn open_views(fixture: &Fixture, cx: &VisualTestContext) -> usize {
    fixture.workspace.read_with(cx, |workspace, cx| {
        workspace.items_of_type::<MergeView>(cx).count()
    })
}

#[gpui::test]
async fn closing_with_unapplied_work_asks_first(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;
    let view = open_view(&fixture, "a.txt", cx).await.expect("open");

    // Untouched: closes without a prompt.
    view.read_with(cx, |view, cx| assert!(!view.is_dirty(cx)));
    close_view(&fixture, &view, cx).await.expect("close");
    cx.run_until_parked();
    assert!(!cx.has_pending_prompt());
    assert_eq!(open_views(&fixture, cx), 0);

    // After taking a side, closing (the tab or Cancel) asks; Cancel keeps the view.
    let view = open_view(&fixture, "a.txt", cx).await.expect("open");
    let conflicts = conflicts(&view, cx);
    view.update(cx, |view, cx| {
        view.accept_chunk(conflicts[0], Side::Ours, cx)
    });
    view.read_with(cx, |view, cx| assert!(view.is_dirty(cx)));
    let close = close_view(&fixture, &view, cx);
    cx.run_until_parked();
    assert!(
        cx.has_pending_prompt(),
        "closing asks about the unapplied merge"
    );
    cx.simulate_prompt_answer("Cancel");
    close.await.expect("close");
    cx.run_until_parked();
    assert_eq!(open_views(&fixture, cx), 1);

    view.update(cx, |view, cx| view.cancel(cx));
    cx.run_until_parked();
    assert!(cx.has_pending_prompt(), "Cancel asks too");
    cx.simulate_prompt_answer("Don't Save");
    cx.run_until_parked();
    assert_eq!(open_views(&fixture, cx), 0);
    assert_eq!(repo.unmerged(), "a.txt", "nothing was applied");

    // An ignored chunk is work too.
    let view = open_view(&fixture, "a.txt", cx).await.expect("open");
    let conflicts = self::conflicts(&view, cx);
    view.update(cx, |view, cx| view.ignore_chunk(conflicts[0], cx));
    view.read_with(cx, |view, cx| assert!(view.is_dirty(cx)));
}

#[gpui::test]
async fn saving_from_the_close_prompt_writes_without_staging(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;
    let view = open_view(&fixture, "a.txt", cx).await.expect("open");
    view.update(cx, |view, cx| view.accept_left(cx));

    let close = close_view(&fixture, &view, cx);
    cx.run_until_parked();
    cx.simulate_prompt_answer("Save");
    close.await.expect("close");
    cx.run_until_parked();
    assert_eq!(open_views(&fixture, cx), 0);
    assert_eq!(repo.read("a.txt"), lines(MAIN));
    assert_eq!(repo.unmerged(), "a.txt", "only Apply marks it resolved");
}

/// Ctrl+S (and Save All) is a habit while editing: it writes the result, but only Apply
/// stages the file and closes the view.
#[gpui::test]
async fn save_writes_the_result_without_staging_or_closing(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;
    let view = open_view(&fixture, "a.txt", cx).await.expect("open");
    view.update(cx, |view, cx| view.accept_left(cx));
    view.read_with(cx, |view, _| assert_eq!(view.unresolved_count(), 0));

    let project = fixture
        .workspace
        .read_with(cx, |workspace, _| workspace.project().clone());
    let task = view.update_in(cx, |view, window, cx| {
        view.save(SaveOptions::default(), project.clone(), window, cx)
    });
    task.await.expect("save");
    cx.run_until_parked();
    assert!(!cx.has_pending_prompt(), "nothing to confirm");
    assert_eq!(repo.read("a.txt"), lines(MAIN));
    assert_eq!(repo.unmerged(), "a.txt", "still conflicted");
    assert_eq!(open_views(&fixture, cx), 1, "the view stays open");
    view.read_with(cx, |view, cx| {
        assert!(!view.is_dirty(cx), "saved");
        assert!(view.has_unapplied_work(), "but not applied");
    });

    // Editing again makes it dirty; Apply then takes the saved file as its own, no prompt.
    let conflicts = conflicts(&view, cx);
    view.update(cx, |view, cx| {
        view.accept_chunk(conflicts[1], Side::Theirs, cx)
    });
    view.read_with(cx, |view, cx| assert!(view.is_dirty(cx)));
    let task = view.update_in(cx, |view, window, cx| {
        view.save(SaveOptions::default(), project.clone(), window, cx)
    });
    task.await.expect("save again");
    // Yours everywhere (Accept Left), then theirs for the second conflict.
    let expected = result_with(&[(1, "B-main"), (4, "e"), (14, "O-feature")]);
    assert_eq!(repo.read("a.txt"), expected);

    let task = view.update_in(cx, |view, window, cx| view.apply(window, cx));
    task.await
        .expect("apply without asking about outside changes");
    cx.run_until_parked();
    assert!(!cx.has_pending_prompt());
    assert_eq!(repo.unmerged(), "");
    assert_eq!(open_views(&fixture, cx), 0);
}

#[gpui::test]
async fn save_still_asks_about_changes_made_outside(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;
    let view = open_view(&fixture, "a.txt", cx).await.expect("open");
    view.update(cx, |view, cx| view.accept_left(cx));
    let project = fixture
        .workspace
        .read_with(cx, |workspace, _| workspace.project().clone());
    let task = view.update_in(cx, |view, window, cx| {
        view.save(SaveOptions::default(), project.clone(), window, cx)
    });
    task.await.expect("save");
    repo.write("a.txt", "changed outside\n");

    let task = view.update_in(cx, |view, window, cx| view.apply(window, cx));
    assert!(wait_for_prompt(cx).await);
    cx.simulate_prompt_answer("Cancel");
    assert_eq!(apply_error(task.await), ApplyError::Cancelled);
    assert_eq!(repo.read("a.txt"), "changed outside\n");
}

#[gpui::test]
async fn applying_does_not_ask_on_close(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;
    let view = open_view(&fixture, "a.txt", cx).await.expect("open");
    view.update(cx, |view, cx| view.accept_right(cx));
    let task = view.update_in(cx, |view, window, cx| view.apply(window, cx));
    task.await.expect("apply");
    cx.run_until_parked();
    assert!(!cx.has_pending_prompt());
    assert_eq!(open_views(&fixture, cx), 0);
}

#[gpui::test]
fn f7_moves_between_conflicts_in_the_jetbrains_keymaps(cx: &mut TestAppContext) {
    init_test(cx);
    let inside = ["Workspace", "Pane", "BreweryMergeView", "Editor"];
    for keymap in [
        include_str!("../../../assets/keymaps/linux/jetbrains.json"),
        include_str!("../../../assets/keymaps/macos/jetbrains.json"),
    ] {
        for (key, action) in [
            ("f7", "brewery_merge::NextConflict"),
            ("shift-f7", "brewery_merge::PreviousConflict"),
        ] {
            assert_eq!(
                bindings_for(keymap, key, &inside, cx).first(),
                Some(&action.to_string()),
                "{key} in the merge editor"
            );
        }
        // Outside the merge editor F7 keeps its debugger binding.
        assert!(
            !bindings_for(keymap, "f7", &["Workspace", "Pane", "Editor"], cx)
                .iter()
                .any(|action| action.starts_with("brewery_merge::"))
        );
    }
}

/// The actions a keymap resolves for `keystroke` with the context stack `contexts`, most
/// specific first. Bindings for actions this test binary does not link are skipped.
fn bindings_for(
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

// ---------------------------------------------------------------------------------------------
// The merge editor never loses work: line endings, stale conflicts, outside changes, undo.
// ---------------------------------------------------------------------------------------------

fn crlf(text: &str) -> String {
    text.replace('\n', "\r\n")
}

fn apply_error(result: Result<()>) -> ApplyError {
    let error = result.err().expect("apply should fail");
    error
        .downcast_ref::<ApplyError>()
        .cloned()
        .unwrap_or_else(|| panic!("not an ApplyError: {error:#}"))
}

/// Waits for a prompt that comes after git calls (real processes, so parking is not enough).
async fn wait_for_prompt(cx: &mut VisualTestContext) -> bool {
    for _ in 0..200 {
        cx.run_until_parked();
        if cx.has_pending_prompt() {
            return true;
        }
        cx.executor().timer(Duration::from_millis(25)).await;
    }
    false
}

async fn open_project_buffer(
    fixture: &Fixture,
    file: &str,
    cx: &mut VisualTestContext,
) -> Entity<Buffer> {
    let project = fixture
        .workspace
        .read_with(cx, |workspace, _| workspace.project().clone());
    let project_path = fixture.repository.read_with(cx, |repository, cx| {
        repository
            .repo_path_to_project_path(&path(file), cx)
            .expect("project path")
    });
    project
        .update(cx, |project, cx| project.open_buffer(project_path, cx))
        .await
        .expect("open buffer")
}

#[gpui::test]
async fn apply_keeps_crlf_under_autocrlf(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.git(&["config", "core.autocrlf", "true"]);
    repo.merge_with_conflicts();
    assert!(repo.read("a.txt").contains("\r\n"), "git checks out CRLF");
    let (fixture, cx) = open_real(&repo, cx).await;
    let view = open_view(&fixture, "a.txt", cx).await.expect("open");
    let conflicts = conflicts(&view, cx);
    view.read_with(cx, |view, cx| {
        assert_eq!(view.left_editor().read(cx).text(cx), lines(MAIN));
    });
    view.update(cx, |view, cx| {
        view.accept_chunk(conflicts[0], Side::Ours, cx);
        view.accept_chunk(conflicts[1], Side::Theirs, cx);
    });

    let task = view.update_in(cx, |view, window, cx| view.apply(window, cx));
    task.await.expect("apply");
    cx.run_until_parked();

    let expected = result_with(&[(1, "B-main"), (14, "O-feature")]);
    assert_eq!(repo.read("a.txt"), crlf(&expected));
    assert_eq!(repo.unmerged(), "");
    assert!(
        !repo.git(&["cat-file", "blob", ":0:a.txt"]).contains('\r'),
        "the index keeps LF"
    );
}

#[gpui::test]
async fn apply_through_an_open_buffer_keeps_eol_crlf(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.commit(".gitattributes", "a.txt text eol=crlf\n", "attributes");
    repo.merge_with_conflicts();
    assert!(repo.read("a.txt").contains("\r\n"));
    let (fixture, cx) = open_real(&repo, cx).await;
    let buffer = open_project_buffer(&fixture, "a.txt", cx).await;
    let view = open_view(&fixture, "a.txt", cx).await.expect("open");
    view.update(cx, |view, cx| view.accept_left(cx));

    let task = view.update_in(cx, |view, window, cx| view.apply(window, cx));
    task.await.expect("apply");
    cx.run_until_parked();

    assert_eq!(repo.read("a.txt"), crlf(&lines(MAIN)));
    buffer.read_with(cx, |buffer, _| {
        assert_eq!(buffer.line_ending(), LineEnding::Windows);
        assert!(!buffer.is_dirty());
    });
    assert_eq!(repo.unmerged(), "");
}

#[gpui::test]
async fn apply_refuses_a_conflict_that_changed_and_open_reloads(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.diverged();
    repo.git(&["checkout", "--quiet", "feature"]);
    let mut second = FEATURE.to_vec();
    second[1] = "B-feature2";
    repo.commit("a.txt", lines(&second), "feature second change");
    assert!(!repo.run(&["rebase", "main"]).status.success());
    let (fixture, cx) = open_real(&repo, cx).await;
    let old = open_view(&fixture, "a.txt", cx).await.expect("open");

    // Outside the merge editor: keep main's lines and go on to the next commit, which
    // conflicts on the same file again.
    repo.git(&["checkout", "--ours", "--", "a.txt"]);
    repo.git(&["add", "--", "a.txt"]);
    assert!(!repo.run(&["rebase", "--continue"]).status.success());
    assert_eq!(repo.unmerged(), "a.txt");
    let on_disk = repo.read("a.txt");

    let task = old.update_in(cx, |view, window, cx| view.apply(window, cx));
    let error = apply_error(task.await);
    assert!(matches!(error, ApplyError::ConflictChanged(_)), "{error}");
    assert_eq!(repo.read("a.txt"), on_disk, "nothing was written");
    assert_eq!(repo.unmerged(), "a.txt");

    let new = open_view(&fixture, "a.txt", cx).await.expect("reopen");
    assert_ne!(
        new.entity_id(),
        old.entity_id(),
        "a fresh view for the new conflict"
    );
    new.read_with(cx, |view, cx| {
        assert!(
            view.left_editor()
                .read(cx)
                .text(cx)
                .contains("B-feature2\n")
        );
    });
    assert_eq!(open_views(&fixture, cx), 1, "the stale view is closed");

    // The current view is reused.
    let again = open_view(&fixture, "a.txt", cx).await.expect("open again");
    assert_eq!(again.entity_id(), new.entity_id());
}

#[gpui::test]
async fn apply_refuses_a_file_resolved_outside(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;
    let view = open_view(&fixture, "a.txt", cx).await.expect("open");
    view.update(cx, |view, cx| view.accept_left(cx));

    repo.git(&["checkout", "--theirs", "--", "a.txt"]);
    repo.git(&["add", "--", "a.txt"]);

    let task = view.update_in(cx, |view, window, cx| view.apply(window, cx));
    let error = apply_error(task.await);
    assert!(matches!(error, ApplyError::NotConflicted(_)), "{error}");
    assert_eq!(
        repo.read("a.txt"),
        lines(FEATURE),
        "the outside resolution stays"
    );
}

#[gpui::test]
async fn apply_asks_before_replacing_unsaved_buffer_edits(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;
    let buffer = open_project_buffer(&fixture, "a.txt", cx).await;
    buffer.update(cx, |buffer, cx| buffer.edit([(0..0, "edited\n")], None, cx));
    let on_disk = repo.read("a.txt");
    let view = open_view(&fixture, "a.txt", cx).await.expect("open");
    view.update(cx, |view, cx| view.accept_left(cx));

    let task = view.update_in(cx, |view, window, cx| view.apply(window, cx));
    assert!(
        wait_for_prompt(cx).await,
        "Apply asks about the unsaved edits"
    );
    cx.simulate_prompt_answer("Cancel");
    assert_eq!(apply_error(task.await), ApplyError::Cancelled);
    assert_eq!(repo.read("a.txt"), on_disk);
    assert_eq!(repo.unmerged(), "a.txt");
    buffer.read_with(cx, |buffer, _| {
        assert!(buffer.text().starts_with("edited\n"))
    });

    let task = view.update_in(cx, |view, window, cx| view.apply(window, cx));
    assert!(wait_for_prompt(cx).await);
    cx.simulate_prompt_answer("Replace");
    task.await.expect("apply");
    cx.run_until_parked();
    assert_eq!(repo.read("a.txt"), lines(MAIN));
    assert_eq!(repo.unmerged(), "");
}

#[gpui::test]
async fn apply_asks_before_replacing_a_file_resolved_by_hand(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    // E.g. with Zed's inline conflict buttons: saved, without markers, not staged.
    repo.write("a.txt", lines(FEATURE));
    let (fixture, cx) = open_real(&repo, cx).await;
    let view = open_view(&fixture, "a.txt", cx).await.expect("open");
    view.update(cx, |view, cx| view.accept_left(cx));

    let task = view.update_in(cx, |view, window, cx| view.apply(window, cx));
    assert!(wait_for_prompt(cx).await);
    cx.simulate_prompt_answer("Cancel");
    assert_eq!(apply_error(task.await), ApplyError::Cancelled);
    assert_eq!(repo.read("a.txt"), lines(FEATURE));

    let task = view.update_in(cx, |view, window, cx| view.apply(window, cx));
    assert!(wait_for_prompt(cx).await);
    cx.simulate_prompt_answer("Replace");
    task.await.expect("apply");
    assert_eq!(repo.read("a.txt"), lines(MAIN));
}

#[gpui::test]
async fn apply_asks_when_the_file_changed_after_the_view_opened(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;
    let view = open_view(&fixture, "a.txt", cx).await.expect("open");
    view.update(cx, |view, cx| view.accept_left(cx));
    let mut changed = repo.read("a.txt");
    changed.push_str("appended outside\n");
    repo.write("a.txt", &changed);

    let task = view.update_in(cx, |view, window, cx| view.apply(window, cx));
    assert!(wait_for_prompt(cx).await);
    cx.simulate_prompt_answer("Cancel");
    assert_eq!(apply_error(task.await), ApplyError::Cancelled);
    assert_eq!(repo.read("a.txt"), changed);
}

#[gpui::test]
async fn undoing_an_accept_makes_the_conflict_unresolved_again(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;
    let view = open_view(&fixture, "a.txt", cx).await.expect("open");
    let conflicts = conflicts(&view, cx);
    let editor = view.read_with(cx, |view, _| view.result_editor().clone());

    view.update(cx, |view, cx| {
        view.accept_chunk(conflicts[0], Side::Ours, cx)
    });
    cx.run_until_parked();
    editor.update_in(cx, |editor, window, cx| {
        editor.undo(&editor::actions::Undo, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(result_text(&view, cx), result_with(&[]));
    assert_eq!(resolution(&view, conflicts[0], cx), Resolution::Unresolved);
    view.read_with(cx, |view, cx| {
        assert_eq!(view.unresolved_count(), 2);
        assert_eq!(view.conflicts_without_a_side(cx), 2);
    });

    editor.update_in(cx, |editor, window, cx| {
        editor.redo(&editor::actions::Redo, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(result_text(&view, cx), result_with(&[(1, "B-main")]));
    assert_eq!(resolution(&view, conflicts[0], cx), Resolution::Ours);

    // Typing the base text back by hand is not a choice either.
    view.update(cx, |view, cx| {
        view.accept_chunk(conflicts[1], Side::Theirs, cx)
    });
    editor.update_in(cx, |editor, window, cx| {
        editor.change_selections(SelectionEffects::no_scroll(), window, cx, |selections| {
            selections.select_ranges([Point::new(14, 0)..Point::new(14, 9)])
        });
        editor.insert("o", window, cx);
    });
    cx.run_until_parked();
    assert_eq!(result_text(&view, cx), result_with(&[(1, "B-main")]));
    assert_eq!(resolution(&view, conflicts[1], cx), Resolution::Unresolved);
}

#[gpui::test]
async fn an_empty_conflict_is_resolved_only_by_typing_into_it(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    let base = ["p", "q", "r", "s", "t", "u"];
    repo.commit("e.txt", lines(&base), "base");
    repo.git(&["checkout", "--quiet", "-b", "feature"]);
    repo.commit(
        "e.txt",
        lines(&["p", "THEIRS", "q", "r", "s", "t", "u"]),
        "feature",
    );
    repo.git(&["checkout", "--quiet", "main"]);
    repo.commit(
        "e.txt",
        lines(&["p", "OURS", "q", "r", "s", "t", "u"]),
        "main",
    );
    assert!(
        !repo
            .run(&["merge", "--no-edit", "feature"])
            .status
            .success()
    );
    let (fixture, cx) = open_real(&repo, cx).await;
    let view = open_view(&fixture, "e.txt", cx).await.expect("open");
    let conflicts = conflicts(&view, cx);
    assert_eq!(conflicts.len(), 1);
    let conflict = conflicts[0];
    assert_eq!(result_text(&view, cx), lines(&base), "an empty base");
    let editor = view.read_with(cx, |view, _| view.result_editor().clone());

    // Backspace at the start of the line after it: joins "p" and "q", ends at the conflict.
    editor.update_in(cx, |editor, window, cx| {
        editor.change_selections(SelectionEffects::no_scroll(), window, cx, |selections| {
            selections.select_ranges([Point::new(1, 0)..Point::new(1, 0)])
        });
        editor.backspace(&editor::actions::Backspace, window, cx);
    });
    cx.run_until_parked();
    assert_eq!(resolution(&view, conflict, cx), Resolution::Unresolved);
    editor.update_in(cx, |editor, window, cx| {
        editor.undo(&editor::actions::Undo, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(result_text(&view, cx), lines(&base));
    assert_eq!(resolution(&view, conflict, cx), Resolution::Unresolved);

    // Typing at its position fills it.
    editor.update_in(cx, |editor, window, cx| {
        editor.change_selections(SelectionEffects::no_scroll(), window, cx, |selections| {
            selections.select_ranges([Point::new(1, 0)..Point::new(1, 0)])
        });
        editor.insert("mine\n", window, cx);
    });
    cx.run_until_parked();
    assert_eq!(resolution(&view, conflict, cx), Resolution::Edited);
    assert_eq!(resolution(&view, conflict + 1, cx), Resolution::Auto);
    view.read_with(cx, |view, cx| {
        assert_eq!(view.result_rows(cx)[conflict], 1..2);
        assert_eq!(view.conflicts_without_a_side(cx), 0);
    });
}

#[gpui::test]
async fn ignored_conflicts_still_count_for_the_apply_warning(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.merge_with_conflicts();
    let (fixture, cx) = open_real(&repo, cx).await;
    let view = open_view(&fixture, "a.txt", cx).await.expect("open");
    let conflicts = conflicts(&view, cx);

    view.update(cx, |view, cx| view.ignore_chunk(conflicts[0], cx));
    view.read_with(cx, |view, cx| {
        assert_eq!(view.unresolved_count(), 1);
        assert_eq!(view.conflicts_without_a_side(cx), 2);
    });
    view.update(cx, |view, cx| {
        view.accept_chunk(conflicts[1], Side::Theirs, cx)
    });
    view.read_with(cx, |view, cx| {
        assert_eq!(view.conflicts_without_a_side(cx), 1)
    });
}
