use super::*;
use editor::test::editor_test_context::assert_state_with_diff;
use fs::RealFs;
use gpui::{TestAppContext, VisualTestContext};
use language::{Language, LanguageConfig, LanguageMatcher};
use settings::{DiffViewStyle, SettingsStore};
use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use tempfile::TempDir;
use workspace::{AppState, MultiWorkspace, SaveIntent};
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

    fn write(&self, path: &str, content: impl AsRef<[u8]>) {
        std::fs::write(self.work.join(path), content).expect("write file");
    }

    /// Commits a change and returns the new commit's hash.
    fn commit(&self, path: &str, content: impl AsRef<[u8]>, message: &str) -> String {
        self.write(path, content);
        self.git(&["add", "--", path]);
        self.git(&["commit", "--quiet", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }
}

struct Fixture {
    workspace: Entity<Workspace>,
    project: Entity<Project>,
    repository: Entity<Repository>,
}

fn init_test(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let _ = AppState::test(cx);
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        release_channel::init(semver::Version::new(0, 0, 0), cx);
        editor::init(cx);
        cx.update_global::<SettingsStore, _>(|store, cx| {
            store.update_user_settings(cx, |settings| {
                settings.editor.diff_view_style = Some(DiffViewStyle::Unified);
            });
        });
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
            project,
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
    old: &str,
    new: NewSide,
    cx: &mut VisualTestContext,
) -> Result<Entity<RevisionDiffView>> {
    let repository = fixture.repository.clone();
    let task = fixture.workspace.update_in(cx, |workspace, window, cx| {
        RevisionDiffView::open(
            workspace.weak_handle(),
            repository,
            path(file),
            old.to_string(),
            new,
            window,
            cx,
        )
    });
    let view = task.await;
    cx.run_until_parked();
    view
}

fn rhs_editor(view: &Entity<RevisionDiffView>, cx: &VisualTestContext) -> Entity<Editor> {
    view.read_with(cx, |view, cx| {
        view.splittable_editor()
            .expect("a diff, not a message")
            .read(cx)
            .rhs_editor()
            .clone()
    })
}

fn short(sha: &str) -> &str {
    &sha[..SHORT_SHA_LEN]
}

#[gpui::test]
async fn modified_file_against_an_earlier_revision(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    let first = repo.commit("a.txt", "one\ntwo\nthree\n", "first");
    repo.commit("a.txt", "one\nTWO\nthree\n", "second");
    repo.write("a.txt", "one\nTWO\nthree\nfour\n");
    let (fixture, cx) = open_real(&repo, cx).await;

    let view = open_view(&fixture, "a.txt", "HEAD~1", NewSide::WorkingCopy, cx)
        .await
        .expect("open");

    assert_state_with_diff(
        &rhs_editor(&view, cx),
        cx,
        "  ˇone\n- two\n+ TWO\n  three\n+ four\n",
    );
    view.read_with(cx, |view, cx| {
        assert_eq!(
            view.tab_content_text(0, cx),
            "a.txt (HEAD~1) ↔ Working Copy"
        );
        let tooltip = view.tab_tooltip_text(cx).expect("tooltip");
        assert_eq!(
            tooltip.as_ref(),
            format!("a.txt\nHEAD~1 ({first}) ↔ Working Copy")
        );
        assert_eq!(
            view.old_side(),
            &ResolvedSide::Revision {
                rev: "HEAD~1".into(),
                sha: first.parse().expect("sha"),
            }
        );
        assert_eq!(view.new_side(), &ResolvedSide::WorkingCopy);
        assert_eq!(view.path(), &path("a.txt"));
        assert!(view.message().is_none());
        // The right side is the project's worktree buffer, editable.
        let buffer = view.new_buffer().expect("buffer").read(cx);
        assert!(!buffer.read_only());
        assert!(buffer.file().is_some());
    });
    let item_count = fixture.workspace.read_with(cx, |workspace, cx| {
        workspace
            .active_pane()
            .read(cx)
            .items()
            .filter(|item| item.downcast::<RevisionDiffView>().is_some())
            .count()
    });
    assert_eq!(item_count, 1, "the view is added to the active pane");

    // Editing the working copy updates the diff.
    let editor = rhs_editor(&view, cx);
    editor.update_in(cx, |editor, window, cx| {
        editor.move_to_end(&editor::actions::MoveToEnd, window, cx);
        editor.insert("five\n", window, cx);
    });
    cx.executor().advance_clock(RECALCULATE_DIFF_DEBOUNCE);
    cx.run_until_parked();
    assert_state_with_diff(
        &editor,
        cx,
        "  one\n- two\n+ TWO\n  three\n+ four\n+ five\n  ˇ",
    );
}

#[gpui::test]
async fn file_missing_at_the_revision_is_all_added(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.commit("a.txt", "a\n", "first");
    repo.write("b.txt", "new 1\nnew 2\n");
    let (fixture, cx) = open_real(&repo, cx).await;

    let view = open_view(&fixture, "b.txt", "HEAD", NewSide::WorkingCopy, cx)
        .await
        .expect("open");
    assert_state_with_diff(&rhs_editor(&view, cx), cx, "+ ˇnew 1\n+ new 2\n");
}

#[gpui::test]
async fn file_deleted_in_the_working_copy_is_all_deleted(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.commit("a.txt", "old 1\nold 2\n", "first");
    std::fs::remove_file(repo.work.join("a.txt")).expect("remove");
    let (fixture, cx) = open_real(&repo, cx).await;

    let view = open_view(&fixture, "a.txt", "HEAD", NewSide::WorkingCopy, cx)
        .await
        .expect("open");
    assert_state_with_diff(&rhs_editor(&view, cx), cx, "- old 1\n- old 2\n  ˇ");
}

#[gpui::test]
async fn revision_against_revision(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    let first = repo.commit("a.txt", "one\ntwo\n", "first");
    let second = repo.commit("a.txt", "one\n2\n", "second");
    repo.write("a.txt", "something else\n");
    let (fixture, cx) = open_real(&repo, cx).await;

    let view = open_view(
        &fixture,
        "a.txt",
        &first,
        NewSide::Revision(second.clone()),
        cx,
    )
    .await
    .expect("open");
    assert_state_with_diff(&rhs_editor(&view, cx), cx, "  ˇone\n- two\n+ 2\n");
    view.read_with(cx, |view, cx| {
        assert_eq!(
            view.tab_content_text(0, cx),
            format!("a.txt ({}) ↔ ({})", short(&first), short(&second))
        );
        assert_eq!(
            view.tab_tooltip_text(cx).expect("tooltip").as_ref(),
            format!("a.txt\n{first} ↔ {second}")
        );
        assert!(view.new_buffer().expect("buffer").read(cx).read_only());
        assert!(!view.can_save(cx));
        assert!(!view.is_dirty(cx));
        assert!(!view.has_conflict(cx));
    });
}

#[gpui::test]
async fn unsaved_working_copy_edits_are_not_lost_on_close(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.commit("a.txt", "one\ntwo\n", "first");
    let (fixture, cx) = open_real(&repo, cx).await;

    let view = open_view(&fixture, "a.txt", "HEAD", NewSide::WorkingCopy, cx)
        .await
        .expect("open");
    view.read_with(cx, |view, cx| assert!(!view.is_dirty(cx)));

    let dirty_events = Rc::new(Cell::new(0));
    let _subscription = cx.update({
        let dirty_events = dirty_events.clone();
        |_, cx| {
            cx.subscribe(&view, move |_, event: &EditorEvent, _| {
                if matches!(event, EditorEvent::DirtyChanged) {
                    dirty_events.set(dirty_events.get() + 1);
                }
            })
        }
    });
    rhs_editor(&view, cx).update_in(cx, |editor, window, cx| {
        editor.move_to_end(&editor::actions::MoveToEnd, window, cx);
        editor.insert("three\n", window, cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |view, cx| {
        assert!(view.is_dirty(cx), "edits in the view make the tab dirty");
        assert!(!view.has_conflict(cx));
    });
    assert!(dirty_events.get() > 0, "the tab hears about it (dirty dot)");

    let pane = fixture
        .workspace
        .read_with(cx, |workspace, _| workspace.active_pane().clone());
    let close = pane.update_in(cx, |pane, window, cx| {
        pane.close_item_by_id(view.item_id(), SaveIntent::Close, window, cx)
    });
    cx.run_until_parked();
    assert!(cx.has_pending_prompt(), "closing asks to save");
    cx.simulate_prompt_answer("Save");
    close.await.expect("close");
    cx.run_until_parked();

    assert_eq!(
        std::fs::read_to_string(repo.work.join("a.txt")).expect("read"),
        "one\ntwo\nthree\n"
    );
    let open_views = pane.read_with(cx, |pane, _| {
        pane.items()
            .filter(|item| item.downcast::<RevisionDiffView>().is_some())
            .count()
    });
    assert_eq!(open_views, 0);
}

#[gpui::test]
async fn binary_content_shows_a_message(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.commit("data.bin", b"\x00\x01\x02binary", "first");
    repo.write("data.bin", b"\x00\x01\x03binary");
    repo.commit("latin.txt", b"caf\xe9\n", "not utf-8");
    let (fixture, cx) = open_real(&repo, cx).await;

    for file in ["data.bin", "latin.txt"] {
        let view = open_view(&fixture, file, "HEAD", NewSide::WorkingCopy, cx)
            .await
            .expect("open");
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.message().map(|message| message.as_ref()),
                Some(BINARY_MESSAGE)
            );
            assert!(view.splittable_editor().is_none());
            assert_eq!(
                view.tab_content_text(0, cx),
                format!("{file} (HEAD) ↔ Working Copy")
            );
        });
    }
}

#[gpui::test]
async fn large_file_shows_a_message(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.commit("big.txt", "small\n", "first");
    repo.write("big.txt", "x".repeat(MAX_FILE_SIZE + 1));
    let (fixture, cx) = open_real(&repo, cx).await;

    let view = open_view(&fixture, "big.txt", "HEAD", NewSide::WorkingCopy, cx)
        .await
        .expect("open");
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.message().map(|message| message.as_ref()),
            Some(LARGE_MESSAGE)
        );
    });

    // Too large at a revision: the blob's size is checked before it is read.
    repo.commit("big.txt", "x".repeat(MAX_FILE_SIZE + 1), "big");
    let view = open_view(
        &fixture,
        "big.txt",
        "HEAD~1",
        NewSide::Revision("HEAD".into()),
        cx,
    )
    .await
    .expect("open");
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.message().map(|message| message.as_ref()),
            Some(LARGE_MESSAGE)
        );
    });
}

#[gpui::test]
async fn language_comes_from_the_path(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    let first = repo.commit("main.rs", "fn main() {}\n", "first");
    let second = repo.commit("main.rs", "fn main() { todo!() }\n", "second");
    let (fixture, cx) = open_real(&repo, cx).await;
    fixture.project.read_with(cx, |project, _| {
        project.languages().add(Arc::new(Language::new(
            LanguageConfig {
                name: "Rust".into(),
                matcher: LanguageMatcher {
                    path_suffixes: vec!["rs".into()],
                    ..Default::default()
                }
                .into(),
                ..Default::default()
            },
            None,
        )));
    });

    let view = open_view(&fixture, "main.rs", &first, NewSide::Revision(second), cx)
        .await
        .expect("open");
    view.read_with(cx, |view, cx| {
        let base = view
            .diff()
            .expect("diff")
            .read(cx)
            .base_text_buffer()
            .clone();
        assert_eq!(
            base.read(cx).language().map(|language| language.name()),
            Some("Rust".into()),
            "the old (left) side"
        );
        assert_eq!(
            view.new_buffer()
                .expect("buffer")
                .read(cx)
                .language()
                .map(|language| language.name()),
            Some("Rust".into()),
            "the new (right) side"
        );
    });
}

#[gpui::test]
async fn unknown_or_option_like_revisions_are_errors(cx: &mut TestAppContext) {
    init_test(cx);
    let repo = TestRepo::new();
    repo.commit("a.txt", "a\n", "first");
    let (fixture, cx) = open_real(&repo, cx).await;

    for rev in ["no-such-branch", "--output=x"] {
        assert!(
            open_view(&fixture, "a.txt", rev, NewSide::WorkingCopy, cx)
                .await
                .is_err(),
            "{rev}"
        );
    }
    assert!(
        open_view(
            &fixture,
            "a.txt",
            "HEAD",
            NewSide::Revision("nope".into()),
            cx
        )
        .await
        .is_err()
    );
    // Missing on both sides.
    assert!(
        open_view(&fixture, "none.txt", "HEAD", NewSide::WorkingCopy, cx)
            .await
            .is_err()
    );
}
